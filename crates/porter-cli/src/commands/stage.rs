use crate::output::{Printer, wire};
use anyhow::Result;
use clap::ValueEnum;
use porter_engine::events::Summary;
use porter_engine::session::{RunPlan, Session, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::Stage;
use std::path::Path;

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
}

/// `dry_load`: load ran without `--apply`, so its row must not read as a real write.
fn table(rows: &[(Stage, Summary)], dry_load: bool) {
    println!(
        "{:<10} {:>6} {:>8} {:>8} {:>7} {:>7}",
        "stage", "done", "partial", "skipped", "failed", "errors"
    );
    for (s, m) in rows {
        let name = match s {
            Stage::Load if dry_load => "load (dry run)".to_string(),
            _ => wire(s),
        };
        println!(
            "{:<10} {:>6} {:>8} {:>8} {:>7} {:>7}",
            name, m.done, m.partial, m.skipped, m.failed, m.errors
        );
    }
}

/// Runs `plan` through the engine's session; this side only renders and wires SIGINT.
async fn drive(dir: &Path, json: bool, plan: RunPlan) -> Result<i32> {
    tracing::debug!(dir = %dir.display(), "opening project");
    let session = Session::open(dir)?;
    let dry_load = match &plan {
        RunPlan::Stage(_, o) => !o.apply,
        RunPlan::All { apply, .. } => !apply,
    };
    let printer = Printer::start(json, session.subscribe());
    let handle = match session.run(plan) {
        Ok(h) => h,
        Err(e) => {
            drop(session);
            printer.finish().await;
            return Err(e);
        }
    };
    let token = handle.cancel_token();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            token.cancel();
        }
    });
    let out = handle.wait().await;
    // The printer drains until every sender (the session's sink) is gone.
    drop(session);
    printer.finish().await;
    for (_, code, err) in &out.per_stage {
        if let (1, Some(e)) = (code, err) {
            eprintln!("error: {e}");
        }
    }
    for m in &out.messages {
        eprintln!("{m}");
    }
    if out.seeded {
        if json {
            crate::output::print_json(&serde_json::json!({"v": 1, "event": "mapping-seeded"}));
        } else {
            eprintln!("mapping seeded in porter.toml");
        }
    }
    if let Some((stage, reason)) = &out.skipped {
        if json {
            crate::output::print_json(&serde_json::json!(
                {"v": 1, "event": "stage-skipped", "stage": stage, "reason": reason}
            ));
        } else {
            eprintln!("{} skipped: {reason}", wire(stage));
        }
    }
    if !json && !out.summaries.is_empty() {
        table(&out.summaries, dry_load);
    }
    Ok(out.code)
}

pub async fn run(dir: &Path, json: bool, cmd: StageCmd, refresh: bool) -> Result<i32> {
    let schema_mode = match cmd {
        StageCmd::Schema(m) => m.into(),
        _ => SchemaMode::Check,
    };
    let opts = StageOpts {
        refresh,
        apply: matches!(cmd, StageCmd::Load(true)),
        schema_mode,
    };
    drive(dir, json, RunPlan::Stage(cmd.stage(), opts)).await
}

/// Without `apply` load is a dry run, so verify is skipped ("dry run").
pub async fn run_all(dir: &Path, json: bool, keep_going: bool, apply: bool) -> Result<i32> {
    drive(dir, json, RunPlan::All { keep_going, apply }).await
}
