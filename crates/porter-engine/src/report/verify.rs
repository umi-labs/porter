use super::*;
use crate::store::IssueFilter;
use porter_model::{Document, ItemId, ItemKind, Resolution};
use std::collections::HashMap;

/// Per-item verify statuses and issues, plus the count, URL and reference check rows
/// verify wrote to `raw/verify/checks.json`.
pub fn render_verify(project: &Project) -> Result<String> {
    let store = project.store();
    let rows = store.item_rows()?;
    let status: HashMap<ItemId, ItemStatus> = store.statuses(Stage::Verify)?.into_iter().collect();
    let titles: HashMap<ItemId, String> = store
        .items::<Document>(ItemKind::Document)?
        .into_iter()
        .map(|(id, _, d)| (id, format!("{} ({})", d.title, d.path)))
        .collect();
    let issues = store.issues(&IssueFilter {
        stage: Some(Stage::Verify),
        ..Default::default()
    })?;
    let runs = store.runs(Stage::Verify)?;
    let checks = crate::verify::read_checks(project)?.unwrap_or_default();
    let bad_urls: Vec<_> = checks.urls.iter().filter(|u| !u.ok).collect();

    let mut per_kind: BTreeMap<&str, BTreeMap<&'static str, u64>> = BTreeMap::new();
    let mut failures: Vec<(String, &str, &str)> = Vec::new();
    for (id, kind, _) in &rows {
        let Some(s) = status.get(id) else { continue };
        *per_kind.entry(kind).or_default().entry(tag(s)).or_default() += 1;
        let why = match s {
            ItemStatus::Failed { error } => error,
            ItemStatus::Partial { reason } => reason,
            _ => continue,
        };
        failures.push((
            titles.get(id).cloned().unwrap_or_else(|| id.to_string()),
            kind,
            why,
        ));
    }
    let passed: u64 = per_kind.values().flat_map(|m| m.get("done")).sum();
    let failed: u64 = per_kind.values().flat_map(|m| m.get("failed")).sum();

    let body = html! {
        div.stats { (stat("Checks passed", passed)) (stat("Checks failed", failed)) (stat("Open issues", issues.iter().filter(|i| i.resolution == Resolution::Open).count())) }
        h2 { "Checks per type" }
        @if per_kind.is_empty() { p.muted { "No verify results yet." } } @else {
            div.scroll { table {
                thead { tr { th { "Type" } th { "Results" } } }
                tbody { @for (k, m) in &per_kind { tr { td { (k) } td { (counts(m)) } } } }
            } }
        }
        h2 { "Counts" }
        @if checks.counts.is_empty() { p.muted { "No counts checked." } } @else {
            div.scroll { table {
                thead { tr { th { "Collection" } th { "Loaded" } th { "Migrated docs in destination" } th { "All docs (incl. pre-existing)" } th { "Result" } } }
                tbody { @for c in &checks.counts { tr {
                    td { (c.collection) } td { (c.expected) } td { (c.actual) } td.muted { (c.total) }
                    @if c.expected == c.actual { td.done { "match" } } @else { td.failed { "mismatch" } }
                } } }
            } }
        }
        h2 { "References" }
        p.muted { (checks.refs_checked) " references checked (parents, relations, links, media); "
            (checks.refs.len()) " problems." }
        @if !checks.refs.is_empty() {
            div.scroll { table {
                thead { tr { th { "Document" } th { "Kind" } th { "Severity" } th { "Problem" } } }
                tbody { @for r in &checks.refs { tr {
                    td { (r.item) } td { (wire(&r.kind)) }
                    td { span class=(format!("sev {}", wire(&r.severity))) { (wire(&r.severity)) } }
                    td { (r.message) }
                } } }
            } }
        }
        h2 { "URL checks" }
        p.muted { (checks.urls.len()) " old URLs checked, " (bad_urls.len()) " failed." }
        @if !checks.urls.is_empty() {
            div.scroll { table {
                thead { tr { th { "Old URL" } th { "Result" } th { "Requests" } } }
                tbody { @for u in bad_urls.iter().copied().chain(checks.urls.iter().filter(|u| u.ok)) { tr {
                    td { (u.url) }
                    @match &u.error { Some(e) => td.failed { (e) }, None => td.done { "ok" } }
                    td { @for c in &u.chain { (c) br; } }
                } } }
            } }
        }
        h2 { "Failures" }
        @if failures.is_empty() { p.muted { "None." } } @else {
            div.scroll { table {
                thead { tr { th { "Item" } th { "Type" } th { "Reason" } } }
                tbody { @for (t, k, w) in &failures { tr { td { (t) } td { (k) } td { (w) } } } }
            } }
        }
        (issues_section(&issues))
        (runs_table(&runs))
    };
    Ok(page(
        project,
        "Verify report",
        runs.iter().rev().find(|r| r.ended_at.is_some()),
        body,
    ))
}
