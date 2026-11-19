use porter_engine::config::ConnectorConfig;
use porter_engine::connectors::RawItem;
use porter_engine::events::{EventSink, exit_code};
use porter_engine::project::Project;
use porter_engine::sources::wordpress::wxr::{
    WxrConnector, WxrItem, parse_wxr, parse_wxr_with_cap,
};
use porter_engine::stage::{Connector, run_stage};
use porter_model::{ItemId, ItemKind, ItemStatus, SourceRef, Stage};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wxr")
        .join(name)
}

fn parse_all(
    name: &str,
) -> (
    Vec<Result<WxrItem, String>>,
    porter_engine::sources::wordpress::wxr::WxrHeader,
) {
    let mut out = vec![];
    let h = parse_wxr(
        BufReader::new(std::fs::File::open(fixture(name)).unwrap()),
        |r| out.push(r.map_err(|e| e.to_string())),
    )
    .unwrap();
    (out, h)
}

async fn run(
    name: &str,
    stage: Stage,
) -> (
    tempfile::TempDir,
    Arc<Project>,
    porter_engine::events::Summary,
) {
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(dir.path(), "t", "wordpress").unwrap());
    let conn = Arc::new(WxrConnector::new(&ConnectorConfig::Wxr {
        path: fixture(name),
    }));
    let s = run_stage(
        p.clone(),
        Arc::new(p.config().clone()),
        EventSink::new(256),
        CancellationToken::new(),
        stage,
        |ctx| async move {
            if stage == Stage::Analyse {
                conn.analyse(&ctx).await
            } else {
                conn.extract(&ctx).await
            }
        },
    )
    .await
    .unwrap();
    (dir, p, s)
}

fn doc_id(post_id: &str) -> ItemId {
    ItemId::new(
        ItemKind::Document,
        &SourceRef {
            connector: "wxr".into(),
            source_id: post_id.into(),
            url: Some(format!("https://example.com/{post_id}/")),
        },
    )
}

#[test]
fn wxr_streams_multiline_and_cdata() {
    let (items, h) = parse_all("tricky.xml");
    assert_eq!(items.len(), 5);
    assert_eq!(items.iter().filter(|i| i.is_ok()).count(), 4);
    let ok: Vec<&WxrItem> = items.iter().filter_map(|i| i.as_ref().ok()).collect();
    assert_eq!(ok[0].content, "Line one\n\nLine two\n<p>para</p>\n");
    assert_eq!(ok[1].content, "a ]]> b <script>if(x[y[0]] > 1){}</script>");
    assert_eq!(
        ok[2].attachment_url.as_deref(),
        Some("https://example.invalid/wp-content/uploads/pic.png")
    );
    assert_eq!(h.title, "Tricky Site");
    assert_eq!(h.base_site_url, "https://example.com");
    assert_eq!(h.authors.len(), 1);
    assert_eq!(h.terms[0].nicename, "news");
}

#[test]
fn wxr_post_type_unwrapped() {
    let (items, _) = parse_all("basic.xml");
    let first = items[0].as_ref().unwrap();
    assert_eq!(first.post_type, "post");
    assert_eq!(first.status, "publish");
    assert_eq!(
        first.categories[0],
        (
            "category".into(),
            "uncategorized".into(),
            "Uncategorized".into()
        )
    );
    assert_eq!(first.meta[1], ("_featured_image".into(), "123".into()));
}

#[tokio::test]
async fn wxr_bad_item_is_failed_not_fatal() {
    let (_d, p, s) = run("tricky.xml", Stage::Extract).await;
    assert_eq!(exit_code(&s), 2);
    let st = p.store().statuses(Stage::Extract).unwrap();
    let failed: Vec<_> = st
        .iter()
        .filter(|(_, s)| matches!(s, ItemStatus::Failed { .. }))
        .collect();
    assert_eq!(failed.len(), 1, "{st:?}");
    for pid in ["10", "11", "14"] {
        assert_eq!(
            p.store().status(Stage::Extract, doc_id(pid)).unwrap(),
            Some(ItemStatus::Done),
            "{pid}"
        );
    }
    let raw = p.raw_dir("wxr");
    assert!(raw.join("items/10.json").exists());
    assert!(raw.join("authors.json").exists());
    assert!(raw.join("terms.json").exists());
    assert!(raw.join("site.json").exists());
}

#[tokio::test]
async fn wxr_attachment_download_failure_is_partial() {
    // example.invalid never resolves: the item is Partial with a missing-media warning.
    let (_d, p, s) = run("tricky.xml", Stage::Extract).await;
    let st = p
        .store()
        .status(Stage::Extract, doc_id("12"))
        .unwrap()
        .unwrap();
    assert!(matches!(st, ItemStatus::Partial { .. }), "{st:?}");
    assert_eq!(s.warnings, 1);
}

#[tokio::test]
async fn wxr_analyse_marks_items_done() {
    let (_d, p, s) = run("basic.xml", Stage::Analyse).await;
    assert_eq!(exit_code(&s), 0);
    assert_eq!(p.store().statuses(Stage::Analyse).unwrap().len(), 2 + 1 + 2);
}

#[test]
#[ignore = "200k items; nightly"]
fn wxr_memory_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.xml");
    let mut w = BufWriter::new(std::fs::File::create(&path).unwrap());
    write!(
        w,
        r#"<rss xmlns:wp="w" xmlns:content="c"><channel><title>Big</title>"#
    )
    .unwrap();
    for i in 1..=200_000u32 {
        writeln!(w, "<item><title>T{i}</title><wp:post_id>{i}</wp:post_id><wp:post_type><![CDATA[post]]></wp:post_type><content:encoded><![CDATA[{}]]></content:encoded></item>", "x".repeat(200)).unwrap();
    }
    write!(w, "</channel></rss>").unwrap();
    drop(w);
    let mut n = 0;
    parse_wxr(BufReader::new(std::fs::File::open(&path).unwrap()), |r| {
        assert!(r.is_ok());
        n += 1;
    })
    .unwrap();
    assert_eq!(n, 200_000);
}

#[test]
fn wxr_oversized_item_failed() {
    let big = "y".repeat(5000);
    let xml = format!(
        r#"<rss xmlns:wp="w" xmlns:content="c"><channel><title>S</title>
<item><title>Big</title><wp:post_id>1</wp:post_id><content:encoded><![CDATA[{big}]]></content:encoded></item>
<item><title>Small</title><wp:post_id>2</wp:post_id><content:encoded><![CDATA[ok]]></content:encoded></item>
</channel></rss>"#
    );
    let mut out = vec![];
    parse_wxr_with_cap(xml.as_bytes(), 1000, |r| {
        out.push(r.map_err(|e| e.to_string()))
    })
    .unwrap();
    assert_eq!(out.len(), 2);
    let e = out[0].as_ref().unwrap_err();
    assert!(e.contains("item larger than"), "{e}");
    assert_eq!(out[1].as_ref().unwrap().content, "ok");
}

#[tokio::test]
async fn wxr_writes_raw_item_rows() {
    let (_d, p, _s) = run("tricky.xml", Stage::Extract).await;
    let rows = p.store().items::<RawItem>(ItemKind::Document).unwrap();
    let labels: Vec<(String, String)> = rows
        .iter()
        .map(|(_, src, r)| (src.source_id.clone(), r.label.clone()))
        .collect();
    for (id, label) in [
        ("10", "post"),
        ("11", "post"),
        ("12", "attachment"),
        ("14", "page"),
    ] {
        assert!(labels.contains(&(id.into(), label.into())), "{labels:?}");
    }
    let r10 = rows.iter().find(|(_, s, _)| s.source_id == "10").unwrap();
    assert_eq!(r10.2.raw_path.as_deref(), Some("items/10.json"));
    assert_eq!(
        p.store().items::<RawItem>(ItemKind::Author).unwrap()[0]
            .2
            .label,
        "author"
    );
    assert_eq!(
        p.store().items::<RawItem>(ItemKind::Term).unwrap()[0]
            .2
            .label,
        "genre"
    );
}

#[test]
fn wxr_oversized_attributes_failed() {
    let flood =
        r#"<x a="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"/>"#.repeat(100);
    let xml = format!(
        r#"<rss xmlns:wp="w"><channel><title>S</title>
<item><wp:post_id>1</wp:post_id>{flood}<y><z b="{}"></z></y></item>
<item><wp:post_id>2</wp:post_id></item>
</channel></rss>"#,
        "b".repeat(2000)
    );
    let mut out = vec![];
    parse_wxr_with_cap(xml.as_bytes(), 1000, |r| {
        out.push(r.map_err(|e| e.to_string()))
    })
    .unwrap();
    assert_eq!(out.len(), 2);
    assert!(out[0].as_ref().unwrap_err().contains("item larger than"));
    assert_eq!(out[1].as_ref().unwrap().post_id, 2);
}
