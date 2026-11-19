use crate::output::print_json;
use anyhow::Result;
use porter_engine::project::Project;
use porter_engine::session::{STAGES, StageState, overview_of, stage_view};
use serde_json::{Value, json};
use std::path::Path;

/// A stage state in the words of the design README's table ("not run", "dry run", …).
fn word(s: &StageState) -> &'static str {
    match s {
        StageState::Never => "not run",
        StageState::Running => "running",
        StageState::Clean => "clean",
        StageState::Warnings => "warnings",
        StageState::Errors => "errors",
        StageState::Interrupted => "interrupted",
        StageState::CouldNotRun(_) => "failed",
        StageState::DryRun => "dry run",
        StageState::Skipped => "skipped",
    }
}

/// What `porter --json status` prints, minus the envelope; `porter hub push` and the
/// hub's worker store it on the version. `stage_views` is each stage as the desktop and
/// TUI show it (`state` hyphenated: "not-run", "dry-run"; `stale` the reason or null).
pub fn report(p: &Project) -> Result<Value> {
    let o = p.store().overview()?;
    let ov = overview_of(p)?;
    let mut views = serde_json::Map::new();
    for stage in STAGES {
        let v = stage_view(&ov, stage);
        views.insert(
            format!("{stage:?}").to_lowercase(),
            json!({"state": word(&v.state).replace(' ', "-"), "stale": v.stale}),
        );
    }
    Ok(json!({
        "stages": o["stages"], "open_issues": o["open_issues"], "last_runs": o["last_runs"],
        "stage_views": views, "ever_written": ov.ever_written,
    }))
}

pub fn run(dir: &Path, json: bool) -> Result<i32> {
    let p = super::open(dir)?;
    if json {
        // The envelope first, as in every other event.
        let mut r = json!({"v": 1, "event": "status"});
        if let (Some(r), Value::Object(body)) = (r.as_object_mut(), report(&p)?) {
            r.extend(body);
        }
        print_json(&r);
        return Ok(0);
    }
    let o = p.store().overview()?;
    let ov = overview_of(&p)?;
    println!(
        "{:<10} {:<20} {:>6} {:>8} {:>8} {:>7}",
        "stage", "state", "done", "partial", "skipped", "failed"
    );
    for stage in STAGES {
        let name = format!("{stage:?}").to_lowercase();
        let v = stage_view(&ov, stage);
        let c = &o["stages"][&name];
        let n = |k: &str| c[k].as_u64().unwrap_or(0);
        let state = match &v.stale {
            Some(_) => format!("{} (stale)", word(&v.state)),
            None => word(&v.state).to_string(),
        };
        println!(
            "{name:<10} {state:<20} {:>6} {:>8} {:>8} {:>7}{}",
            n("done"),
            n("partial"),
            n("skipped"),
            n("failed"),
            v.stale.map(|r| format!("  ↻ {r}")).unwrap_or_default()
        );
    }
    let sev = |k: &str| o["open_issues"][k].as_u64().unwrap_or(0);
    println!(
        "\nopen issues: {} error, {} warning, {} info",
        sev("error"),
        sev("warning"),
        sev("info")
    );
    Ok(0)
}
