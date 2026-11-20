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
    /// Turn a suggestion down: off, kept in porter.toml so it isn't suggested again
    Dismiss { id: String },
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
            "dismissed": r.dismissed, "label": r.label, "why": r.why, "from": from, "into": into,
        }));
    } else {
        println!(
            "{:<9} {:<20} {}{}",
            if r.dismissed {
                "dismissed"
            } else if r.enabled {
                "on"
            } else {
                "off"
            },
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
        TidyCmd::Enable { id } => change(&file, json, &id, "enabled", |d| {
            d.set_tidy_enabled(&id, true)
        })?,
        TidyCmd::Disable { id } => change(&file, json, &id, "disabled", |d| {
            d.set_tidy_enabled(&id, false)
        })?,
        TidyCmd::Dismiss { id } => change(&file, json, &id, "dismissed", |d| d.dismiss_tidy(&id))?,
    }
    Ok(0)
}

fn change(
    file: &Path,
    json: bool,
    id: &str,
    action: &str,
    f: impl FnOnce(&mut MappingDoc) -> Result<()>,
) -> Result<()> {
    let mut doc = MappingDoc::open(file)?;
    f(&mut doc)?;
    let problems = doc.save();
    if !problems.is_empty() {
        anyhow::bail!("{}", problems.join("; "));
    }
    if let Some(r) = doc.config()?.tidy.iter().find(|r| r.id == id) {
        show(json, action, r);
    }
    Ok(())
}
