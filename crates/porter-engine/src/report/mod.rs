//! Self-contained HTML reports: inline CSS, no scripts, no external requests.
//! Content from the client's site is always escaped (maud) and never turned into
//! a link, so it can't inject markup or `javascript:` URLs.

mod analysis;
mod extract;
mod verify;

pub use analysis::render_analysis;
pub use extract::render_extract;
pub use verify::render_verify;

use crate::project::Project;
use crate::store::RunRow;
use anyhow::{Context, Result};
use maud::{DOCTYPE, Markup, html};
use porter_model::{Issue, ItemStatus, Resolution, Stage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What each connector contributed and where access fell short. Written by the
/// analyse stage, rendered by the analysis report.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Coverage {
    pub platform: Option<String>,
    pub builders: Vec<String>,
    pub plugins: Vec<String>,
    pub connectors: Vec<ConnectorCoverage>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ConnectorCoverage {
    pub name: String,
    pub contributed: BTreeMap<String, u64>,
    pub gaps: Vec<String>,
}

fn coverage_path(project: &Project) -> PathBuf {
    project.raw_dir("analysis").join("coverage.json")
}

pub fn read_coverage(project: &Project) -> Result<Option<Coverage>> {
    let p = coverage_path(project);
    if !p.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    Ok(Some(
        serde_json::from_str(&text).with_context(|| format!("parsing {}", p.display()))?,
    ))
}

pub fn write_coverage(project: &Project, c: &Coverage) -> Result<()> {
    let p = coverage_path(project);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(c)?)
        .with_context(|| format!("writing {}", p.display()))
}

/// Writes `reports/{analysis,extract,verify}.html` for each stage with a finished run.
pub fn write_all(project: &Project) -> Result<Vec<PathBuf>> {
    let dir = project.reports_dir();
    std::fs::create_dir_all(&dir)?;
    let mut out = Vec::new();
    for (stage, name, render) in [
        (
            Stage::Analyse,
            "analysis",
            render_analysis as fn(&Project) -> Result<String>,
        ),
        (Stage::Extract, "extract", render_extract),
        (Stage::Verify, "verify", render_verify),
    ] {
        if last_finished(project, stage)?.is_none() {
            continue;
        }
        let p = dir.join(format!("{name}.html"));
        std::fs::write(&p, render(project)?).with_context(|| format!("writing {}", p.display()))?;
        out.push(p);
    }
    Ok(out)
}

fn last_finished(project: &Project, stage: Stage) -> Result<Option<RunRow>> {
    Ok(project
        .store()
        .runs(stage)?
        .into_iter()
        .rev()
        .find(|r| r.ended_at.is_some()))
}

fn tag(s: &ItemStatus) -> &'static str {
    match s {
        ItemStatus::Done => "done",
        ItemStatus::Partial { .. } => "partial",
        ItemStatus::Skipped { .. } => "skipped",
        ItemStatus::Failed { .. } => "failed",
    }
}

fn wire<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Object(m)) => m.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

/// "12 done, 1 failed" style cell from a tag -> count map.
fn counts(m: &BTreeMap<&'static str, u64>) -> String {
    if m.is_empty() {
        return "-".into();
    }
    m.iter()
        .map(|(k, v)| format!("{v} {k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn human_bytes(n: u64) -> String {
    let mut v = n as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if v < 1024.0 {
            return if unit == "B" {
                format!("{n} B")
            } else {
                format!("{v:.1} {unit}")
            };
        }
        v /= 1024.0;
    }
    format!("{v:.1} TB")
}

/// Issues of `stages`, grouped by kind.
fn issues_section(issues: &[Issue]) -> Markup {
    let mut by_kind: BTreeMap<String, Vec<&Issue>> = BTreeMap::new();
    for i in issues {
        by_kind.entry(wire(&i.kind)).or_default().push(i);
    }
    html! {
        h2 { "Issues" }
        @if issues.is_empty() { p.muted { "No issues." } }
        @for (kind, list) in &by_kind {
            details open[list.len() <= 10] {
                summary { (kind) " " span.badge { (list.len()) } }
                div.scroll { table {
                    thead { tr { th { "Severity" } th { "Stage" } th { "Message" } th { "Resolution" } } }
                    tbody { @for i in list { tr {
                        td { span class=(format!("sev {}", wire(&i.severity))) { (wire(&i.severity)) } }
                        td { (wire(&i.stage)) }
                        td { (i.message) }
                        td { (match &i.resolution {
                            Resolution::HandledManually { note } => format!("handled manually: {note}"),
                            r => wire(r),
                        }) }
                    } } }
                } }
            }
        }
    }
}

fn page(project: &Project, title: &str, run: Option<&RunRow>, body: Markup) -> String {
    let when = run
        .and_then(|r| r.ended_at.clone())
        .unwrap_or_else(|| "no finished run".into());
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " - " (project.config().project.name) }
                style { (maud::PreEscaped(CSS)) }
            }
            body { main {
                header {
                    h1 { (title) }
                    p.muted {
                        "Project " strong { (project.config().project.name) }
                        " · run " (when)
                        " · porter " (env!("CARGO_PKG_VERSION"))
                    }
                }
                (body)
            } }
        }
    }
    .into_string()
}

fn stat(label: &str, value: impl std::fmt::Display) -> Markup {
    html! { div.stat { div.n { (value) } div.muted { (label) } } }
}

fn runs_table(runs: &[RunRow]) -> Markup {
    html! {
        h2 { "Run history" }
        @if runs.is_empty() { p.muted { "No runs." } } @else {
            div.scroll { table {
                thead { tr { th { "Started" } th { "Ended" } th { "Version" } th { "Done" } th { "Partial" } th { "Skipped" } th { "Failed" } th { "Errors" } th { "Outcome" } } }
                tbody { @for r in runs.iter().rev() { tr {
                    td { (r.started_at) }
                    td { (r.ended_at.clone().unwrap_or_else(|| "-".into())) }
                    td { (r.porter_version) }
                    @if let Some(s) = &r.summary {
                        td { (s.done) } td { (s.partial) } td { (s.skipped) } td { (s.failed) } td { (s.errors) }
                        td { (if s.interrupted { "interrupted" } else { "finished" }) }
                    } @else { td colspan="6" { "in progress" } }
                } } }
            } }
        }
    }
}

const CSS: &str = r#"
:root{--bg:#fafaf9;--fg:#1c1917;--muted:#6b6560;--card:#fff;--line:#e4e0db;--accent:#2457c5;--ok:#1a7f45;--warn:#9a6700;--err:#b42318}
@media (prefers-color-scheme:dark){:root{--bg:#161412;--fg:#ece9e4;--muted:#9a948d;--card:#1f1c19;--line:#35302b;--accent:#8ab0ff;--ok:#5ccd8b;--warn:#e3b341;--err:#ff8077}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:64rem;margin:0 auto;padding:1rem}
h1{font-size:1.6rem;margin:.5rem 0 0}h2{font-size:1.2rem;margin:2rem 0 .5rem;border-bottom:1px solid var(--line);padding-bottom:.25rem}
.muted{color:var(--muted)}
.stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(9rem,1fr));gap:.75rem}
.stat{background:var(--card);border:1px solid var(--line);border-radius:.5rem;padding:.75rem}.stat .n{font-size:1.5rem;font-weight:600}
.scroll{overflow-x:auto}
table{border-collapse:collapse;width:100%;background:var(--card);font-size:.9rem}
th,td{text-align:left;padding:.4rem .6rem;border-bottom:1px solid var(--line);vertical-align:top;overflow-wrap:anywhere}
th{color:var(--muted);font-weight:600}
details{margin:.4rem 0}summary{cursor:pointer;font-weight:600}
.tree details,.tree ul{margin-left:1rem}.tree ul{padding-left:0;list-style:none}.tree summary{font-weight:400}
.badge{background:var(--line);border-radius:1rem;padding:0 .5rem;font-size:.8rem}
.sev.error,.failed{color:var(--err)}.sev.warning,.partial{color:var(--warn)}.sev.info,.done{color:var(--ok)}
code{font-family:ui-monospace,monospace}
"#;
