use crate::output::{print_json, wire};
use anyhow::{Context, Result};
use porter_engine::store::IssueFilter;
use std::fmt::Write;
use std::path::Path;

pub fn run(dir: &Path, json: bool) -> Result<i32> {
    let project = super::open(dir)?;
    let issues = project.store().issues(&IssueFilter::default())?;
    let mut md = String::from("# Porter report\n\n## Issues\n\n");
    for i in &issues {
        let _ = writeln!(
            md,
            "- [{}] {} ({}, {}): {}",
            wire(&i.severity),
            wire(&i.kind),
            wire(&i.stage),
            wire(&i.resolution),
            i.message
        );
    }
    if issues.is_empty() {
        md.push_str("None.\n");
    }
    std::fs::create_dir_all(project.reports_dir())?;
    let path = project.reports_dir().join("report.md");
    std::fs::write(&path, md).with_context(|| format!("writing {}", path.display()))?;
    if json {
        print_json(
            &serde_json::json!({"v": 1, "event": "report", "path": path, "issues": issues.len()}),
        );
    } else {
        println!("wrote {}", path.display());
    }
    Ok(0)
}
