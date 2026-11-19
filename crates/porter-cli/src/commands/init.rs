use crate::output::print_json;
use anyhow::Result;
use porter_engine::project::Project;
use std::path::Path;

pub fn run(dir: &Path, json: bool, name: &str, adapter: &str) -> Result<i32> {
    // Never prompts: --interactive is accepted for scripts but init takes everything as args.
    let known = porter_engine::registry::source_names();
    if !known.contains(&adapter) {
        anyhow::bail!(
            "unknown adapter \"{adapter}\" (available: {})",
            known.join(", ")
        );
    }
    Project::init(dir, name, adapter)?;
    if json {
        print_json(&serde_json::json!({"v": 1, "event": "init", "dir": dir, "name": name}));
    } else {
        println!("created {}", dir.join("porter.toml").display());
    }
    Ok(0)
}
