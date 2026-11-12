//! The one way front ends (CLI, terminal UI, desktop app) drive a project: open it,
//! run stages on a background task, subscribe to events, and query the store without
//! blocking the caller's runtime.

use crate::config::ProjectConfig;
use crate::events::{Event, EventSink, Summary, exit_code};
use crate::mapping::{MappingConfig, write_seed_if_empty};
use crate::project::Project;
use crate::registry;
use crate::stage::{SchemaMode, run_stage};
use crate::store::RunRow;
use anyhow::{Result, bail};
use porter_model::{Issue, ItemId, ItemKind, ItemStatus, Resolution, Severity, SourceRef, Stage};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use crate::store::IssueFilter as IssueQuery;

const STAGES: [Stage; 6] = [
    Stage::Analyse,
    Stage::Extract,
    Stage::Normalise,
    Stage::Schema,
    Stage::Load,
    Stage::Verify,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageOpts {
    /// Redo work whose output already exists (analyse, extract, normalise).
    pub refresh: bool,
    /// Load writes to the destination; otherwise it is a dry run.
    pub apply: bool,
    pub schema_mode: SchemaMode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunPlan {
    Stage(Stage, StageOpts),
    /// analyse → load, then verify only with `apply` (a dry-run load leaves nothing to verify).
    All {
        keep_going: bool,
        apply: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunInfo {
    pub plan: RunPlan,
}

#[derive(Clone, Debug, Default)]
pub struct RunOutcome {
    /// Each stage attempted, with its exit code and, if it could not run or was
    /// interrupted mid-way, the error.
    pub per_stage: Vec<(Stage, i32, Option<String>)>,
    /// Worst code, by precedence 1 (could not run) > 3 (interrupted) > 2 (issues) > 0.
    pub code: i32,
    /// Summaries of the stages that ran to the end (possibly interrupted).
    pub summaries: Vec<(Stage, Summary)>,
    /// Analyse wrote a `[mapping]` starting point into `porter.toml`.
    pub seeded: bool,
    /// A stage left out on purpose, with why (verify after a dry-run load).
    pub skipped: Option<(Stage, String)>,
    /// Human-readable warnings and errors that aren't any one stage's.
    pub messages: Vec<String>,
}

pub struct RunHandle {
    cancel: CancellationToken,
    task: JoinHandle<RunOutcome>,
}

impl RunHandle {
    /// Stops the run at the next safe point; it ends with code 3 (resumable).
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// For a signal handler or UI that cancels while another task is in `wait`.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// A panicked run comes back as code 1 with the panic message, never a hang.
    pub async fn wait(self) -> RunOutcome {
        match self.task.await {
            Ok(o) => o,
            Err(e) => {
                let why = if e.is_panic() {
                    let p = e.into_panic();
                    p.downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into())
                } else {
                    e.to_string()
                };
                RunOutcome {
                    code: 1,
                    messages: vec![format!("error: run panicked: {why}")],
                    ..Default::default()
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub offset: u64,
    pub limit: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paged<T> {
    pub rows: Vec<T>,
    pub total: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemQuery {
    pub kind: Option<ItemKind>,
    /// Case-insensitive substring of the source id, url or title.
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemRow {
    pub id: ItemId,
    pub kind: ItemKind,
    pub label: String,
    pub title: Option<String>,
    pub statuses: Vec<(Stage, ItemStatus)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemDetail {
    pub row: ItemRow,
    pub source: SourceRef,
    /// The stored row: a connector's raw summary before normalise, the neutral model after.
    pub data: serde_json::Value,
    pub issues: Vec<Issue>,
}

#[derive(Clone, Debug, Default)]
pub struct Overview {
    /// Latest run of each stage that has run, in pipeline order.
    pub last_runs: Vec<(Stage, RunRow)>,
    pub open_errors: u64,
    pub open_warnings: u64,
    pub open_infos: u64,
}

type Running = Arc<Mutex<Option<RunInfo>>>;

pub struct Session {
    project: Arc<Project>,
    config: Arc<ProjectConfig>,
    events: EventSink,
    running: Running,
}

impl Session {
    pub fn open(dir: &Path) -> Result<Session> {
        let project = Project::open(dir)?;
        Ok(Session {
            config: Arc::new(project.config().clone()),
            project: Arc::new(project),
            events: EventSink::new(4096),
            running: Arc::default(),
        })
    }

    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }

    pub fn config(&self) -> &ProjectConfig {
        &self.config
    }

    /// Re-reads `porter.toml`. Returns the validation messages (or the parse error);
    /// on a parse error the previous config stays in use.
    pub fn reload_config(&mut self) -> Vec<String> {
        match ProjectConfig::load(&self.project.dir().join("porter.toml")) {
            Ok(c) => {
                let msgs = c.validate();
                self.config = Arc::new(c);
                msgs
            }
            Err(e) => vec![format!("{e:#}")],
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn running(&self) -> Option<RunInfo> {
        lock(&self.running).clone()
    }

    /// Starts `plan` on a background task. Only one run at a time per session.
    pub fn run(&self, plan: RunPlan) -> Result<RunHandle> {
        {
            let mut slot = lock(&self.running);
            if slot.is_some() {
                bail!("a stage is already running");
            }
            *slot = Some(RunInfo { plan: plan.clone() });
        }
        let release = Release(self.running.clone());
        let cancel = CancellationToken::new();
        let env = Env {
            project: self.project.clone(),
            config: self.config.clone(),
            events: self.events.clone(),
            cancel: cancel.clone(),
        };
        let task = tokio::spawn(async move {
            let _release = release;
            drive(env, plan).await
        });
        Ok(RunHandle { cancel, task })
    }

    pub async fn overview(&self) -> Result<Overview> {
        let p = self.project.clone();
        blocking(move || {
            let store = p.store();
            let mut o = Overview::default();
            for stage in STAGES {
                if let Some(r) = store.runs(stage)?.pop() {
                    o.last_runs.push((stage, r));
                }
            }
            // ponytail: loads every open issue to count them; a GROUP BY when projects get huge.
            let open = store.issues(&IssueQuery {
                resolution: Some(Resolution::Open),
                ..Default::default()
            })?;
            for i in open {
                match i.severity {
                    Severity::Error => o.open_errors += 1,
                    Severity::Warning => o.open_warnings += 1,
                    Severity::Info => o.open_infos += 1,
                }
            }
            Ok(o)
        })
        .await
    }

    pub async fn issues(&self, f: IssueQuery, page: Page) -> Result<Paged<Issue>> {
        let p = self.project.clone();
        blocking(move || {
            // ponytail: pages in memory; push LIMIT/OFFSET into the store if issues reach 100k+.
            let all = p.store().issues(&f)?;
            Ok(Paged {
                total: all.len() as u64,
                rows: all
                    .into_iter()
                    .skip(usize::try_from(page.offset).unwrap_or(usize::MAX))
                    .take(usize::try_from(page.limit).unwrap_or(usize::MAX))
                    .collect(),
            })
        })
        .await
    }

    pub async fn items(&self, f: ItemQuery, page: Page) -> Result<Paged<ItemRow>> {
        let p = self.project.clone();
        blocking(move || {
            let store = p.store();
            let (rows, total) =
                store.item_page(f.kind, f.text.as_deref(), page.offset, page.limit)?;
            let rows = rows
                .into_iter()
                .map(|(id, kind, label, title)| {
                    Ok(ItemRow {
                        id,
                        kind,
                        label,
                        title,
                        statuses: store.item_statuses(id)?,
                    })
                })
                .collect::<Result<_>>()?;
            Ok(Paged { rows, total })
        })
        .await
    }

    pub async fn item(&self, id: ItemId) -> Result<ItemDetail> {
        let p = self.project.clone();
        blocking(move || {
            let store = p.store();
            let Some((kind, source, data)) = store.item_source(id)? else {
                bail!("no item {id}");
            };
            let title = data
                .get("title")
                .and_then(|t| t.as_str())
                .map(str::to_string);
            let row = ItemRow {
                id,
                kind,
                label: source
                    .url
                    .clone()
                    .unwrap_or_else(|| source.source_id.clone()),
                title,
                statuses: store.item_statuses(id)?,
            };
            let issues = store
                .issues(&IssueQuery::default())?
                .into_iter()
                .filter(|i| i.item == Some(id))
                .collect();
            Ok(ItemDetail {
                row,
                source,
                data,
                issues,
            })
        })
        .await
    }

    pub fn resolve_issue(&self, id: Uuid, r: Resolution) -> Result<()> {
        self.project.store().resolve_issue(id, r)
    }
}

fn lock(m: &Mutex<Option<RunInfo>>) -> std::sync::MutexGuard<'_, Option<RunInfo>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Frees the session's run slot when the run task ends, panics included.
struct Release(Running);

impl Drop for Release {
    fn drop(&mut self) {
        *lock(&self.0) = None;
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await?
}

struct Env {
    project: Arc<Project>,
    config: Arc<ProjectConfig>,
    events: EventSink,
    cancel: CancellationToken,
}

#[derive(Clone, Copy)]
enum Step {
    Analyse,
    Extract,
    Normalise,
    Schema(SchemaMode),
    Load(bool),
    Verify,
}

impl Step {
    fn stage(self) -> Stage {
        match self {
            Step::Analyse => Stage::Analyse,
            Step::Extract => Stage::Extract,
            Step::Normalise => Stage::Normalise,
            Step::Schema(_) => Stage::Schema,
            Step::Load(_) => Stage::Load,
            Step::Verify => Stage::Verify,
        }
    }

    fn needs_destination(self) -> bool {
        matches!(self, Step::Schema(_) | Step::Load(_) | Step::Verify)
    }
}

/// Exit code precedence: 1 (could not run) > 3 (interrupted) > 2 (issues) > 0.
fn worse(a: i32, b: i32) -> i32 {
    let rank = |c: i32| match c {
        1 => 3,
        3 => 2,
        2 => 1,
        _ => 0,
    };
    if rank(b) > rank(a) { b } else { a }
}

fn validate(cfg: &ProjectConfig, step: Step) -> Result<()> {
    let errs: Vec<String> = cfg
        .validate()
        .into_iter()
        .filter(|e| step.needs_destination() || !e.starts_with("destination."))
        .collect();
    if !errs.is_empty() {
        bail!("porter.toml: {}", errs.join("; "));
    }
    Ok(())
}

fn list(names: &[&str]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

async fn exec(env: &Env, step: Step, refresh: bool) -> Result<Summary> {
    validate(&env.config, step)?;
    let cfg = &env.config;
    let (p, c, e, k, s) = (
        env.project.clone(),
        cfg.clone(),
        env.events.clone(),
        env.cancel.clone(),
        step.stage(),
    );
    if matches!(step, Step::Analyse | Step::Extract | Step::Normalise) {
        let Some(src) = registry::source(&cfg.source.adapter) else {
            bail!(
                "no source adapter named \"{}\" (available: {})",
                cfg.source.adapter,
                list(registry::source_names())
            );
        };
        return run_stage(p, c, e, k, s, |mut ctx| async move {
            ctx.refresh = refresh;
            match step {
                Step::Normalise => src.normalise(&ctx),
                _ => {
                    for conn in src.connectors(&ctx.config)? {
                        if ctx.cancel.is_cancelled() {
                            break;
                        }
                        if matches!(step, Step::Analyse) {
                            conn.analyse(&ctx).await?;
                        } else {
                            conn.extract(&ctx).await?;
                        }
                    }
                    Ok(())
                }
            }
        })
        .await;
    }
    let Some(dest) = registry::destination(&cfg.destination.adapter, cfg) else {
        bail!(
            "no destination adapter named \"{}\" (available: {})",
            cfg.destination.adapter,
            list(registry::destination_names())
        );
    };
    match step {
        Step::Schema(m) => {
            run_stage(
                p,
                c,
                e,
                k,
                s,
                |ctx| async move { dest.schema(m, &ctx).await },
            )
            .await
        }
        Step::Load(apply) => {
            run_stage(
                p,
                c,
                e,
                k,
                s,
                |ctx| async move { dest.load(&ctx, apply).await },
            )
            .await
        }
        Step::Verify => {
            run_stage(p, c, e, k, s, |ctx| async move {
                crate::verify::run(&ctx, dest.as_ref()).await
            })
            .await
        }
        _ => bail!("not a destination stage"),
    }
}

/// Runs `items` in order, returning the worst exit code. Never starts an item
/// once the token is cancelled, and stops after code 3, even with `keep_going`.
async fn run_loop<T: Copy, F, Fut>(
    cancel: &CancellationToken,
    items: &[T],
    keep_going: bool,
    mut f: F,
) -> i32
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = i32>,
{
    let mut worst = 0;
    for &item in items {
        if cancel.is_cancelled() {
            return worse(worst, 3);
        }
        let c = f(item).await;
        worst = worse(worst, c);
        if c == 3 || cancel.is_cancelled() {
            return worse(worst, 3);
        }
        if c != 0 && !keep_going {
            break;
        }
    }
    worst
}

fn lock_out(m: &Mutex<RunOutcome>) -> std::sync::MutexGuard<'_, RunOutcome> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

async fn drive(env: Env, plan: RunPlan) -> RunOutcome {
    let (steps, keep_going, refresh, skip) = match plan {
        RunPlan::Stage(stage, o) => {
            let step = match stage {
                Stage::Analyse => Step::Analyse,
                Stage::Extract => Step::Extract,
                Stage::Normalise => Step::Normalise,
                Stage::Schema => Step::Schema(o.schema_mode),
                Stage::Load => Step::Load(o.apply),
                Stage::Verify => Step::Verify,
            };
            (vec![step], false, o.refresh, None)
        }
        RunPlan::All { keep_going, apply } => {
            let mut steps = vec![
                Step::Analyse,
                Step::Extract,
                Step::Normalise,
                Step::Schema(SchemaMode::Check),
                Step::Load(apply),
            ];
            // Without apply load is a dry run, so there is nothing in the destination to
            // verify: verify is skipped rather than run against what an earlier run left.
            let skip = if apply {
                steps.push(Step::Verify);
                None
            } else {
                Some((Stage::Verify, "dry run".to_string()))
            };
            (steps, keep_going, false, skip)
        }
    };
    // A Mutex, not a RefCell: the run future must be Send. Never held across an await.
    let out = Mutex::new(RunOutcome::default());
    let code = run_loop(&env.cancel, &steps, keep_going, |step| {
        let (env, out) = (&env, &out);
        async move {
            let stage = step.stage();
            let (code, err) = match exec(env, step, refresh).await {
                Ok(s) => {
                    let c = exit_code(&s);
                    lock_out(out).summaries.push((stage, s));
                    (c, None)
                }
                Err(e) if env.cancel.is_cancelled() => (3, Some(format!("{e:#}"))),
                Err(e) => (1, Some(format!("{e:#}"))),
            };
            lock_out(out).per_stage.push((stage, code, err));
            code
        }
    })
    .await;
    let mut out = out.into_inner().unwrap_or_else(|e| e.into_inner());
    out.code = code;
    // After a successful analyse, write a `[mapping]` starting point if the user has none.
    if matches!(code, 0 | 2) && steps.iter().any(|s| matches!(s, Step::Analyse)) {
        let seed = MappingConfig::seed_from(env.project.store());
        match write_seed_if_empty(env.project.dir(), &seed) {
            Ok(wrote) => out.seeded = wrote,
            Err(e) => out
                .messages
                .push(format!("warning: could not seed mapping: {e:#}")),
        }
    }
    // Reported once the stage before it has run to the end.
    let last = steps.last().map(|s| s.stage());
    out.skipped = skip.filter(|_| out.summaries.iter().any(|(s, _)| Some(*s) == last));
    out
}

#[cfg(test)]
mod tests {
    use super::{run_loop, worse};
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn interrupt_stops_run_even_with_keep_going() {
        let cancel = CancellationToken::new();
        let called = std::cell::RefCell::new(Vec::new());
        let code = run_loop(&cancel, &[1, 2, 3], true, |n| {
            called.borrow_mut().push(n);
            if n == 2 {
                cancel.cancel();
            }
            async move { if n == 2 { 3 } else { 2 } }
        })
        .await;
        assert_eq!(code, 3);
        assert_eq!(*called.borrow(), vec![1, 2]);
    }

    #[tokio::test]
    async fn pre_cancelled_token_calls_nothing() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let called = std::cell::Cell::new(0);
        let code = run_loop(&cancel, &[1, 2], true, |_| {
            called.set(called.get() + 1);
            async { 0 }
        })
        .await;
        assert_eq!((code, called.get()), (3, 0));
    }

    #[test]
    fn exit_precedence_is_1_3_2_0() {
        assert_eq!(worse(2, 3), 3);
        assert_eq!(worse(3, 2), 3);
        assert_eq!(worse(3, 1), 1);
        assert_eq!(worse(1, 2), 1);
        assert_eq!(worse(0, 2), 2);
        assert_eq!(worse(2, 0), 2);
    }

    #[tokio::test]
    async fn panicked_run_waits_as_code_1() {
        let h = super::RunHandle {
            cancel: CancellationToken::new(),
            task: tokio::spawn(async { panic!("boom") }),
        };
        let out = h.wait().await;
        assert_eq!(out.code, 1);
        assert!(out.messages[0].contains("boom"), "{:?}", out.messages);
    }
}
