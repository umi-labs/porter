use crate::output::print_json;
use anyhow::Result;
use std::path::Path;

pub fn run(dir: &Path, json: bool) -> Result<i32> {
    let project = super::open(dir)?;
    let paths = porter_engine::report::write_all(&project)?;
    if json {
        print_json(&serde_json::json!({"v": 1, "event": "report", "paths": paths}));
    } else if paths.is_empty() {
        eprintln!("no finished analyse, extract or verify run to report on");
    } else {
        for p in &paths {
            println!("wrote {}", p.display());
        }
    }
    Ok(0)
}
