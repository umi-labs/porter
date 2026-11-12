use crate::output::print_json;
use anyhow::{Context, Result};
use porter_engine::store::IssueFilter;
use std::path::Path;

pub fn snapshot(dir: &Path) -> Result<serde_json::Value> {
    let project = super::open(dir)?;
    let mut v = project.store().overview()?;
    v["issues"] = serde_json::to_value(project.store().issues(&IssueFilter::default())?)?;
    v["v"] = 1.into();
    v["event"] = "export".into();
    Ok(v)
}

pub fn run(dir: &Path, json: bool, file: &Path) -> Result<i32> {
    let v = snapshot(dir)?;
    std::fs::write(file, serde_json::to_string_pretty(&v)?)
        .with_context(|| format!("writing {}", file.display()))?;
    if json {
        print_json(&serde_json::json!({"v": 1, "event": "export", "path": file}));
    } else {
        println!("wrote {}", file.display());
    }
    Ok(0)
}
