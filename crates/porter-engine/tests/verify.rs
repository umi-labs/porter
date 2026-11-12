//! Verify: counts, structure (parents, links, media) and old URLs, checked against the
//! source model. Wiremock stands in for both the Payload destination and the new site;
//! a live round trip against the test app is gated by PORTER_PAYLOAD_TEST_URL +
//! PORTER_PAYLOAD_API_KEY.
use indexmap::IndexMap;
use porter_engine::config::ProjectConfig;
use porter_engine::destinations::payload::PayloadDestination;
use porter_engine::events::{EventSink, Summary, exit_code};
use porter_engine::mapping::{FieldMap, TypeMap};
use porter_engine::project::Project;
use porter_engine::stage::run_stage;
use porter_engine::store::IssueFilter;
use porter_model::{
    DocStatus, Document, FieldValue, Inline, Issue, IssueKind, ItemId, ItemKind, ItemStatus,
    LinkTarget, Marks, Redirect, RedirectOrigin, RichText, Route, RouteTarget, RtBlock, Seo,
    Severity, SourceRef, Stage,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const KEY_ENV: &str = "PORTER_TEST_VERIFY_KEY";
const OLD: &str = "https://old.example.test";

// ---- a read-only fake of Payload's REST API ----

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<HashMap<(String, String), Value>>>);

impl Fake {
    fn put(&self, coll: &str, doc: Value) {
        let id = doc["id"].to_string();
        self.0.lock().unwrap().insert((coll.into(), id), doc);
    }
}

impl Respond for Fake {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let docs = self.0.lock().unwrap();
        let p = r.url.path().trim_start_matches("/api/").to_string();
        match p.split('/').collect::<Vec<_>>().as_slice() {
            [coll] => {
                let n = docs.keys().filter(|(c, _)| c == coll).count();
                ResponseTemplate::new(200).set_body_json(json!({"docs": [], "totalDocs": n}))
            }
            [coll, id] => match docs.get(&(coll.to_string(), id.to_string())) {
                Some(d) => ResponseTemplate::new(200).set_body_json(d),
                None => ResponseTemplate::new(404),
            },
            _ => ResponseTemplate::new(404),
        }
    }
}

async fn payload(fake: &Fake) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(any())
        .respond_with(fake.clone())
        .mount(&s)
        .await;
    s
}

// ---- fixtures ----

fn src(sid: &str) -> SourceRef {
    SourceRef {
        connector: "wordpress".into(),
        source_id: sid.into(),
        url: Some(format!("{OLD}/{sid}/")),
    }
}

fn did(sid: &str) -> ItemId {
    ItemId::new(ItemKind::Document, &src(sid))
}

fn rich(blocks: Vec<RtBlock>) -> FieldValue {
    FieldValue::RichText(RichText { blocks })
}

fn para(inlines: Vec<Inline>) -> RtBlock {
    RtBlock::Paragraph {
        inlines,
        align: Default::default(),
    }
}

fn txt(s: &str) -> Inline {
    Inline::Text {
        text: s.into(),
        marks: Marks::default(),
    }
}

fn doc(sid: &str, slug: &str, parent: Option<&str>) -> Document {
    let mut fields = IndexMap::new();
    fields.insert("content".to_string(), rich(vec![para(vec![txt(slug)])]));
    Document {
        id: did(sid),
        doc_type: "page".into(),
        status: DocStatus::Published,
        locale: None,
        translations: vec![],
        slug: slug.into(),
        path: format!("/{slug}/"),
        parent: parent.map(did),
        title: slug.to_uppercase(),
        fields,
        seo: Seo::default(),
        created: None,
        updated: None,
        author: None,
        original: Value::Null,
        source: src(sid),
    }
}

/// Lexical as the loaded doc holds it.
fn lexical(children: Vec<Value>) -> Value {
    json!({"root": {"type": "root", "children": [{"type": "paragraph", "children": children}]}})
}

struct Fx {
    _d: tempfile::TempDir,
    p: Arc<Project>,
    cfg: ProjectConfig,
}

fn fx(dest_url: &str) -> Fx {
    // SAFETY: every test sets the same value, so concurrent writers can't disagree.
    unsafe { std::env::set_var(KEY_ENV, "test-key") };
    let d = tempfile::tempdir().unwrap();
    let p = Project::init(d.path(), "t", "wordpress").unwrap();
    let mut cfg = p.config().clone();
    cfg.destination.adapter = "payload".into();
    cfg.destination.url = dest_url.into();
    cfg.destination.api_key_env = KEY_ENV.into();
    cfg.mapping.types.insert(
        "page".into(),
        TypeMap {
            to: Some("pages".into()),
            path: Some("{parent.path}/{slug}/".into()),
            fields: [(
                "content".to_string(),
                FieldMap {
                    to: "content".into(),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..TypeMap::default()
        },
    );
    Fx {
        _d: d,
        p: Arc::new(p),
        cfg,
    }
}

impl Fx {
    /// Stores `d` and records it as loaded into `coll/id`.
    fn loaded(&self, d: &Document, coll: &str, id: u64) {
        let s = self.p.store();
        s.put_item(ItemKind::Document, d.id, &d.source, d).unwrap();
        self.landed(d.id, ItemKind::Document, coll, id);
    }

    fn landed(&self, item: ItemId, _kind: ItemKind, coll: &str, id: u64) {
        let s = self.p.store();
        let run = s.begin_run(Stage::Load).unwrap();
        s.set_status(run, Stage::Load, item, &ItemStatus::Done)
            .unwrap();
        s.put_dest_id(item, coll, &json!(id)).unwrap();
    }

    fn route(&self, url: &str, status: Option<u16>, target: RouteTarget) -> ItemId {
        let s = SourceRef {
            connector: "wordpress".into(),
            source_id: format!("route:{url}"),
            url: Some(url.into()),
        };
        let id = ItemId::new(ItemKind::Route, &s);
        let r = Route {
            url: url.into(),
            target,
            http_status: status,
            title: None,
        };
        self.p
            .store()
            .put_item(ItemKind::Route, id, &s, &r)
            .unwrap();
        id
    }

    fn redirect(&self, from: &str, to: &str) -> ItemId {
        let s = SourceRef {
            connector: "wordpress".into(),
            source_id: format!("redirect:{from}"),
            url: None,
        };
        let id = ItemId::new(ItemKind::Redirect, &s);
        let r = Redirect {
            from: from.into(),
            to: to.into(),
            code: 301,
            origin: RedirectOrigin::Plugin,
        };
        self.p
            .store()
            .put_item(ItemKind::Redirect, id, &s, &r)
            .unwrap();
        id
    }

    async fn verify(&self) -> Summary {
        let dest = PayloadDestination::new(&self.cfg.destination);
        run_stage(
            self.p.clone(),
            Arc::new(self.cfg.clone()),
            EventSink::new(1024),
            CancellationToken::new(),
            Stage::Verify,
            |ctx| async move { porter_engine::verify::run(&ctx, &dest).await },
        )
        .await
        .unwrap()
    }

    fn status(&self, id: ItemId) -> Option<ItemStatus> {
        self.p.store().status(Stage::Verify, id).unwrap()
    }

    fn issues(&self, kind: IssueKind) -> Vec<Issue> {
        self.p
            .store()
            .issues(&IssueFilter {
                stage: Some(Stage::Verify),
                kind: Some(kind),
                ..Default::default()
            })
            .unwrap()
    }
}

fn failed(s: Option<ItemStatus>) -> String {
    match s {
        Some(ItemStatus::Failed { error }) => error,
        other => panic!("expected Failed, got {other:?}"),
    }
}

// ---- tests ----

#[tokio::test]
async fn verify_count_mismatch() {
    let fake = Fake::default();
    let s = payload(&fake).await;
    let f = fx(&s.uri());
    f.loaded(&doc("1", "a", None), "pages", 1);
    f.loaded(&doc("2", "b", None), "pages", 2);
    // Only one of the two reached the destination.
    fake.put("pages", json!({"id": 1, "title": "A", "parent": null}));

    let sum = f.verify().await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    let counts = f.issues(IssueKind::SchemaMismatch);
    assert_eq!(counts.len(), 1, "{counts:#?}");
    assert_eq!(counts[0].severity, Severity::Error);
    assert_eq!(
        counts[0].details,
        json!({"collection": "pages", "expected": 2, "actual": 1})
    );
    assert_eq!(f.status(did("1")), Some(ItemStatus::Done));
    assert!(failed(f.status(did("2"))).contains("not found"));
}

#[tokio::test]
async fn verify_wrong_parent_detected() {
    let fake = Fake::default();
    let s = payload(&fake).await;
    let f = fx(&s.uri());
    f.loaded(&doc("1", "a", None), "pages", 1);
    f.loaded(&doc("2", "b", Some("1")), "pages", 2);
    f.loaded(&doc("3", "c", None), "pages", 3);
    fake.put("pages", json!({"id": 1, "parent": null}));
    // Wired to the wrong parent: the model says 1.
    fake.put("pages", json!({"id": 2, "parent": 3}));
    fake.put("pages", json!({"id": 3, "parent": null}));

    let sum = f.verify().await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    let ps = f.issues(IssueKind::ParentMissing);
    assert_eq!(ps.len(), 1, "{ps:#?}");
    assert_eq!(ps[0].severity, Severity::Error);
    assert_eq!(ps[0].item, Some(did("2")));
    assert!(ps[0].message.contains("pages/1"), "{}", ps[0].message);
    assert!(ps[0].message.contains("pages/3"), "{}", ps[0].message);
    assert!(failed(f.status(did("2"))).contains("parent"));
    assert_eq!(f.status(did("1")), Some(ItemStatus::Done));
    assert_eq!(f.status(did("3")), Some(ItemStatus::Done));
}

#[tokio::test]
async fn verify_wrong_link_and_missing_media_detected() {
    let fake = Fake::default();
    let s = payload(&fake).await;
    let f = fx(&s.uri());
    let media = ItemId::new(
        ItemKind::Media,
        &SourceRef {
            connector: "media".into(),
            source_id: "m1".into(),
            url: None,
        },
    );
    let mut a = doc("1", "a", None);
    a.fields.insert(
        "content".into(),
        rich(vec![
            para(vec![Inline::Link {
                target: LinkTarget::Doc(did("2")),
                new_tab: false,
                inlines: vec![txt("to b")],
                fragment: None,
            }]),
            RtBlock::Media {
                media,
                caption: None,
                align: Default::default(),
                width: None,
                link: None,
            },
        ]),
    );
    f.loaded(&a, "pages", 1);
    f.loaded(&doc("2", "b", None), "pages", 2);
    f.loaded(&doc("3", "c", None), "pages", 3);
    f.landed(media, ItemKind::Media, "media", 5);
    let link = json!({"type": "link", "fields": {"linkType": "internal",
        "doc": {"relationTo": "pages", "value": 3}}, "children": []});
    let upload = json!({"type": "upload", "relationTo": "media", "value": 5});
    let mut loaded = lexical(vec![link]);
    loaded["root"]["children"]
        .as_array_mut()
        .unwrap()
        .push(upload);
    fake.put("pages", json!({"id": 1, "parent": null, "content": loaded}));
    fake.put("pages", json!({"id": 2, "parent": null}));
    fake.put("pages", json!({"id": 3, "parent": null}));
    // media/5 is gone from the destination.

    let sum = f.verify().await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    let rel: Vec<Issue> = f
        .issues(IssueKind::UnresolvedRelation)
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert_eq!(rel.len(), 1, "{rel:#?}");
    assert!(rel[0].message.contains("pages/2"), "{}", rel[0].message);
    assert!(rel[0].message.contains("pages/3"), "{}", rel[0].message);
    let mm = f.issues(IssueKind::MissingMedia);
    assert_eq!(mm.len(), 1, "{mm:#?}");
    assert!(mm[0].message.contains("media/5"), "{}", mm[0].message);
    failed(f.status(did("1")));
    assert_eq!(f.status(did("2")), Some(ItemStatus::Done));
}

#[tokio::test]
async fn verify_url_one_hop_ok_two_hops_fail() {
    let fake = Fake::default();
    let dest = payload(&fake).await;
    let site = MockServer::start().await;
    let hop = |from: &str, to: &str, code: u16| {
        Mock::given(method("GET"))
            .and(path(from.to_string()))
            .respond_with(ResponseTemplate::new(code).insert_header("Location", to))
    };
    let ok = |p: &str| {
        Mock::given(method("GET"))
            .and(path(p.to_string()))
            .respond_with(ResponseTemplate::new(200))
    };
    ok("/same/").mount(&site).await;
    hop("/a/", "/a-new/", 301).mount(&site).await;
    ok("/a-new/").mount(&site).await;
    hop("/b/", "/b1/", 301).mount(&site).await;
    hop("/b1/", "/b2/", 301).mount(&site).await;
    ok("/b2/").mount(&site).await;
    hop("/c/", "https://elsewhere.example.test/c/", 302)
        .mount(&site)
        .await;
    hop("/old-r/", "/a-new/", 301).mount(&site).await;

    let mut f = fx(&dest.uri());
    f.cfg.verify.site_url = Some(site.uri());
    let same = f.route(&format!("{OLD}/same/"), Some(200), RouteTarget::None);
    let a = f.route(&format!("{OLD}/a/"), Some(200), RouteTarget::None);
    let b = f.route(&format!("{OLD}/b/"), Some(200), RouteTarget::None);
    let c = f.route(&format!("{OLD}/c/"), Some(200), RouteTarget::None);
    let gone = f.route(&format!("{OLD}/gone/"), Some(404), RouteTarget::None);
    let r = f.redirect("/old-r/", "/a-new/");

    let sum = f.verify().await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    assert_eq!(f.status(same), Some(ItemStatus::Done));
    assert_eq!(f.status(a), Some(ItemStatus::Done));
    assert_eq!(f.status(r), Some(ItemStatus::Done));
    let eb = failed(f.status(b));
    assert!(eb.contains("/b2/"), "{eb}");
    let ec = failed(f.status(c));
    assert!(
        ec.contains("redirects off-site to elsewhere.example.test"),
        "{ec}"
    );
    // Only routes that were 2xx on the old site are checked.
    assert_eq!(f.status(gone), None);
    let links = f.issues(IssueKind::UnresolvedLink);
    assert_eq!(links.len(), 2, "{links:#?}");
    assert!(links.iter().all(|i| i.severity == Severity::Error));
    let chain = links.iter().find(|i| i.item == Some(b)).unwrap();
    assert_eq!(chain.details["chain"].as_array().unwrap().len(), 3);
    // Off-site hops are never followed.
    assert!(site.received_requests().await.unwrap().len() <= 8);
}

#[tokio::test]
async fn verify_without_site_url_is_model_check_plus_gap() {
    let fake = Fake::default();
    let s = payload(&fake).await;
    let f = fx(&s.uri());
    let about = doc("1", "about", None);
    let x = doc("2", "x", None);
    let y = doc("3", "y", None);
    for (i, d) in [&about, &x, &y].into_iter().enumerate() {
        f.loaded(d, "pages", i as u64 + 1);
        fake.put("pages", json!({"id": i + 1, "parent": null}));
    }
    let r_about = f.route(
        &format!("{OLD}/about/"),
        Some(200),
        RouteTarget::Document(about.id),
    );
    let r_x = f.route(
        &format!("{OLD}/old-x/"),
        Some(200),
        RouteTarget::Document(x.id),
    );
    let r_y = f.route(
        &format!("{OLD}/old-y/"),
        Some(200),
        RouteTarget::Document(y.id),
    );
    let red = f.redirect("/old-y/", "/y/");
    f.landed(red, ItemKind::Redirect, "redirects", 1);
    fake.put("redirects", json!({"id": 1, "from": "/old-y/"}));

    let sum = f.verify().await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    assert_eq!(f.status(r_about), Some(ItemStatus::Done));
    assert_eq!(f.status(r_y), Some(ItemStatus::Done));
    let ex = failed(f.status(r_x));
    assert!(ex.contains("/old-x/") && ex.contains("/x/"), "{ex}");
    let gaps = f.issues(IssueKind::AccessGap);
    assert_eq!(gaps.len(), 1, "{gaps:#?}");
    assert_eq!(gaps[0].severity, Severity::Info);
    assert_eq!(
        gaps[0].message,
        "live URL check skipped: set verify.site_url"
    );
    // The checks land in the verify report.
    let html = porter_engine::report::render_verify(&f.p).unwrap();
    assert!(html.contains("URL checks"), "no URL table");
    assert!(html.contains("/old-x/"));
    assert!(html.contains("Counts"));
}

#[tokio::test]
async fn live_verify_roundtrip() {
    use porter_engine::destinations::payload::client::PayloadClient;
    use porter_engine::stage::Destination;
    let (Ok(url), Ok(api_key)) = (
        std::env::var("PORTER_PAYLOAD_TEST_URL"),
        std::env::var("PORTER_PAYLOAD_API_KEY"),
    ) else {
        eprintln!("skipped: PORTER_PAYLOAD_TEST_URL / PORTER_PAYLOAD_API_KEY not set");
        return;
    };
    let c = PayloadClient::new(&url, &api_key, "users").unwrap();
    let mut f = fx(&url);
    f.cfg.destination.api_key_env = "PORTER_PAYLOAD_API_KEY".into();
    f.cfg.destination.config_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/payload-app/src"));
    // Test-app pages have no `content` field.
    f.cfg.mapping.types.get_mut("page").unwrap().fields.clear();
    let run = uuid::Uuid::now_v7().simple().to_string();
    let sid = |n: &str| format!("live-{run}-{n}");
    let docs = [
        doc(&sid("p1"), &format!("{run}-root"), None),
        doc(&sid("p2"), "child", Some(&sid("p1"))),
        doc(&sid("p3"), "grandchild", Some(&sid("p2"))),
    ];
    for d in &docs {
        f.p.store()
            .put_item(ItemKind::Document, d.id, &d.source, d)
            .unwrap();
    }
    let dest = PayloadDestination::new(&f.cfg.destination);
    let load = run_stage(
        f.p.clone(),
        Arc::new(f.cfg.clone()),
        EventSink::new(1024),
        CancellationToken::new(),
        Stage::Load,
        |ctx| async move { dest.load(&ctx, true).await },
    )
    .await
    .unwrap();
    assert_eq!(exit_code(&load), 0, "{load:?}");

    let sum = f.verify().await;
    let issues = f.p.store().issues(&IssueFilter::default()).unwrap();
    assert_eq!(exit_code(&sum), 0, "{sum:?} {issues:#?}");
    eprintln!("live verify clean: {sum:?}");

    // Corrupt one parent through the REST API: grandchild -> root.
    let (_, leaf) = f.p.store().dest_id(docs[2].id).unwrap().unwrap();
    let (_, root) = f.p.store().dest_id(docs[0].id).unwrap().unwrap();
    c.update("pages", &leaf, &json!({"parent": root}), false)
        .await
        .unwrap();
    let sum = f.verify().await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    let ps = f.issues(IssueKind::ParentMissing);
    assert_eq!(ps.len(), 1, "{ps:#?}");
    assert_eq!(ps[0].item, Some(docs[2].id));
    eprintln!("live verify after corruption: {}", ps[0].message);
}
