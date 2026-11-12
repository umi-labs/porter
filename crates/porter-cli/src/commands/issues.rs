use crate::output::{print_json, wire};
use anyhow::{Result, bail};
use porter_engine::store::IssueFilter;
use porter_model::Resolution;
use serde::de::DeserializeOwned;
use std::path::Path;
use uuid::Uuid;

pub struct Args {
    pub stage: Option<String>,
    pub kind: Option<String>,
    pub severity: Option<String>,
    pub open: bool,
    pub resolve: Option<String>,
    pub as_: Option<String>,
    pub note: Option<String>,
}

fn parse<T: DeserializeOwned>(what: &str, s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| anyhow::anyhow!("unknown {what} \"{s}\""))
}

pub fn run(dir: &Path, json: bool, a: Args) -> Result<i32> {
    let project = super::open(dir)?;
    if let Some(id) = a.resolve {
        let id: Uuid = id
            .parse()
            .map_err(|_| anyhow::anyhow!("\"{id}\" is not an issue id"))?;
        let r = match a.as_.as_deref() {
            Some("handled") => match a.note {
                Some(note) if !note.trim().is_empty() => Resolution::HandledManually { note },
                _ => bail!("--as handled needs --note <text>"),
            },
            Some("wont-fix") => Resolution::WontFix,
            _ => bail!("--resolve needs --as handled|wont-fix"),
        };
        project.store().resolve_issue(id, r.clone())?;
        if json {
            print_json(
                &serde_json::json!({"v": 1, "event": "issue-resolved", "id": id, "resolution": r}),
            );
        } else {
            println!("resolved {id}");
        }
        return Ok(0);
    }
    let filter = IssueFilter {
        stage: a.stage.as_deref().map(|s| parse("stage", s)).transpose()?,
        kind: a.kind.as_deref().map(|s| parse("kind", s)).transpose()?,
        severity: a
            .severity
            .as_deref()
            .map(|s| parse("severity", s))
            .transpose()?,
        resolution: a.open.then_some(Resolution::Open),
    };
    for i in project.store().issues(&filter)? {
        if json {
            print_json(&serde_json::json!({"v": 1, "event": "issue", "issue": i}));
        } else {
            println!(
                "{} {:<8} {:<7} {:<20} {}",
                i.id,
                wire(&i.stage),
                wire(&i.severity),
                wire(&i.kind),
                i.message
            );
        }
    }
    Ok(0)
}
