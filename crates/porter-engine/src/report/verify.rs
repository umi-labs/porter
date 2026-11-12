use super::*;
use crate::store::IssueFilter;
use porter_model::{Document, ItemId, ItemKind};
use std::collections::HashMap;

// ponytail: the verify stage doesn't yet persist per-URL / link / relation check
// rows, so this report is built from per-item verify statuses and verify-stage
// issues. Add dedicated check tables when the verify stage lands.
pub fn render_verify(project: &Project) -> Result<String> {
    let store = project.store();
    let rows = store.item_rows()?;
    let status: HashMap<ItemId, ItemStatus> = store.statuses(Stage::Verify)?.into_iter().collect();
    let titles: HashMap<ItemId, String> = store
        .items::<Document>(ItemKind::Document)?
        .into_iter()
        .map(|(id, _, d)| (id, format!("{} ({})", d.title, d.path)))
        .collect();
    let issues = store.issues(&IssueFilter { stage: Some(Stage::Verify), ..Default::default() })?;
    let runs = store.runs(Stage::Verify)?;

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
        failures.push((titles.get(id).cloned().unwrap_or_else(|| id.to_string()), kind, why));
    }
    let passed: u64 = per_kind.values().flat_map(|m| m.get("done")).sum();
    let failed: u64 = per_kind.values().flat_map(|m| m.get("failed")).sum();

    let body = html! {
        div.stats { (stat("Checks passed", passed)) (stat("Checks failed", failed)) (stat("Open issues", issues.len())) }
        h2 { "Checks per type" }
        @if per_kind.is_empty() { p.muted { "No verify results yet." } } @else {
            div.scroll { table {
                thead { tr { th { "Type" } th { "Results" } } }
                tbody { @for (k, m) in &per_kind { tr { td { (k) } td { (counts(m)) } } } }
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
    Ok(page(project, "Verify report", runs.iter().rev().find(|r| r.ended_at.is_some()), body))
}
