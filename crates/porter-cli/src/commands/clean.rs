use crate::output::print_json;
use anyhow::Result;
use std::path::Path;

pub fn run(dir: &Path, json: bool) -> Result<i32> {
    // The store is closed by clean(); the project must not be used afterwards.
    super::open(dir)?.clean()?;
    if json {
        print_json(&serde_json::json!({"v": 1, "event": "clean", "dir": dir}));
    } else {
        println!("cleaned {}", dir.display());
    }
    Ok(0)
}
