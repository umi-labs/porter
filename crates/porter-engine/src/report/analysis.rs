use super::*;
use crate::store::IssueFilter;
use porter_model::{Document, Form, ItemId, ItemKind, Media, Menu, Redirect, Route};
use std::collections::HashMap;

#[derive(Default)]
struct Node {
    count: usize,
    kids: BTreeMap<String, Node>,
}

/// Path part of a URL, without scheme/host, query or fragment.
fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r.find('/').map_or("/", |i| &r[i..]));
    rest.split(['?', '#']).next().unwrap_or("/")
}

fn tree(paths: impl Iterator<Item = String>) -> Node {
    let mut root = Node::default();
    for p in paths {
        let mut n = &mut root;
        n.count += 1;
        for seg in path_of(&p).split('/').filter(|s| !s.is_empty()) {
            n = n.kids.entry(seg.to_string()).or_default();
            n.count += 1;
        }
    }
    root
}

fn render_node(n: &Node) -> Markup {
    html! { @for (name, k) in &n.kids {
        @if k.kids.is_empty() { ul { li { code { "/" (name) } } } }
        @else { details { summary { code { "/" (name) } " " span.badge { (k.count) } } (render_node(k)) } }
    } }
}

pub fn render_analysis(project: &Project) -> Result<String> {
    let store = project.store();
    let docs: Vec<(ItemId, _, Document)> = store.items(ItemKind::Document)?;
    let routes: Vec<(ItemId, _, Route)> = store.items(ItemKind::Route)?;
    let redirects: Vec<(ItemId, _, Redirect)> = store.items(ItemKind::Redirect)?;
    let media: Vec<(ItemId, _, Media)> = store.items(ItemKind::Media)?;
    let forms: Vec<(ItemId, _, Form)> = store.items(ItemKind::Form)?;
    let menus: Vec<(ItemId, _, Menu)> = store.items(ItemKind::Menu)?;
    let status: HashMap<ItemId, ItemStatus> = store.statuses(Stage::Analyse)?.into_iter().collect();
    let issues = store.issues(&IssueFilter { stage: Some(Stage::Analyse), ..Default::default() })?;
    let runs = store.runs(Stage::Analyse)?;
    let coverage = read_coverage(project)?;

    let mut types: BTreeMap<&str, BTreeMap<&'static str, u64>> = BTreeMap::new();
    let mut locales: BTreeMap<String, u64> = BTreeMap::new();
    for (id, _, d) in &docs {
        let t = status.get(id).map_or("unprocessed", tag);
        *types.entry(&d.doc_type).or_default().entry(t).or_default() += 1;
        *locales.entry(d.locale.clone().unwrap_or_else(|| "(none)".into())).or_default() += 1;
    }
    // Total bytes of media files that are on disk; a missing file counts as 0.
    let bytes: u64 = media
        .iter()
        .filter_map(|(id, ..)| store.media_path(*id).ok())
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    let urls = if routes.is_empty() {
        tree(docs.iter().map(|(_, _, d)| d.path.clone()))
    } else {
        tree(routes.iter().map(|(_, _, r)| r.url.clone()))
    };

    let body = html! {
        div.stats {
            (stat("Documents", docs.len())) (stat("URLs", urls.count)) (stat("Media files", media.len()))
            (stat("Media size", human_bytes(bytes))) (stat("Redirects", redirects.len()))
            (stat("Forms", forms.len())) (stat("Menus", menus.len()))
        }
        h2 { "Platform" }
        @match &coverage {
            Some(c) => {
                p { "Platform: " strong { (c.platform.clone().unwrap_or_else(|| "unknown".into())) } }
                p { "Builders: " (if c.builders.is_empty() { "none detected".to_string() } else { c.builders.join(", ") }) }
                p { "Plugins: " (if c.plugins.is_empty() { "none detected".to_string() } else { c.plugins.join(", ") }) }
            }
            None => p.muted { "Not recorded." }
        }
        h2 { "Content types" }
        @if types.is_empty() { p.muted { "No documents." } } @else {
            div.scroll { table {
                thead { tr { th { "Type" } th { "Total" } th { "By status" } } }
                tbody { @for (t, m) in &types { tr { td { (t) } td { (m.values().sum::<u64>()) } td { (counts(m)) } } } }
            } }
        }
        h2 { "Documents" }
        @if !docs.is_empty() { details { summary { "List (first 500 of " (docs.len()) ")" }
            div.scroll { table {
                thead { tr { th { "Title" } th { "Type" } th { "Path" } th { "Status" } } }
                tbody { @for (id, _, d) in docs.iter().take(500) { tr {
                    td { (d.title) } td { (d.doc_type) } td { (d.path) } td { (status.get(id).map_or("unprocessed", tag)) }
                } } }
            } }
        } }
        h2 { "URL tree" }
        @if urls.kids.is_empty() { p.muted { "No URLs." } } @else { div.tree { (render_node(&urls)) } }
        h2 { "Languages" }
        @if locales.is_empty() { p.muted { "None." } } @else {
            p { @for (l, n) in &locales { span.badge { (l) ": " (n) } " " } }
        }
        h2 { "Menus" }
        @if menus.is_empty() { p.muted { "None." } } @else {
            ul { @for (_, _, m) in &menus { li { (m.name) " (" (m.items.len()) " top-level items)" } } }
        }
        h2 { "Forms" }
        @if forms.is_empty() { p.muted { "None." } } @else {
            ul { @for (_, _, f) in &forms { li { (f.title) " (" (f.fields.len()) " fields)" @if f.retired { " - retired" } } } }
        }
        h2 { "Redirects" }
        @if redirects.is_empty() { p.muted { "None." } } @else {
            div.scroll { table {
                thead { tr { th { "From" } th { "To" } th { "Code" } th { "Origin" } } }
                tbody { @for (_, _, r) in &redirects { tr { td { (r.from) } td { (r.to) } td { (r.code) } td { (wire(&r.origin)) } } } }
            } }
        }
        h2 { "Coverage" }
        @match &coverage {
            None => p.muted { "No coverage recorded." },
            Some(c) => div.scroll { table {
                thead { tr { th { "Connector" } th { "Contributed" } th { "Access gaps" } } }
                tbody { @for k in &c.connectors { tr {
                    td { (k.name) }
                    td { (if k.contributed.is_empty() { "-".to_string() } else {
                        k.contributed.iter().map(|(a, b)| format!("{b} {a}")).collect::<Vec<_>>().join(", ") }) }
                    td { @if k.gaps.is_empty() { "none" } @else { ul { @for g in &k.gaps { li { (g) } } } } }
                } } }
            } }
        }
        (issues_section(&issues))
        (runs_table(&runs))
    };
    Ok(page(project, "Analysis report", runs.iter().rev().find(|r| r.ended_at.is_some()), body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Summary;
    use porter_model::{DocStatus, IssueKind, Resolution, Seo, Severity, SourceRef};

    pub(crate) fn doc(i: &str, title: &str, path: &str) -> (Document, SourceRef) {
        let src = SourceRef { connector: "c".into(), source_id: i.into(), url: None };
        (
            Document {
                id: ItemId::new(ItemKind::Document, &src),
                doc_type: "page".into(),
                status: DocStatus::Published,
                locale: Some("en".into()),
                translations: vec![],
                slug: i.into(),
                path: path.into(),
                parent: None,
                title: title.into(),
                fields: Default::default(),
                seo: Seo::default(),
                created: None,
                updated: None,
                author: None,
                original: serde_json::json!({}),
                source: src.clone(),
            },
            src,
        )
    }

    pub(crate) fn seeded(titles: [&str; 3]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "demo <b>", "x").unwrap();
        let run = p.store().begin_run(Stage::Analyse).unwrap();
        for (n, t) in titles.iter().enumerate() {
            let (d, src) = doc(&n.to_string(), t, &format!("/section/page-{n}"));
            p.store().put_item(ItemKind::Document, d.id, &src, &d).unwrap();
            p.store().set_status(run, Stage::Analyse, d.id, &ItemStatus::Done).unwrap();
        }
        p.store()
            .open_issue(
                run,
                porter_model::Issue {
                    id: uuid::Uuid::now_v7(),
                    kind: IssueKind::AccessGap,
                    severity: Severity::Warning,
                    item: None,
                    stage: Stage::Analyse,
                    message: "no API access to private posts".into(),
                    details: serde_json::json!({}),
                    resolution: Resolution::Open,
                },
            )
            .unwrap();
        p.store().end_run(run, &Summary::default()).unwrap();
        (dir, p)
    }

    #[test]
    fn analysis_report_self_contained() {
        let (_d, p) = seeded(["A", "B", "C"]);
        write_coverage(
            &p,
            &Coverage {
                platform: Some("examplecms".into()),
                connectors: vec![ConnectorCoverage {
                    name: "api".into(),
                    contributed: [("documents".to_string(), 3)].into(),
                    gaps: vec!["drafts need auth".into()],
                }],
                ..Coverage::default()
            },
        )
        .unwrap();
        let html = render_analysis(&p).unwrap();
        assert!(!html.contains("src=\"http") && !html.contains("href=\"http"));
        assert!(!html.contains("<script"));
        assert!(html.contains("no API access to private posts"));
        assert!(html.contains("Coverage") && html.contains("drafts need auth"));
        assert!(html.contains("porter 2.0.0") && html.contains("demo &lt;b&gt;"));
        assert!(html.contains("<details"));
        assert_eq!(write_all(&p).unwrap().len(), 1);
    }

    #[test]
    fn no_coverage_says_so() {
        let (_d, p) = seeded(["A", "B", "C"]);
        assert!(render_analysis(&p).unwrap().contains("No coverage recorded"));
    }

    #[test]
    fn report_escapes_content() {
        let (_d, p) = seeded(["<script>alert(1)</script>", "B", "C"]);
        let html = render_analysis(&p).unwrap();
        assert!(!html.contains("<script>alert(1)"));
        assert!(!html.contains("<script"));
    }
}
