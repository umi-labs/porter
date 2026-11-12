use super::*;
use crate::store::IssueFilter;
use porter_model::ItemId;
use std::collections::HashMap;

pub fn render_extract(project: &Project) -> Result<String> {
    let store = project.store();
    let rows = store.item_rows()?;
    let status: HashMap<ItemId, ItemStatus> = store.statuses(Stage::Extract)?.into_iter().collect();
    let issues = store.issues(&IssueFilter { stage: Some(Stage::Extract), ..Default::default() })?;
    let runs = store.runs(Stage::Extract)?;

    let mut per_conn: BTreeMap<&str, BTreeMap<&'static str, u64>> = BTreeMap::new();
    let (mut media_ok, mut media_failed) = (0u64, 0u64);
    let mut failures: Vec<(&str, &str, &str)> = Vec::new();
    for (id, kind, conn) in &rows {
        let Some(s) = status.get(id) else { continue };
        *per_conn.entry(conn).or_default().entry(tag(s)).or_default() += 1;
        if kind == "media" {
            match s {
                ItemStatus::Done => media_ok += 1,
                ItemStatus::Failed { .. } => media_failed += 1,
                _ => {}
            }
        }
        if let ItemStatus::Failed { error } = s {
            failures.push((conn, kind, error));
        }
    }

    let body = html! {
        div.stats {
            (stat("Items fetched", per_conn.values().flat_map(|m| m.get("done")).sum::<u64>()))
            (stat("Media fetched", media_ok)) (stat("Media failed", media_failed))
            (stat("Failed items", failures.len()))
        }
        h2 { "Fetched per connector" }
        @if per_conn.is_empty() { p.muted { "Nothing extracted yet." } } @else {
            div.scroll { table {
                thead { tr { th { "Connector" } th { "Items by status" } } }
                tbody { @for (c, m) in &per_conn { tr { td { (c) } td { (counts(m)) } } } }
            } }
        }
        @if !failures.is_empty() {
            h2 { "Failures" }
            div.scroll { table {
                thead { tr { th { "Connector" } th { "Kind" } th { "Error" } } }
                tbody { @for (c, k, e) in &failures { tr { td { (c) } td { (k) } td { (e) } } } }
            } }
        }
        (issues_section(&issues))
        (runs_table(&runs))
    };
    Ok(page(project, "Extract report", runs.iter().rev().find(|r| r.ended_at.is_some()), body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Summary;
    use porter_model::{ItemKind, SourceRef};

    #[test]
    fn extract_counts_and_escapes_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "demo", "x").unwrap();
        let run = p.store().begin_run(Stage::Extract).unwrap();
        for (i, st) in [("1", ItemStatus::Done), ("2", ItemStatus::Failed { error: "<img src=x onerror=1>".into() })] {
            let src = SourceRef { connector: "api".into(), source_id: i.into(), url: None };
            let id = ItemId::new(ItemKind::Media, &src);
            p.store().put_item(ItemKind::Media, id, &src, &serde_json::json!({})).unwrap();
            p.store().set_status(run, Stage::Extract, id, &st).unwrap();
        }
        p.store().end_run(run, &Summary { done: 1, failed: 1, ..Summary::default() }).unwrap();
        let html = render_extract(&p).unwrap();
        assert!(html.contains("1 done, 1 failed") && html.contains("Run history"));
        assert!(!html.contains("<img") && html.contains("&lt;img"));
    }
}
