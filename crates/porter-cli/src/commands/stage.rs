use crate::output::{Printer, wire};
use anyhow::{Result, bail};
use clap::ValueEnum;
use porter_engine::config::ProjectConfig;
use porter_engine::events::{EventSink, Summary, exit_code};
use porter_engine::mapping::{MappingConfig, write_seed_if_empty};
use porter_engine::project::Project;
use porter_engine::registry;
use porter_engine::stage::{SchemaMode, run_stage};
use porter_model::Stage;
use std::cell::RefCell;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, ValueEnum)]
pub enum Mode {
    Check,
    Generate,
    Prompt,
    Spec,
}

impl From<Mode> for SchemaMode {
    fn from(m: Mode) -> SchemaMode {
        match m {
            Mode::Check => SchemaMode::Check,
            Mode::Generate => SchemaMode::Generate,
            Mode::Prompt => SchemaMode::Prompt,
            Mode::Spec => SchemaMode::Spec,
        }
    }
}

#[derive(Clone, Copy)]
pub enum StageCmd {
    Analyse,
    Extract,
    Normalise,
    Schema(Mode),
    Load(bool),
    Verify,
}

impl StageCmd {
    fn stage(self) -> Stage {
        match self {
            StageCmd::Analyse => Stage::Analyse,
            StageCmd::Extract => Stage::Extract,
            StageCmd::Normalise => Stage::Normalise,
            StageCmd::Schema(_) => Stage::Schema,
            StageCmd::Load(_) => Stage::Load,
            StageCmd::Verify => Stage::Verify,
        }
    }

    fn needs_destination(self) -> bool {
        matches!(
            self,
            StageCmd::Schema(_) | StageCmd::Load(_) | StageCmd::Verify
        )
    }
}

struct Env {
    project: Arc<Project>,
    config: Arc<ProjectConfig>,
    events: EventSink,
    cancel: CancellationToken,
    refresh: bool,
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

fn validate(env: &Env, cmd: StageCmd) -> Result<()> {
    let errs: Vec<String> = env
        .config
        .validate()
        .into_iter()
        .filter(|e| cmd.needs_destination() || !e.starts_with("destination."))
        .collect();
    if !errs.is_empty() {
        bail!("porter.toml: {}", errs.join("; "));
    }
    Ok(())
}

async fn exec(env: &Env, cmd: StageCmd) -> Result<Summary> {
    validate(env, cmd)?;
    let cfg = &env.config;
    let go = |stage| {
        (
            env.project.clone(),
            cfg.clone(),
            env.events.clone(),
            env.cancel.clone(),
            stage,
        )
    };
    let stage = cmd.stage();
    if matches!(
        cmd,
        StageCmd::Analyse | StageCmd::Extract | StageCmd::Normalise
    ) {
        let Some(src) = registry::source(&cfg.source.adapter) else {
            bail!(
                "no source adapter named \"{}\" (available: {})",
                cfg.source.adapter,
                list(registry::source_names())
            );
        };
        let (p, c, e, k, s) = go(stage);
        let refresh = env.refresh;
        return run_stage(p, c, e, k, s, |mut ctx| async move {
            ctx.refresh = refresh;
            match cmd {
                StageCmd::Normalise => src.normalise(&ctx),
                _ => {
                    for conn in src.connectors(&ctx.config)? {
                        if ctx.cancel.is_cancelled() {
                            break;
                        }
                        if matches!(cmd, StageCmd::Analyse) {
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
    let (p, c, e, k, s) = go(stage);
    match cmd {
        StageCmd::Schema(m) => {
            run_stage(p, c, e, k, s, |ctx| async move {
                dest.schema(m.into(), &ctx).await
            })
            .await
        }
        StageCmd::Load(apply) => {
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
        StageCmd::Verify => {
            run_stage(p, c, e, k, s, |ctx| async move {
                porter_engine::verify::run(&ctx, dest.as_ref()).await
            })
            .await
        }
        _ => bail!("not a destination stage"),
    }
}

fn list(names: &[&str]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

fn env(dir: &Path, printer: &Printer) -> Result<Env> {
    let project = super::open(dir)?;
    let config = Arc::new(project.config().clone());
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            token.cancel();
        }
    });
    Ok(Env {
        project: Arc::new(project),
        config,
        events: printer.sink(),
        cancel,
        refresh: false,
    })
}

fn table(rows: &[(Stage, Summary)]) {
    println!(
        "{:<10} {:>6} {:>8} {:>8} {:>7} {:>7}",
        "stage", "done", "partial", "skipped", "failed", "errors"
    );
    for (s, m) in rows {
        println!(
            "{:<10} {:>6} {:>8} {:>8} {:>7} {:>7}",
            wire(s),
            m.done,
            m.partial,
            m.skipped,
            m.failed,
            m.errors
        );
    }
}

/// One stage's exit code. A stage that errors, or is cancelled, maps to 1 / 3.
async fn code_of(env: &Env, cmd: StageCmd, rows: &RefCell<Vec<(Stage, Summary)>>) -> i32 {
    match exec(env, cmd).await {
        Ok(s) => {
            let c = exit_code(&s);
            rows.borrow_mut().push((cmd.stage(), s));
            c
        }
        Err(_) if env.cancel.is_cancelled() => 3,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
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

/// After a successful analyse, writes a `[mapping]` starting point if the user has none.
fn seed_mapping(env: &Env) -> bool {
    let seed = MappingConfig::seed_from(env.project.store());
    match write_seed_if_empty(env.project.dir(), &seed) {
        Ok(wrote) => wrote,
        Err(e) => {
            eprintln!("warning: could not seed mapping: {e:#}");
            false
        }
    }
}

async fn drive(
    dir: &Path,
    json: bool,
    cmds: &[StageCmd],
    keep_going: bool,
    refresh: bool,
) -> Result<i32> {
    let printer = Printer::start(json);
    let mut env = match env(dir, &printer) {
        Ok(e) => e,
        Err(e) => {
            printer.finish().await;
            return Err(e);
        }
    };
    env.refresh = refresh;
    let rows = RefCell::new(Vec::new());
    let worst = run_loop(&env.cancel, cmds, keep_going, |cmd| {
        code_of(&env, cmd, &rows)
    })
    .await;
    let rows = rows.into_inner();
    let seeded = matches!(worst, 0 | 2)
        && cmds.iter().any(|c| matches!(c, StageCmd::Analyse))
        && seed_mapping(&env);
    drop(env);
    printer.finish().await;
    if seeded {
        if json {
            crate::output::print_json(&serde_json::json!({"v": 1, "event": "mapping-seeded"}));
        } else {
            eprintln!("mapping seeded in porter.toml");
        }
    }
    if !json && !rows.is_empty() {
        table(&rows);
    }
    Ok(worst)
}

pub async fn run(dir: &Path, json: bool, cmd: StageCmd, refresh: bool) -> Result<i32> {
    drive(dir, json, &[cmd], false, refresh).await
}

pub async fn run_all(dir: &Path, json: bool, keep_going: bool, apply: bool) -> Result<i32> {
    let steps = [
        StageCmd::Analyse,
        StageCmd::Extract,
        StageCmd::Normalise,
        StageCmd::Schema(Mode::Check),
        StageCmd::Load(apply),
        StageCmd::Verify,
    ];
    drive(dir, json, &steps, keep_going, false).await
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
}
