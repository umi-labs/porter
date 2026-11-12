use porter_engine::config::{ConnectorConfig, ProjectConfig};
use porter_engine::connectors::RawItem;
use porter_engine::events::EventSink;
use porter_engine::project::Project;
use porter_engine::report::read_coverage;
use porter_engine::sources::wordpress::analyse::{MergeReport, merge};
use porter_engine::stage::run_stage;
use porter_engine::store::IssueFilter;
use porter_model::{IssueKind, ItemId, ItemKind, Route, RouteTarget, SourceRef, Stage};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct Fx {
    _d: tempfile::TempDir,
    p: Arc<Project>,
}

fn fx() -> Fx {
    let d = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(d.path(), "t", "wordpress").unwrap());
    Fx { _d: d, p }
}

impl Fx {
    fn post(&self, id: u32, label: &str, status: &str, url: &str) {
        let src = SourceRef {
            connector: "wxr".into(),
            source_id: id.to_string(),
            url: Some(url.into()),
        };
        let row = RawItem {
            label: label.into(),
            title: None,
            status: Some(status.into()),
            url: Some(url.into()),
            raw_path: None,
        };
        let iid = ItemId::new(ItemKind::Document, &src);
        self.p
            .store()
            .put_item(ItemKind::Document, iid, &src, &row)
            .unwrap();
    }

    fn route(&self, url: &str, status: u16) {
        let key = porter_engine::url::normalise_url(url, None).unwrap().0;
        let src = SourceRef {
            connector: "crawl".into(),
            source_id: key,
            url: Some(url.into()),
        };
        let r = Route {
            url: url.into(),
            target: RouteTarget::None,
            http_status: Some(status),
            title: None,
        };
        let iid = ItemId::new(ItemKind::Route, &src);
        self.p
            .store()
            .put_item(ItemKind::Route, iid, &src, &r)
            .unwrap();
    }

    fn raw(&self, rel: &str, body: &str) {
        let p = self.p.dir().join("raw").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    async fn merge(&self, cfg: ProjectConfig) -> MergeReport {
        let out = Arc::new(std::sync::Mutex::new(MergeReport::default()));
        let o = out.clone();
        run_stage(
            self.p.clone(),
            Arc::new(cfg),
            EventSink::new(64),
            CancellationToken::new(),
            Stage::Analyse,
            |ctx| async move {
                *o.lock().unwrap() = merge(&ctx)?;
                Ok(())
            },
        )
        .await
        .unwrap();
        std::mem::take(&mut *out.lock().unwrap())
    }

    fn issues(&self, kind: IssueKind) -> Vec<String> {
        self.p
            .store()
            .issues(&IssueFilter {
                kind: Some(kind),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|i| i.message)
            .collect()
    }
}

fn cfg() -> ProjectConfig {
    ProjectConfig {
        connectors: vec![
            ConnectorConfig::Wxr {
                path: "x.xml".into(),
            },
            ConnectorConfig::Crawl {
                url: "https://example.com".into(),
                concurrency: 1,
                delay_ms: 0,
                include: vec![],
                exclude: vec![],
                max_pages: None,
                keep_query: vec![],
                max_media_bytes: 0,
            },
        ],
        ..Default::default()
    }
}

#[tokio::test]
async fn merge_reports_unreachable_and_orphans() {
    let f = fx();
    f.post(1, "page", "publish", "https://example.com/a/");
    f.post(2, "page", "publish", "https://www.example.com/b");
    f.post(3, "page", "publish", "https://example.com/gone/");
    f.post(4, "page", "draft", "https://example.com/draft/");
    f.post(5, "attachment", "publish", "https://example.com/a/img/");
    f.route("https://example.com/a/", 200);
    f.route("https://example.com/b/", 200);
    f.route("https://example.com/orphan/", 200);
    f.route("https://example.com/missing/", 404);
    f.route("https://example.com/", 200);
    let r = f.merge(cfg()).await;
    assert_eq!(r.only_in_source, vec!["https://example.com/gone/"]);
    assert_eq!(r.only_live, vec!["https://example.com/orphan/"]);
    assert_eq!(r.matched, 2);
    let c = read_coverage(&f.p).unwrap().unwrap();
    let wxr = c.connectors.iter().find(|c| c.name == "wxr").unwrap();
    assert_eq!(wxr.contributed["page"], 4);
    assert!(c.connectors.iter().any(|c| c.name == "crawl"));
    assert!(f.p.dir().join("raw/analysis/merge.json").exists());
    // No wp-db or authenticated REST: forms are out of reach.
    assert!(
        f.issues(IssueKind::AccessGap)
            .iter()
            .any(|m| m.contains("form definitions"))
    );
}

#[tokio::test]
async fn merge_by_body_class_id() {
    let f = fx();
    f.post(7, "page", "publish", "https://example.com/new-slug/");
    f.route("https://example.com/old-slug/", 200);
    f.raw(
        "crawl/pages/aaaa.json",
        r#"{"url":"https://example.com/old-slug/","status":200,"body_classes":["page","page-id-7"]}"#,
    );
    let r = f.merge(cfg()).await;
    assert_eq!(r.matched, 1);
    assert!(
        r.only_in_source.is_empty() && r.only_live.is_empty(),
        "{r:?}"
    );
}

#[tokio::test]
async fn elementor_detected_raises_converter_missing() {
    let f = fx();
    f.raw(
        "crawl/detect.json",
        r#"{"platform":"wordpress","builders":["elementor"],"plugins":["smart-slider"]}"#,
    );
    f.merge(cfg()).await;
    let m = f.issues(IssueKind::ConverterMissing);
    assert_eq!(m.len(), 2, "{m:?}");
    assert!(
        m.iter()
            .any(|m| m.contains("Elementor") && m.contains("raw blocks"))
    );
    let c = read_coverage(&f.p).unwrap().unwrap();
    assert_eq!(c.platform.as_deref(), Some("wordpress"));
    assert_eq!(c.builders, vec!["elementor"]);
}

/// Analyse re-run after normalise: the normalised Documents in the same table are not
/// connector rows and must not stop it.
#[tokio::test]
async fn analyse_after_normalise_skips_documents() {
    let f = fx();
    f.post(7, "page", "publish", "https://example.com/a/");
    let src = SourceRef {
        connector: "wordpress".into(),
        source_id: "post:7".into(),
        url: None,
    };
    let doc = serde_json::json!({ "id": "x", "type": "page", "fields": {} });
    f.p.store()
        .put_item(
            ItemKind::Document,
            ItemId::new(ItemKind::Document, &src),
            &src,
            &doc,
        )
        .unwrap();
    f.route("https://example.com/a/", 200);
    let r = f.merge(cfg()).await;
    assert_eq!(r.matched, 1, "{r:?}");
}
