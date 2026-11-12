use crate::output::print_json;
use anyhow::Result;
use std::path::Path;

pub fn run(dir: &Path, json: bool) -> Result<i32> {
    let o = super::open(dir)?.store().overview()?;
    if json {
        print_json(&serde_json::json!({
            "v": 1, "event": "status",
            "stages": o["stages"], "open_issues": o["open_issues"], "last_runs": o["last_runs"],
        }));
        return Ok(0);
    }
    println!(
        "{:<10} {:>6} {:>8} {:>8} {:>7}",
        "stage", "done", "partial", "skipped", "failed"
    );
    for stage in [
        "analyse",
        "extract",
        "normalise",
        "schema",
        "load",
        "verify",
    ] {
        let c = &o["stages"][stage];
        let n = |k: &str| c[k].as_u64().unwrap_or(0);
        println!(
            "{stage:<10} {:>6} {:>8} {:>8} {:>7}",
            n("done"),
            n("partial"),
            n("skipped"),
            n("failed")
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
