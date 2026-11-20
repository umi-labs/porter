use crate::output::print_json;
use anyhow::Result;
use clap::Subcommand;
use porter_engine::config::ProjectConfig;
use porter_engine::mapping::edit::MappingDoc;
use porter_engine::tidy::suggest::append;
use porter_engine::tidy::{TidyKind, TidyRule};
use std::path::Path;

#[derive(Subcommand)]
pub enum TidyCmd {
    /// Every tidy-up rule: id, on or off, what it does and why
    List,
    /// Add suggested tidy-ups to porter.toml, off (needs a normalised project)
    Suggest,
    /// Turn a tidy-up on
    Enable { id: String },
    /// Turn a tidy-up off
    Disable { id: String },
}

fn show(json: bool, action: &str, r: &TidyRule) {
    if json {
        let (from, into) = match &r.kind {
            TidyKind::MergeTypes { from, into, .. } | TidyKind::MergeBlocks { from, into, .. } => {
                (from, into)
            }
        };
        print_json(&serde_json::json!({
            "v": 1, "event": "tidy", "action": action, "id": r.id, "enabled": r.enabled,
            "label": r.label, "why": r.why, "from": from, "into": into,
        }));
    } else {
        println!(
            "{:<3} {:<20} {}{}",
            if r.enabled { "on" } else { "off" },
            r.id,
            r.label.as_deref().unwrap_or(""),
            r.why
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default()
        );
    }
}

pub fn run(dir: &Path, json: bool, cmd: TidyCmd) -> Result<i32> {
    let file = dir.join("porter.toml");
    match cmd {
        TidyCmd::List => {
            for r in &ProjectConfig::load(&file)?.tidy {
                show(json, "list", r);
            }
        }
        TidyCmd::Suggest => {
            let project = super::open(dir)?;
            for r in &append(&project)? {
                show(json, "suggested", r);
            }
        }
        TidyCmd::Enable { id } => toggle(&file, json, &id, true)?,
        TidyCmd::Disable { id } => toggle(&file, json, &id, false)?,
    }
    Ok(0)
}

fn toggle(file: &Path, json: bool, id: &str, on: bool) -> Result<()> {
    let mut doc = MappingDoc::open(file)?;
    doc.set_tidy_enabled(id, on)?;
    let problems = doc.save();
    if !problems.is_empty() {
        anyhow::bail!("{}", problems.join("; "));
    }
    if let Some(r) = doc.config()?.tidy.iter().find(|r| r.id == id) {
        show(json, if on { "enabled" } else { "disabled" }, r);
    }
    Ok(())
}
