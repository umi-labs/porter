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
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::destinations::payload::PREVIEW_DONE;
pub use crate::store::IssueFilter as IssueQuery;

/// A finished dry-run load.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub counts: BTreeMap<String, u64>,
    pub at: std::time::SystemTime,
}

pub const STAGES: [Stage; 6] = [
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

/// One normalised document in the content tree.
#[derive(Clone, Debug, PartialEq)]
pub struct DocNode {
    pub id: ItemId,
    pub title: String,
    pub doc_type: String,
    pub path: String,
    /// Some live document names it as parent.
    pub has_children: bool,
    /// The worst status any stage gave it (failed > partial > skipped > done); None
    /// before any stage did.
    pub worst_status: Option<ItemStatus>,
}

#[derive(Clone, Debug, Default)]
pub struct Overview {
    /// Latest run of each stage that has run, in pipeline order.
    pub last_runs: Vec<(Stage, RunRow)>,
    pub open_errors: u64,
    pub open_warnings: u64,
    pub open_infos: u64,
}

/// What a stage's last run honestly amounts to. `Running` and `CouldNotRun` are live
/// states the front end overlays; `stage_state` only derives the stored ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageState {
    Never,
    Running,
    Clean,
    Warnings,
    Errors,
    Interrupted,
    CouldNotRun(String),
}

/// Same rules as `exit_code`: failed items or error issues are never Clean.
pub fn stage_state(overview: &Overview, stage: Stage) -> StageState {
    let Some((_, run)) = overview.last_runs.iter().find(|(s, _)| *s == stage) else {
        return StageState::Never;
    };
    // A run row with no summary started and never ended (crash/kill): not a success.
    let Some(sum) = &run.summary else {
        return StageState::Interrupted;
    };
    match exit_code(sum) {
        3 => StageState::Interrupted,
        2 => StageState::Errors,
        _ if sum.warnings > 0 => StageState::Warnings,
        _ => StageState::Clean,
    }
}

type Running = Arc<Mutex<Option<RunInfo>>>;

pub struct Session {
    project: Arc<Project>,
    config: RwLock<Arc<ProjectConfig>>,
    events: EventSink,
    running: Running,
}

impl Session {
    pub fn open(dir: &Path) -> Result<Session> {
        let project = Project::open(dir)?;
        Ok(Session {
            config: RwLock::new(Arc::new(project.config().clone())),
            project: Arc::new(project),
            events: EventSink::new(4096),
            running: Arc::default(),
        })
    }

    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }

    /// The config in use now; a snapshot, so a later `reload_config` does not change it.
    pub fn config(&self) -> Arc<ProjectConfig> {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Re-reads `porter.toml`. Returns the validation messages (or the parse error);
    /// on a parse error the previous config stays in use.
    pub fn reload_config(&self) -> Vec<String> {
        match ProjectConfig::load(&self.project.dir().join("porter.toml")) {
            Ok(c) => {
                let msgs = c.validate();
                *self.config.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(c);
                msgs
            }
            Err(e) => vec![format!("{e:#}")],
        }
    }

    /// The last completed dry-run load: items per collection and when it finished. None if
    /// no dry run has completed (a cancelled one leaves nothing to rely on).
    pub fn load_preview_counts(&self) -> Option<Preview> {
        let dir = self.project.raw_dir("load-preview");
        let at = std::fs::metadata(dir.join(PREVIEW_DONE))
            .ok()?
            .modified()
            .ok()?;
        let mut counts = BTreeMap::new();
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let n = std::fs::read_dir(e.path()).map_or(0, |d| {
                d.flatten()
                    .filter(|f| f.path().extension().is_some_and(|x| x == "json"))
                    .count() as u64
            });
            if n > 0 {
                counts.insert(e.file_name().to_string_lossy().into_owned(), n);
            }
        }
        Some(Preview { counts, at })
    }

    /// Why a preview can no longer be trusted: the data was re-normalised, or porter.toml
    /// was edited, after it was made. Fails closed: anything that cannot be checked
    /// (file times, run history, timestamps) counts as stale.
    pub fn preview_stale(&self, p: &Preview) -> Option<&'static str> {
        match std::fs::metadata(self.project.dir().join("porter.toml")).and_then(|m| m.modified()) {
            Ok(m) if m > p.at => return Some("porter.toml was edited after it"),
            Ok(_) => {}
            Err(_) => return Some("porter.toml could not be checked"),
        }
        let Ok(runs) = self.project.store().runs(Stage::Normalise) else {
            return Some("the run history could not be read");
        };
        for r in &runs {
            let Some(t) = r.ended_at.as_deref() else {
                continue;
            };
            match chrono::DateTime::parse_from_rfc3339(t) {
                Ok(t) if std::time::SystemTime::from(t) > p.at => {
                    return Some("normalise ran after it");
                }
                Ok(_) => {}
                Err(_) => return Some("the run history has an unreadable timestamp"),
            }
        }
        None
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn running(&self) -> Option<RunInfo> {
        lock(&self.running).clone()
    }

    /// Starts `plan` on a background task. Only one run at a time per session. Each run
    /// re-reads `porter.toml`, as every CLI command does; a file that does not parse fails
    /// the run with that error (validation stays per stage).
    pub fn run(&self, plan: RunPlan) -> Result<RunHandle> {
        let config = {
            let mut slot = lock(&self.running);
            if slot.is_some() {
                bail!("a stage is already running");
            }
            let c = Arc::new(ProjectConfig::load(
                &self.project.dir().join("porter.toml"),
            )?);
            *self.config.write().unwrap_or_else(|e| e.into_inner()) = c.clone();
            *slot = Some(RunInfo { plan: plan.clone() });
            c
        };
        let release = Release(self.running.clone());
        let cancel = CancellationToken::new();
        let env = Env {
            project: self.project.clone(),
            config,
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

    /// Document types of the normalised documents, with how many of each.
    pub async fn doc_types(&self) -> Result<Vec<(String, u64)>> {
        let (p, connector) = (self.project.clone(), self.config().source.adapter.clone());
        blocking(move || p.store().doc_types(&connector)).await
    }

    /// One page of normalised documents under `parent`, by title. With no parent: the
    /// roots, i.e. documents with no parent or whose parent is not a live document (so an
    /// orphan is never unreachable). `doc_type` narrows by type.
    pub async fn children(
        &self,
        parent: Option<ItemId>,
        doc_type: Option<&str>,
        page: Page,
    ) -> Result<Paged<DocNode>> {
        let (p, connector) = (self.project.clone(), self.config().source.adapter.clone());
        let doc_type = doc_type.map(str::to_string);
        blocking(move || {
            let store = p.store();
            let (rows, total) = store.doc_children(
                &connector,
                parent,
                doc_type.as_deref(),
                page.offset,
                page.limit,
            )?;
            let ids: Vec<ItemId> = rows.iter().map(|r| r.0).collect();
            let mut worst: std::collections::HashMap<ItemId, ItemStatus> = Default::default();
            for (id, st) in store.statuses_of(&ids)? {
                let w = worst.entry(id).or_insert_with(|| st.clone());
                if status_rank(&st) > status_rank(w) {
                    *w = st;
                }
            }
            let rows = rows
                .into_iter()
                .map(|(id, title, doc_type, path, has_children)| DocNode {
                    id,
                    title,
                    doc_type,
                    path,
                    has_children,
                    worst_status: worst.remove(&id),
                })
                .collect();
            Ok(Paged { rows, total })
        })
        .await
    }

    pub fn resolve_issue(&self, id: Uuid, r: Resolution) -> Result<()> {
        self.project.store().resolve_issue(id, r)
    }
}

/// Failed > partial > skipped > done.
fn status_rank(s: &ItemStatus) -> u8 {
    match s {
        ItemStatus::Done => 0,
        ItemStatus::Skipped { .. } => 1,
        ItemStatus::Partial { .. } => 2,
        ItemStatus::Failed { .. } => 3,
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
            run_stage(p, c, e, k, s, |ctx| async move {
                // Recorded first, so even an interrupted dry run never reads as a write.
                if !apply {
                    ctx.project.store().mark_dry_run(ctx.run)?;
                }
                dest.load(&ctx, apply).await
            })
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
    fn ov(stage: Stage, summary: Option<Summary>) -> Overview {
        Overview {
            last_runs: vec![(
                stage,
                RunRow {
                    id: "r".into(),
                    porter_version: "0".into(),
                    started_at: "t".into(),
                    ended_at: None,
                    summary,
                    dry_run: false,
                },
            )],
            ..Default::default()
        }
    }

    #[test]
    fn stage_state_from_summary() {
        let s = |f: fn(&mut Summary)| {
            let mut x = Summary::default();
            f(&mut x);
            Some(x)
        };
        let st = |sum| stage_state(&ov(Stage::Load, sum), Stage::Load);
        assert_eq!(
            stage_state(&Overview::default(), Stage::Load),
            StageState::Never
        );
        assert_eq!(
            stage_state(&ov(Stage::Load, None), Stage::Analyse),
            StageState::Never
        );
        assert_eq!(st(s(|x| x.done = 5)), StageState::Clean);
        assert_eq!(st(s(|x| x.warnings = 1)), StageState::Warnings);
        assert_eq!(st(s(|x| x.errors = 1)), StageState::Errors);
        assert_eq!(st(s(|x| x.failed = 1)), StageState::Errors);
        assert_eq!(
            st(s(|x| {
                x.errors = 2;
                x.warnings = 3
            })),
            StageState::Errors
        );
        assert_eq!(st(s(|x| x.interrupted = true)), StageState::Interrupted);
        // Started but never ended: not a success.
        assert_eq!(st(None), StageState::Interrupted);
    }

    use super::*;
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

    #[test]
    fn preview_stale_fails_closed_on_store_error() {
        let d = tempfile::tempdir().unwrap();
        crate::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let s = Session::open(d.path()).unwrap();
        let dir = d.path().join("raw/load-preview");
        std::fs::create_dir_all(dir.join("posts")).unwrap();
        std::fs::write(dir.join("posts/a.json"), "{}").unwrap();
        std::fs::write(dir.join(crate::destinations::payload::PREVIEW_DONE), "").unwrap();
        let p = s.load_preview_counts().unwrap();
        assert_eq!(s.preview_stale(&p), None);
        s.project.store().close().unwrap();
        assert!(
            s.preview_stale(&p).is_some(),
            "unreadable history reads as stale"
        );
    }

    /// The CLI re-reads porter.toml for every command; a session must too, or a run after
    /// analyse seeded `[mapping]` (or after any edit) would use the config it opened with.
    #[tokio::test]
    async fn each_run_reloads_porter_toml() {
        let d = tempfile::tempdir().unwrap();
        Project::init(d.path(), "demo", "wordpress").unwrap();
        let s = Session::open(d.path()).unwrap();
        let toml = d.path().join("porter.toml");
        let text = std::fs::read_to_string(&toml).unwrap();
        std::fs::write(&toml, text.replace("name = \"demo\"", "name = \"renamed\"")).unwrap();
        let plan = RunPlan::Stage(
            Stage::Analyse,
            StageOpts {
                refresh: false,
                apply: false,
                schema_mode: SchemaMode::Check,
            },
        );
        s.run(plan.clone()).unwrap().wait().await;
        assert_eq!(s.config().project.name, "renamed");
        // A parse error fails the run with the error as the CLI prints it, and frees the slot.
        std::fs::write(&toml, "not [toml").unwrap();
        let e = format!(
            "{:#}",
            s.run(plan.clone()).err().expect("bad toml must fail")
        );
        assert!(e.contains("parsing") && e.contains("porter.toml"), "{e}");
        assert!(s.running().is_none());
    }

    /// A dry-run load is stored as one, so no front end can show it as a real write.
    #[tokio::test]
    async fn dry_run_load_is_recorded_as_dry_run() {
        let d = tempfile::tempdir().unwrap();
        Project::init(d.path(), "demo", "wordpress").unwrap();
        let toml = d.path().join("porter.toml");
        let text = std::fs::read_to_string(&toml).unwrap();
        std::fs::write(
            &toml,
            text.replace(
                "[destination]\n",
                "[destination]\nadapter = \"payload\"\nurl = \"https://cms.example.com\"\napi_key_env = \"K\"\n",
            ),
        )
        .unwrap();
        let s = Session::open(d.path()).unwrap();
        let load = |apply| {
            RunPlan::Stage(
                Stage::Load,
                StageOpts {
                    refresh: false,
                    apply,
                    schema_mode: SchemaMode::Check,
                },
            )
        };
        let out = s.run(load(false)).unwrap().wait().await;
        assert_ne!(out.code, 1, "{out:?}");
        let ov = s.overview().await.unwrap();
        let (_, run) = ov
            .last_runs
            .iter()
            .find(|(st, _)| *st == Stage::Load)
            .unwrap();
        assert!(run.dry_run, "a dry-run load must be stored as one");
    }

    #[test]
    fn reload_config_through_shared_ref_and_preview_counts() {
        let d = tempfile::tempdir().unwrap();
        Project::init(d.path(), "demo", "wordpress").unwrap();
        let s = Arc::new(Session::open(d.path()).unwrap());
        assert_eq!(s.config().project.name, "demo");
        assert_eq!(s.load_preview_counts(), None);
        let toml = std::fs::read_to_string(d.path().join("porter.toml")).unwrap();
        std::fs::write(
            d.path().join("porter.toml"),
            toml.replace("name = \"demo\"", "name = \"renamed\""),
        )
        .unwrap();
        s.reload_config();
        assert_eq!(s.config().project.name, "renamed");
        std::fs::write(d.path().join("porter.toml"), "not [toml").unwrap();
        assert!(!s.reload_config().is_empty());
        assert_eq!(s.config().project.name, "renamed", "bad parse keeps old");
        let p = d.path().join("raw/load-preview");
        std::fs::create_dir_all(p.join("posts")).unwrap();
        std::fs::create_dir_all(p.join("media")).unwrap();
        std::fs::write(p.join("posts/a.json"), "{}").unwrap();
        std::fs::write(p.join("posts/b.json"), "{}").unwrap();
        assert_eq!(
            s.load_preview_counts(),
            None,
            "no marker: not a finished dry run"
        );
        std::fs::write(p.join(crate::destinations::payload::PREVIEW_DONE), "").unwrap();
        let c = s.load_preview_counts().unwrap().counts;
        assert_eq!(c.get("posts"), Some(&2));
        assert!(!c.contains_key("media"));
    }
}
