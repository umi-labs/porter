//! Payload load: ordering, retries, resume, dry run, drops (wiremock fake Payload), plus a
//! live round trip gated by PORTER_PAYLOAD_TEST_URL + PORTER_PAYLOAD_API_KEY.
use indexmap::IndexMap;
use porter_engine::config::ProjectConfig;
use porter_engine::destinations::payload::PayloadDestination;
use porter_engine::events::{EventSink, Summary, exit_code};
use porter_engine::mapping::TypeMap;
use porter_engine::project::Project;
use porter_engine::stage::{Destination, run_stage};
use porter_engine::store::IssueFilter;
use porter_model::{
    DocStatus, Document, FieldValue, Inline, IssueKind, ItemId, ItemKind, ItemStatus, Marks,
    RichText, RtBlock, Seo, SourceRef, Stage,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const KEY_ENV: &str = "PORTER_TEST_LOAD_KEY";

// ---- a stateful fake of Payload's REST API ----

#[derive(Default)]
struct State {
    /// (collection, doc)
    docs: Vec<(String, Value)>,
    /// (method, path, body)
    log: Vec<(String, String, Value)>,
    posts: usize,
    fail_title: Option<String>,
    cancel_after_posts: Option<(usize, CancellationToken)>,
    /// Upload POSTs to answer with Payload's unique-filename error first.
    collisions: usize,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

fn sv(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

impl Respond for Fake {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        let mut body: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        // Multipart upload: the doc's fields are the `_payload` part.
        let text = String::from_utf8_lossy(&r.body);
        if let Some(i) = text.find("name=\"_payload\"") {
            let rest = &text[i..];
            let start = rest.find("\r\n\r\n").unwrap() + 4;
            let end = rest[start..].find("\r\n--").unwrap();
            body = serde_json::from_str(&rest[start..start + end]).unwrap();
        }
        let path = r.url.path().trim_start_matches("/api/").to_string();
        st.log
            .push((r.method.to_string(), path.clone(), body.clone()));
        let segs: Vec<&str> = path.split('/').collect();
        match (r.method.as_str(), segs.as_slice()) {
            ("GET", [coll]) => {
                let filter: Vec<(String, String)> = r
                    .url
                    .query_pairs()
                    .filter_map(|(k, v)| {
                        let f = k.strip_prefix("where[")?.strip_suffix("][equals]")?;
                        Some((f.to_string(), v.to_string()))
                    })
                    .collect();
                let docs: Vec<Value> = st
                    .docs
                    .iter()
                    .filter(|(c, d)| c == coll && filter.iter().all(|(f, v)| sv(&d[f]) == *v))
                    .map(|(_, d)| d.clone())
                    .collect();
                ResponseTemplate::new(200)
                    .set_body_json(json!({"docs": docs, "totalDocs": docs.len()}))
            }
            ("POST", ["globals", _]) => {
                ResponseTemplate::new(200).set_body_json(json!({"result": body}))
            }
            ("POST", [coll]) => {
                if *coll == "media" && st.collisions > 0 {
                    st.collisions -= 1;
                    return ResponseTemplate::new(400).set_body_json(json!({"errors": [{
                        "message": "The following field is invalid: filename",
                        "data": {"errors": [{"path": "filename", "message": "Value must be unique"}]}
                    }]}));
                }
                if st.fail_title.is_some() && body["title"].as_str() == st.fail_title.as_deref() {
                    return ResponseTemplate::new(503).insert_header("Retry-After", "0");
                }
                // Ids are per collection, as in Payload's SQL adapters.
                let next = st
                    .docs
                    .iter()
                    .filter(|(c, _)| c == coll)
                    .filter_map(|(_, d)| d["id"].as_u64())
                    .max()
                    .unwrap_or(0)
                    + 1;
                let mut doc = body.clone();
                doc["id"] = json!(next);
                st.docs.push((coll.to_string(), doc.clone()));
                st.posts += 1;
                if let Some((n, tok)) = &st.cancel_after_posts
                    && st.posts == *n
                {
                    tok.cancel();
                }
                ResponseTemplate::new(201).set_body_json(json!({"doc": doc}))
            }
            ("PATCH", [coll, id]) => {
                let Some((_, d)) = st
                    .docs
                    .iter_mut()
                    .find(|(c, d)| c == coll && sv(&d["id"]) == *id)
                else {
                    return ResponseTemplate::new(404);
                };
                for (k, v) in body.as_object().unwrap() {
                    d[k] = v.clone();
                }
                ResponseTemplate::new(200).set_body_json(json!({"doc": d.clone()}))
            }
            _ => ResponseTemplate::new(404),
        }
    }
}

impl Fake {
    fn log(&self) -> Vec<(String, String, Value)> {
        self.0.lock().unwrap().log.clone()
    }
    fn writes(&self) -> Vec<(String, String, Value)> {
        self.log()
            .into_iter()
            .filter(|(m, ..)| m != "GET")
            .collect()
    }
}

// ---- fixtures ----

fn src(sid: &str) -> SourceRef {
    SourceRef {
        connector: "wordpress".into(),
        source_id: sid.into(),
        url: Some(format!("https://old.example.test/{sid}/")),
    }
}

fn did(sid: &str) -> ItemId {
    ItemId::new(ItemKind::Document, &src(sid))
}

fn para(text: &str) -> RichText {
    RichText {
        blocks: vec![RtBlock::Paragraph {
            inlines: vec![Inline::Text {
                text: text.into(),
                marks: Marks::default(),
            }],
            align: Default::default(),
        }],
    }
}

fn doc(ty: &str, sid: &str, slug: &str, parent: Option<&str>) -> Document {
    let mut fields = IndexMap::new();
    fields.insert(
        "content".to_string(),
        FieldValue::RichText(para(&format!("Body of {slug}"))),
    );
    Document {
        id: did(sid),
        doc_type: ty.into(),
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

struct Fx {
    _d: tempfile::TempDir,
    p: Arc<Project>,
    cfg: ProjectConfig,
}

fn fx(url: &str) -> Fx {
    // SAFETY: every test sets the same value, so concurrent writers can't disagree.
    unsafe { std::env::set_var(KEY_ENV, "test-key") };
    let d = tempfile::tempdir().unwrap();
    let p = Project::init(d.path(), "t", "wordpress").unwrap();
    let mut cfg = p.config().clone();
    cfg.destination.adapter = "payload".into();
    cfg.destination.url = url.into();
    cfg.destination.api_key_env = KEY_ENV.into();
    let tm = |to: &str| TypeMap {
        to: Some(to.into()),
        path: Some("{parent.path}/{slug}/".into()),
        fields: [(
            "content".to_string(),
            porter_engine::mapping::FieldMap {
                to: "content".into(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
        ..TypeMap::default()
    };
    cfg.mapping.types.insert("page".into(), tm("pages"));
    cfg.mapping.types.insert(
        "member".into(),
        TypeMap {
            drop_reason: Some("members are handled by the CRM".into()),
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
    fn put(&self, d: &Document) {
        self.p
            .store()
            .put_item(ItemKind::Document, d.id, &d.source, d)
            .unwrap();
    }

    async fn load(&self, apply: bool, cancel: CancellationToken) -> Summary {
        let dest = PayloadDestination::new(&self.cfg.destination);
        run_stage(
            self.p.clone(),
            Arc::new(self.cfg.clone()),
            EventSink::new(1024),
            cancel,
            Stage::Load,
            |ctx| async move { dest.load(&ctx, apply).await },
        )
        .await
        .unwrap()
    }

    fn status(&self, id: ItemId) -> Option<ItemStatus> {
        self.p.store().status(Stage::Load, id).unwrap()
    }
}

async fn server(fake: &Fake) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(any())
        .respond_with(fake.clone())
        .mount(&s)
        .await;
    s
}

fn key(id: ItemId) -> String {
    id.to_string()
}

// ---- tests ----

#[tokio::test]
async fn children_after_parents() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    // Stored child first: order must come from the tree, not the store.
    f.put(&doc("page", "post:2", "b", Some("post:1")));
    f.put(&doc("page", "post:1", "a", None));
    let sum = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 0, "{sum:?}");

    let writes = fake.writes();
    let posts: Vec<&Value> = writes
        .iter()
        .filter(|(m, ..)| m == "POST")
        .map(|(.., b)| b)
        .collect();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0]["legacyId"], key(did("post:1")));
    assert_eq!(posts[1]["legacyId"], key(did("post:2")));
    // Pass 1 carries scalars only.
    assert!(posts[0].get("content").is_none(), "{}", posts[0]);
    let last_post = writes.iter().rposition(|(m, ..)| m == "POST").unwrap();
    let first_patch = writes.iter().position(|(m, ..)| m == "PATCH").unwrap();
    assert!(last_post < first_patch, "{writes:?}");
    let patches: Vec<&(String, String, Value)> =
        writes.iter().filter(|(m, ..)| m == "PATCH").collect();
    assert_eq!(patches.len(), 2);
    let child = patches
        .iter()
        .find(|(_, p, _)| p == "pages/2")
        .expect("child patched");
    assert_eq!(child.2["parent"], json!(1));
    assert!(child.2["content"]["root"].is_object(), "{}", child.2);
    assert_eq!(f.status(did("post:2")), Some(ItemStatus::Done));
    let rows = f.p.store().dest_ids().unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn load_retries_then_fails_item() {
    let fake = Fake::default();
    fake.0.lock().unwrap().fail_title = Some("BAD".into());
    let s = server(&fake).await;
    let f = fx(&s.uri());
    f.put(&doc("page", "post:1", "good", None));
    f.put(&doc("page", "post:2", "bad", None));
    f.put(&doc("page", "post:3", "fine", None));
    let sum = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    let bad_posts = fake
        .writes()
        .iter()
        .filter(|(m, _, b)| m == "POST" && b["title"] == "BAD")
        .count();
    assert_eq!(bad_posts, 5);
    assert!(matches!(
        f.status(did("post:2")),
        Some(ItemStatus::Failed { .. })
    ));
    assert_eq!(f.status(did("post:1")), Some(ItemStatus::Done));
    assert_eq!(f.status(did("post:3")), Some(ItemStatus::Done));
    let issues =
        f.p.store()
            .issues(&IssueFilter {
                stage: Some(Stage::Load),
                ..Default::default()
            })
            .unwrap();
    assert!(
        issues
            .iter()
            .any(|i| i.kind == IssueKind::WriteFailed && i.item == Some(did("post:2"))),
        "{issues:?}"
    );
}

#[tokio::test]
async fn load_resumes_without_duplicates() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    for i in 1..=6 {
        f.put(&doc("page", &format!("post:{i}"), &format!("p{i}"), None));
    }
    let cancel = CancellationToken::new();
    fake.0.lock().unwrap().cancel_after_posts = Some((3, cancel.clone()));
    let first = f.load(true, cancel).await;
    assert_eq!(exit_code(&first), 3, "{first:?}");
    assert_eq!(fake.0.lock().unwrap().posts, 3);
    assert_eq!(f.p.store().dest_ids().unwrap().len(), 3);

    fake.0.lock().unwrap().cancel_after_posts = None;
    let before = fake.log().len();
    let second = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&second), 0, "{second:?}");
    let rerun: Vec<(String, String, Value)> = fake.log()[before..].to_vec();
    let posted: Vec<String> = rerun
        .iter()
        .filter(|(m, ..)| m == "POST")
        .map(|(.., b)| sv(&b["legacyId"]))
        .collect();
    assert_eq!(posted.len(), 3, "{posted:?}");
    for i in 1..=3 {
        assert!(!posted.contains(&key(did(&format!("post:{i}")))));
    }
    // The 3 loaded earlier were updated in pass 1, not re-created.
    let pass1_patches = rerun
        .iter()
        .filter(|(m, _, b)| m == "PATCH" && b.get("content").is_none())
        .count();
    assert_eq!(pass1_patches, 3);
    assert_eq!(fake.0.lock().unwrap().docs.len(), 6);
    assert_eq!(f.p.store().dest_ids().unwrap().len(), 6);
}

#[tokio::test]
async fn dry_run_sends_nothing() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    f.put(&doc("page", "post:1", "a", None));
    f.put(&doc("page", "post:2", "b", Some("post:1")));
    let sum = f.load(false, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 0, "{sum:?}");
    assert!(fake.writes().is_empty(), "{:?}", fake.writes());
    for sid in ["post:1", "post:2"] {
        assert_eq!(
            f.status(did(sid)),
            Some(ItemStatus::Skipped {
                reason: "dry run".into()
            })
        );
        let p =
            f.p.raw_dir("load-preview")
                .join("pages")
                .join(format!("{}.json", did(sid)));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["legacyId"], key(did(sid)));
        assert!(v["content"]["root"].is_object(), "{v}");
    }
    assert!(f.p.store().dest_ids().unwrap().is_empty());
}

/// What the Apply sheet lists as not written comes from the dry run's own statuses, and
/// the documents the dry run would write are not among them.
#[tokio::test]
async fn dry_run_left_out_lists_only_what_is_not_written() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    f.put(&doc("page", "post:1", "a", None));
    f.put(&doc("page", "post:2", "b", None));
    f.put(&doc("member", "member:1", "m", None));
    f.put(&doc("news", "news:1", "n", None));
    f.load(false, CancellationToken::new()).await;
    let store = f.p.store();
    // (`run_stage` alone: the session is what marks the row as a dry run.)
    let run = store.runs(Stage::Load).unwrap().pop().unwrap();
    // A failed item in the same run is left out too.
    store
        .set_status(
            porter_engine::store::RunId(run.id.parse().unwrap()),
            Stage::Load,
            did("post:2"),
            &ItemStatus::Failed {
                error: "boom".into(),
            },
        )
        .unwrap();
    let mut got = store.run_left_out(&run.id, Stage::Load).unwrap();
    got.sort();
    assert_eq!(
        got,
        [
            (
                "member".to_string(),
                "members are handled by the CRM".to_string(),
                1
            ),
            ("news".into(), "no destination mapped".into(), 1),
            ("page".into(), "failed".into(), 1),
        ]
    );
}

#[tokio::test]
async fn dry_run_clears_old_previews() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    f.put(&doc("page", "post:1", "a", None));
    let stale = f.p.raw_dir("load-preview").join("pages").join("gone.json");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "{}").unwrap();
    f.load(false, CancellationToken::new()).await;
    assert!(
        !stale.exists(),
        "an earlier dry run's preview must not survive"
    );
    let dir = f.p.raw_dir("load-preview");
    assert!(dir.join(".done").exists(), "finished dry run is marked");
    assert!(
        dir.join("pages")
            .join(format!("{}.json", did("post:1")))
            .exists()
    );
}

#[tokio::test]
async fn dropped_type_skipped_with_reason() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    f.put(&doc("member", "member:1", "m", None));
    f.put(&doc("page", "post:1", "a", None));
    f.load(true, CancellationToken::new()).await;
    assert_eq!(
        f.status(did("member:1")),
        Some(ItemStatus::Skipped {
            reason: "members are handled by the CRM".into()
        })
    );
    assert!(
        fake.writes()
            .iter()
            .all(|(.., b)| b["legacyId"] != key(did("member:1")))
    );
}

#[tokio::test]
async fn status_mapped_to_skip_is_not_loaded() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    let tm = f.cfg.mapping.types.get_mut("page").unwrap();
    tm.status.insert("draft".into(), "skip".into());
    tm.status.insert("private".into(), "draft".into());
    let mut draft = doc("page", "post:1", "a", None);
    draft.status = DocStatus::Draft;
    let mut private = doc("page", "post:2", "b", None);
    private.status = DocStatus::Private;
    f.put(&draft);
    f.put(&private);
    f.load(true, CancellationToken::new()).await;
    assert_eq!(
        f.status(did("post:1")),
        Some(ItemStatus::Skipped {
            reason: "status `draft` skipped in the mapping".into()
        })
    );
    assert_eq!(f.status(did("post:2")), Some(ItemStatus::Done));
    let writes = fake.writes();
    assert!(
        writes
            .iter()
            .all(|(.., b)| b["legacyId"] != key(did("post:1")))
    );
    assert!(
        writes
            .iter()
            .any(|(.., b)| b["legacyId"] == key(did("post:2")))
    );
}

/// A page without a builder layout holds rich text; into a blocks field it goes as one
/// rich-text block, mapped like any other.
#[tokio::test]
async fn rich_text_into_blocks_field_becomes_one_block() {
    use porter_engine::mapping::{BlockMap, BlockMatch};
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    let dir = site("{ name: 'layout', type: 'blocks', blocks: [Content] }", "");
    let cfg = std::fs::read_to_string(dir.path().join("payload.config.ts")).unwrap();
    std::fs::write(
        dir.path().join("payload.config.ts"),
        format!("const Content = {{ slug: 'content', fields: [{{ name: 'richText', type: 'richText' }}] }}\n{cfg}"),
    )
    .unwrap();
    f.cfg.destination.config_dir = Some(dir.path().to_path_buf());
    f.cfg.mapping.types.get_mut("page").unwrap().fields.clear();
    map_field(&mut f, "page", "content", "layout");
    f.cfg.mapping.blocks = vec![BlockMap {
        r#match: BlockMatch {
            kind: Some("rich-text".into()),
            module: None,
        },
        to: "content".into(),
        fields: [("text".to_string(), "richText".to_string())]
            .into_iter()
            .collect(),
    }];
    f.put(&doc("page", "post:1", "a", None));
    f.load(true, CancellationToken::new()).await;
    assert_eq!(f.status(did("post:1")), Some(ItemStatus::Done));
    let layout = fake
        .writes()
        .into_iter()
        .find_map(|(.., b)| b.get("layout").cloned())
        .expect("layout written");
    assert_eq!(layout[0]["blockType"], "content", "{layout}");
    assert!(layout[0]["richText"]["root"].is_object(), "{layout}");
}

/// Schema-aware writes: a required field left empty fails the item without a write, a
/// polymorphic relation is `{relationTo, value}`, and a cleared array goes as `[]`.
#[tokio::test]
async fn schema_rules_required_polymorphic_nulls() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    let site = tempfile::tempdir().unwrap();
    std::fs::write(
        site.path().join("payload.config.ts"),
        r#"import { buildConfig } from 'payload'
export default buildConfig({
  collections: [{
    slug: 'pages',
    fields: [
      { name: 'title', type: 'text', required: true },
      { name: 'legacyId', type: 'text', unique: true },
      { name: 'content', type: 'richText' },
      { name: 'subtitle', type: 'text', required: true },
      { name: 'related', type: 'relationship', relationTo: ['pages', 'posts'] },
      { name: 'tags', type: 'array', fields: [{ name: 'tag', type: 'text' }] },
    ],
  }],
})
"#,
    )
    .unwrap();
    f.cfg.destination.config_dir = Some(site.path().to_path_buf());
    let tm = f.cfg.mapping.types.get_mut("page").unwrap();
    for (from, to) in [("sub", "subtitle"), ("rel", "related"), ("tags", "tags")] {
        tm.fields.insert(
            from.into(),
            porter_engine::mapping::FieldMap {
                to: to.into(),
                ..Default::default()
            },
        );
    }
    let mut a = doc("page", "post:1", "a", None);
    a.fields
        .insert("sub".into(), FieldValue::Text("Sub".into()));
    a.fields
        .insert("rel".into(), FieldValue::Doc(did("post:2")));
    a.fields.insert("tags".into(), FieldValue::Null);
    let mut b = doc("page", "post:2", "b", None);
    b.fields.insert("sub".into(), FieldValue::Text("  ".into()));
    f.put(&a);
    f.put(&b);
    let sum = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    match f.status(did("post:2")) {
        Some(ItemStatus::Failed { error }) => assert!(error.contains("subtitle"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert!(
        fake.writes()
            .iter()
            .all(|(.., b)| b["legacyId"] != key(did("post:2")))
    );
    let issues = f.p.store().issues(&IssueFilter::default()).unwrap();
    assert!(issues.iter().any(|i| i.kind == IssueKind::SchemaMismatch
        && i.item == Some(did("post:2"))
        && i.message.contains("subtitle")));
    // post:2 failed, so post:1's relation to it is unresolved.
    let patch = fake
        .writes()
        .into_iter()
        .find(|(m, ..)| m == "PATCH")
        .unwrap()
        .2;
    assert_eq!(patch["tags"], json!([]), "{patch}");
    assert_eq!(patch["related"], Value::Null, "{patch}");
    assert!(matches!(
        f.status(did("post:1")),
        Some(ItemStatus::Partial { .. })
    ));

    // Once post:2 loads, the relation is written polymorphic.
    let mut b2 = doc("page", "post:2", "b", None);
    b2.fields.insert("sub".into(), FieldValue::Text("B".into()));
    f.put(&b2);
    let before = fake.log().len();
    f.load(true, CancellationToken::new()).await;
    let dest = |sid: &str| {
        f.p.store()
            .dest_ids()
            .unwrap()
            .into_iter()
            .find(|(i, ..)| *i == did(sid))
            .unwrap()
            .2
    };
    let a_path = format!("pages/{}", sv(&dest("post:1")));
    let rel = fake.log()[before..]
        .iter()
        .filter(|(m, p, b)| m == "PATCH" && *p == a_path && b.get("related").is_some())
        .map(|(.., b)| b["related"].clone())
        .next_back()
        .unwrap();
    assert_eq!(rel, json!({"relationTo": "pages", "value": dest("post:2")}));
    assert_eq!(f.status(did("post:1")), Some(ItemStatus::Done));
}

#[tokio::test]
async fn media_uploads_collide_then_go_one_at_a_time() {
    use porter_model::Media;
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    let mut ids = vec![];
    for n in 0..5 {
        let h =
            f.p.store()
                .put_media_bytes(
                    format!("bytes {n}").as_bytes(),
                    &format!("https://old.example.test/m{n}.png"),
                    "same.png",
                )
                .unwrap();
        let mut m: Media = f.p.store().get_item(h.id).unwrap().unwrap();
        m.alt = Some(format!("New {n}"));
        let src = SourceRef {
            connector: "media".into(),
            source_id: m.hash.clone(),
            url: None,
        };
        f.p.store()
            .put_item(ItemKind::Media, m.id, &src, &m)
            .unwrap();
        ids.push(m.id);
    }
    {
        // ids[0] was uploaded on an earlier run, with older metadata.
        let mut st = fake.0.lock().unwrap();
        st.docs.push((
            "media".into(),
            json!({"id": 100, "legacyId": ids[0].to_string(), "alt": "Old"}),
        ));
        st.collisions = 1;
    }
    let sum = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 0, "{sum:?}");
    for id in &ids {
        assert_eq!(f.status(*id), Some(ItemStatus::Done));
    }
    let st = fake.0.lock().unwrap();
    assert_eq!(st.docs.len(), 5, "no duplicate uploads");
    assert_eq!(
        st.docs[0].1["alt"], "New 0",
        "existing media metadata updated"
    );
    let patches: Vec<_> = st.log.iter().filter(|(m, ..)| m == "PATCH").collect();
    assert_eq!(patches.len(), 1);
    assert_eq!(
        patches[0].2,
        json!({"alt": "New 0", "_status": "published"})
    );
    drop(st);
    assert_eq!(f.p.store().dest_ids().unwrap().len(), 5);
}

#[tokio::test]
async fn upload_sends_focal_point() {
    use porter_model::Media;
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    let h =
        f.p.store()
            .put_media_bytes(b"bytes", "https://old.example.test/m.png", "m.png")
            .unwrap();
    let mut m: Media = f.p.store().get_item(h.id).unwrap().unwrap();
    m.focal_x = Some(30.0);
    m.focal_y = Some(60.0);
    let src = SourceRef {
        connector: "media".into(),
        source_id: m.hash.clone(),
        url: None,
    };
    f.p.store()
        .put_item(ItemKind::Media, m.id, &src, &m)
        .unwrap();
    let sum = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 0, "{sum:?}");
    {
        let st = fake.0.lock().unwrap();
        let post = st
            .log
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "media")
            .unwrap();
        assert_eq!(
            (&post.2["focalX"], &post.2["focalY"]),
            (&json!(30.0), &json!(60.0))
        );
    }
    // Existing doc with a different focal point: PATCHed.
    {
        let mut st = fake.0.lock().unwrap();
        st.log.clear();
        st.docs[0].1["focalX"] = json!(50.0);
    }
    f.load(true, CancellationToken::new()).await;
    let st = fake.0.lock().unwrap();
    let patch = st.log.iter().find(|(m, ..)| m == "PATCH").unwrap();
    assert_eq!(patch.2["focalX"], json!(30.0));
}

/// A Payload config for `pages` and `posts`, written to a temp dir.
fn site(fields: &str, blocks: &str) -> tempfile::TempDir {
    let site = tempfile::tempdir().unwrap();
    std::fs::write(
        site.path().join("payload.config.ts"),
        format!(
            r#"import {{ buildConfig }} from 'payload'
export default buildConfig({{
  collections: [
    {{ slug: 'pages', fields: [
      {{ name: 'title', type: 'text' }},
      {{ name: 'legacyId', type: 'text', unique: true }},
      {fields}
    ] }},
    {{ slug: 'posts', fields: [
      {{ name: 'title', type: 'text' }},
      {{ name: 'legacyId', type: 'text', unique: true }},
    ] }},
  ],
  blocks: [{blocks}],
}})
"#
        ),
    )
    .unwrap();
    site
}

fn map_field(f: &mut Fx, ty: &str, from: &str, to: &str) {
    f.cfg.mapping.types.get_mut(ty).unwrap().fields.insert(
        from.into(),
        porter_engine::mapping::FieldMap {
            to: to.into(),
            ..Default::default()
        },
    );
}

fn with_posts(f: &mut Fx) {
    f.cfg.mapping.types.insert(
        "post".into(),
        TypeMap {
            to: Some("posts".into()),
            ..TypeMap::default()
        },
    );
}

fn issues_of(f: &Fx, id: ItemId) -> Vec<porter_model::Issue> {
    f.p.store()
        .issues(&IssueFilter::default())
        .unwrap()
        .into_iter()
        .filter(|i| i.item == Some(id))
        .collect()
}

fn patch_of(fake: &Fake, path: &str) -> Value {
    fake.writes()
        .into_iter()
        .rfind(|(m, p, _)| m == "PATCH" && p == path)
        .unwrap()
        .2
}

/// Ids are per collection: a pages id sent to a posts-only field would silently point
/// at an unrelated post.
#[tokio::test]
async fn relation_to_wrong_collection_is_unresolved() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    let dir = site(
        "{ name: 'hero', type: 'relationship', relationTo: 'posts' }",
        "",
    );
    f.cfg.destination.config_dir = Some(dir.path().to_path_buf());
    with_posts(&mut f);
    f.cfg.mapping.types.get_mut("page").unwrap().fields.clear();
    map_field(&mut f, "page", "rel", "hero");
    let mut a = doc("page", "post:1", "a", None);
    a.fields
        .insert("rel".into(), FieldValue::Doc(did("post:2")));
    let mut b = doc("page", "post:3", "b", None);
    b.fields
        .insert("rel".into(), FieldValue::Doc(did("post:4")));
    f.put(&a);
    f.put(&doc("page", "post:2", "other-page", None));
    f.put(&b);
    f.put(&doc("post", "post:4", "a-post", None));
    f.load(true, CancellationToken::new()).await;
    // pages/1 -> a page: wrong collection, so null and a warning.
    assert_eq!(patch_of(&fake, "pages/1")["hero"], Value::Null);
    assert!(
        issues_of(&f, did("post:1"))
            .iter()
            .any(|i| i.kind == IssueKind::UnresolvedRelation
                && i.message.contains("posts")
                && i.message.contains("pages")),
        "{:?}",
        issues_of(&f, did("post:1"))
    );
    assert!(matches!(
        f.status(did("post:1")),
        Some(ItemStatus::Partial { .. })
    ));
    // pages/3 -> a post: right collection, plain id.
    assert_eq!(patch_of(&fake, "pages/3")["hero"], json!(1));
}

#[tokio::test]
async fn parent_in_other_collection_is_not_written() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    with_posts(&mut f);
    f.put(&doc("page", "post:1", "a", None));
    f.put(&doc("post", "post:2", "b", Some("post:1")));
    f.load(true, CancellationToken::new()).await;
    let patch = patch_of(&fake, "posts/1");
    assert!(patch.get("parent").is_none(), "{patch}");
    assert!(
        issues_of(&f, did("post:2"))
            .iter()
            .any(|i| i.kind == IssueKind::ParentMissing && i.message.contains("pages"))
    );
    assert!(matches!(
        f.status(did("post:2")),
        Some(ItemStatus::Partial { .. })
    ));
}

/// pages:3 and posts:3 both exist; a block reference must carry its own collection.
#[tokio::test]
async fn block_polymorphic_relation_uses_item_collection() {
    use porter_engine::mapping::{BlockMap, BlockMatch};
    use porter_model::{Block, BlockKind};
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    let dir = site("{ name: 'layout', type: 'blocks', blocks: [Card] }", "");
    // The block, inline in the config reader's eyes via a const.
    let cfg = std::fs::read_to_string(dir.path().join("payload.config.ts")).unwrap();
    std::fs::write(
        dir.path().join("payload.config.ts"),
        format!(
            "const Card = {{ slug: 'card', fields: [\n  {{ name: 'link', type: 'relationship', relationTo: ['pages', 'posts'] }},\n  {{ name: 'page', type: 'relationship', relationTo: 'pages' }},\n] }}\n{cfg}"
        ),
    )
    .unwrap();
    f.cfg.destination.config_dir = Some(dir.path().to_path_buf());
    with_posts(&mut f);
    f.cfg.mapping.types.get_mut("page").unwrap().fields.clear();
    map_field(&mut f, "page", "layout", "layout");
    f.cfg.mapping.blocks = vec![BlockMap {
        r#match: BlockMatch {
            kind: Some("image".into()),
            module: None,
        },
        to: "card".into(),
        fields: [("link", "link"), ("page", "page")]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
    }];
    for i in 1..=3 {
        f.put(&doc("post", &format!("post:b{i}"), &format!("b{i}"), None));
    }
    let mut block = Block {
        kind: BlockKind::Image,
        props: IndexMap::new(),
        children: vec![],
        original: None,
    };
    block
        .props
        .insert("link".into(), FieldValue::Doc(did("post:b3")));
    // A post given to a pages-only field: must not become pages/3.
    block
        .props
        .insert("page".into(), FieldValue::Doc(did("post:b3")));
    let mut a = doc("page", "post:1", "a", None);
    a.fields
        .insert("layout".into(), FieldValue::Layout(vec![block]));
    f.put(&a);
    f.put(&doc("page", "post:2", "p2", None));
    f.put(&doc("page", "post:3", "p3", None));
    f.load(true, CancellationToken::new()).await;
    let ids = f.p.store().dest_ids().unwrap();
    let dest = |sid: &str| ids.iter().find(|(i, ..)| *i == did(sid)).unwrap().clone();
    assert_eq!(dest("post:3").1, "pages");
    assert_eq!(dest("post:3").2, json!(3));
    assert_eq!(dest("post:b3").1, "posts");
    assert_eq!(dest("post:b3").2, json!(3));
    let a_path = format!("pages/{}", sv(&dest("post:1").2));
    let layout = &patch_of(&fake, &a_path)["layout"];
    assert_eq!(
        layout,
        &json!([{"blockType": "card", "link": {"relationTo": "posts", "value": 3}, "page": null}]),
        "{layout}"
    );
    assert!(
        issues_of(&f, did("post:1"))
            .iter()
            .any(|i| i.kind == IssueKind::UnresolvedRelation && i.message.contains("page"))
    );
}

#[tokio::test]
async fn dry_run_reports_media_without_a_file() {
    use porter_model::Media;
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    let src = SourceRef {
        connector: "media".into(),
        source_id: "nohash".into(),
        url: None,
    };
    let m = Media {
        id: ItemId::new(ItemKind::Media, &src),
        hash: "ab".repeat(32),
        variants: vec![],
        filename: "gone.png".into(),
        mime: "image/png".into(),
        width: None,
        height: None,
        alt: None,
        caption: None,
        title: None,
        credit: None,
        folder: None,
        focal_x: None,
        focal_y: None,
        usages: vec![],
    };
    f.p.store()
        .put_item(ItemKind::Media, m.id, &src, &m)
        .unwrap();
    let sum = f.load(false, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    assert!(matches!(f.status(m.id), Some(ItemStatus::Failed { .. })));
    assert!(
        issues_of(&f, m.id)
            .iter()
            .any(|i| i.kind == IssueKind::MissingMedia)
    );
    assert!(fake.log().is_empty());
}

#[tokio::test]
async fn redirect_to_unloaded_doc_warns() {
    use porter_model::{Redirect, RedirectOrigin};
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    let gone = doc("member", "member:9", "gone", None);
    f.put(&gone);
    let rsrc = SourceRef {
        connector: "wordpress".into(),
        source_id: "redirect:1".into(),
        url: None,
    };
    let rid = ItemId::new(ItemKind::Redirect, &rsrc);
    let r = Redirect {
        from: "/old/".into(),
        to: gone.source.url.clone().unwrap(),
        code: 301,
        origin: RedirectOrigin::Plugin,
    };
    f.p.store()
        .put_item(ItemKind::Redirect, rid, &rsrc, &r)
        .unwrap();
    f.load(true, CancellationToken::new()).await;
    assert!(
        issues_of(&f, rid)
            .iter()
            .any(|i| i.kind == IssueKind::UnresolvedLink),
        "{:?} {:?} {:?}",
        issues_of(&f, rid),
        f.status(rid),
        f.p.store().lookup_url(&r.to)
    );
    assert!(matches!(f.status(rid), Some(ItemStatus::Partial { .. })));
}

#[tokio::test]
async fn required_forward_reference_says_rerun_fixes_it() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    let dir = site(
        "{ name: 'hero', type: 'relationship', relationTo: 'pages', required: true }",
        "",
    );
    f.cfg.destination.config_dir = Some(dir.path().to_path_buf());
    f.cfg.mapping.types.get_mut("page").unwrap().fields.clear();
    map_field(&mut f, "page", "rel", "hero");
    let mut a = doc("page", "post:1", "a", None);
    a.fields
        .insert("rel".into(), FieldValue::Doc(did("post:2")));
    let mut b = doc("page", "post:2", "b", None);
    b.fields
        .insert("rel".into(), FieldValue::Doc(did("post:3")));
    // The chain's root: a draft, so Payload doesn't require `hero`.
    let mut c = doc("page", "post:3", "c", None);
    c.status = DocStatus::Draft;
    f.put(&a);
    f.put(&b);
    f.put(&c);
    f.load(true, CancellationToken::new()).await;
    match f.status(did("post:1")) {
        Some(ItemStatus::Failed { error }) => {
            assert!(
                error.contains("forward reference") && error.contains(&did("post:2").to_string()),
                "{error}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        f.status(did("post:2")),
        Some(ItemStatus::Failed { .. })
    ));
    assert_eq!(f.status(did("post:3")), Some(ItemStatus::Done));
    // Each re-run loads the next link of the chain.
    f.load(true, CancellationToken::new()).await;
    assert_eq!(f.status(did("post:2")), Some(ItemStatus::Done));
    f.load(true, CancellationToken::new()).await;
    assert_eq!(f.status(did("post:1")), Some(ItemStatus::Done));
}

/// The parent was removed at the source: the re-run must clear it in Payload, though no
/// schema lists nested-docs' `parent`. Same for terms.
#[tokio::test]
async fn removed_parent_is_cleared_on_rerun() {
    use porter_model::Term;
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    f.cfg
        .mapping
        .taxonomies
        .insert("category".into(), "categories".into());
    let term = |sid: &str, parent: Option<ItemId>| {
        let src = SourceRef {
            connector: "wordpress".into(),
            source_id: sid.into(),
            url: None,
        };
        let id = ItemId::new(ItemKind::Term, &src);
        let t = Term {
            taxonomy: "category".into(),
            name: sid.into(),
            slug: sid.replace(':', "-"),
            parent,
        };
        f.p.store().put_item(ItemKind::Term, id, &src, &t).unwrap();
        id
    };
    let top = term("term:1", None);
    let sub = term("term:2", Some(top));
    f.put(&doc("page", "post:1", "a", None));
    f.put(&doc("page", "post:2", "b", Some("post:1")));
    f.load(true, CancellationToken::new()).await;
    assert_eq!(patch_of(&fake, "pages/2")["parent"], json!(1));
    let cat = |fake: &Fake| {
        fake.0
            .lock()
            .unwrap()
            .docs
            .iter()
            .find(|(c, d)| c == "categories" && d["legacyId"] == sub.to_string())
            .unwrap()
            .1
            .clone()
    };
    assert_eq!(cat(&fake)["parent"], json!(1));

    // Both parents removed at the source.
    f.put(&doc("page", "post:2", "b", None));
    term("term:2", None);
    let before = fake.log().len();
    f.load(true, CancellationToken::new()).await;
    let rerun = fake.log()[before..].to_vec();
    let page = rerun
        .iter()
        .rfind(|(m, p, _)| m == "PATCH" && p == "pages/2")
        .unwrap();
    assert_eq!(page.2["parent"], Value::Null, "{}", page.2);
    assert!(page.2.as_object().unwrap().contains_key("parent"));
    let term_patch = rerun
        .iter()
        .rfind(|(m, p, _)| m == "PATCH" && p == "categories/2")
        .unwrap();
    assert!(
        term_patch.2.as_object().unwrap().contains_key("parent"),
        "{}",
        term_patch.2
    );
    assert_eq!(cat(&fake)["parent"], Value::Null);
    assert_eq!(f.status(did("post:2")), Some(ItemStatus::Done));
}

// ---- live ----

/// A valid 1x1 transparent PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[tokio::test]
async fn live_load_roundtrip() {
    use porter_engine::destinations::payload::client::PayloadClient;
    use porter_engine::mapping::{MenusMap, RedirectsMap};
    use porter_model::{Media, Menu, MenuItem, MenuTarget, Redirect, RedirectOrigin};
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
    f.cfg.mapping.redirects = RedirectsMap {
        collection: "redirects".into(),
        ..Default::default()
    };
    f.cfg.mapping.menus = MenusMap {
        global: Some("navigation".into()),
        collection: None,
        fields: [("Main".to_string(), "main".to_string())].into(),
    };
    f.cfg.mapping.raw_block = Some("raw".into());
    // Unique per run, so the test reruns against the same database.
    let run = uuid::Uuid::now_v7().simple().to_string();
    let sid = |n: &str| format!("live-{run}-{n}");

    // 2 media.
    let mut media = vec![];
    for n in ["m1", "m2"] {
        let bytes = [PNG, run.as_bytes(), n.as_bytes()].concat();
        let h =
            f.p.store()
                .put_media_bytes(
                    &bytes,
                    &format!("https://old.example.test/{run}/{n}.png"),
                    &format!("{run}-{n}.png"),
                )
                .unwrap();
        let mut m: Media = f.p.store().get_item(h.id).unwrap().unwrap();
        m.mime = "image/png".into();
        m.alt = Some(format!("Alt {n}"));
        m.folder = Some(format!("{run}/sub"));
        let srcm = SourceRef {
            connector: "media".into(),
            source_id: m.hash.clone(),
            url: m.variants.first().cloned(),
        };
        f.p.store()
            .put_item(ItemKind::Media, m.id, &srcm, &m)
            .unwrap();
        media.push(m.id);
    }
    // 5 docs: a tree of 3 pages, 2 posts.
    f.cfg.mapping.types.insert(
        "post".into(),
        TypeMap {
            to: Some("posts".into()),
            path: Some("/blog/{slug}/".into()),
            fields: [(
                "content".to_string(),
                porter_engine::mapping::FieldMap {
                    to: "content".into(),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..TypeMap::default()
        },
    );
    // Test-app pages have no `content` field.
    f.cfg.mapping.types.get_mut("page").unwrap().fields.clear();
    let mut b1 = doc("post", &sid("b1"), &format!("{run}-b1"), None);
    b1.fields.insert(
        "content".into(),
        FieldValue::RichText(RichText {
            blocks: vec![RtBlock::Media {
                media: media[0],
                caption: None,
                align: Default::default(),
                width: None,
                link: None,
            }],
        }),
    );
    let docs = [
        doc("page", &sid("p1"), &format!("{run}-root"), None),
        doc("page", &sid("p2"), "child", Some(&sid("p1"))),
        doc("page", &sid("p3"), "grandchild", Some(&sid("p2"))),
        b1,
        doc("post", &sid("b2"), &format!("{run}-b2"), None),
    ];
    for d in &docs {
        f.put(d);
    }
    // 1 menu, 2 redirects.
    let msrc = SourceRef {
        connector: "wordpress".into(),
        source_id: sid("menu"),
        url: None,
    };
    let menu = Menu {
        name: "Main".into(),
        items: vec![MenuItem {
            label: "Home".into(),
            target: MenuTarget::Doc(docs[0].id),
            children: vec![MenuItem {
                label: "Child".into(),
                target: MenuTarget::Doc(docs[1].id),
                children: vec![],
            }],
        }],
    };
    f.p.store()
        .put_item(
            ItemKind::Menu,
            ItemId::new(ItemKind::Menu, &msrc),
            &msrc,
            &menu,
        )
        .unwrap();
    for (n, from) in [("r1", "old-a"), ("r2", "old-b")] {
        let rsrc = SourceRef {
            connector: "wordpress".into(),
            source_id: sid(n),
            url: None,
        };
        let r = Redirect {
            from: format!("/{run}/{from}/"),
            to: "/somewhere/".into(),
            code: 301,
            origin: RedirectOrigin::Plugin,
        };
        f.p.store()
            .put_item(
                ItemKind::Redirect,
                ItemId::new(ItemKind::Redirect, &rsrc),
                &rsrc,
                &r,
            )
            .unwrap();
    }

    let pages0 = c.count("pages").await.unwrap();
    let posts0 = c.count("posts").await.unwrap();
    let media0 = c.count("media").await.unwrap();
    let redirects0 = c.count("redirects").await.unwrap();
    let sum = f.load(true, CancellationToken::new()).await;
    let issues = f.p.store().issues(&IssueFilter::default()).unwrap();
    assert_eq!(exit_code(&sum), 0, "{sum:?} {issues:#?}");
    assert_eq!(c.count("pages").await.unwrap(), pages0 + 3);
    assert_eq!(c.count("posts").await.unwrap(), posts0 + 2);
    assert_eq!(c.count("media").await.unwrap(), media0 + 2);
    assert_eq!(c.count("redirects").await.unwrap(), redirects0 + 2);
    let ids = f.p.store().dest_ids().unwrap();
    assert_eq!(ids.len(), 2 + 5 + 1 + 2, "{ids:?}");
    let leaf = ids.iter().find(|(i, ..)| *i == docs[2].id).unwrap();
    let mid = ids.iter().find(|(i, ..)| *i == docs[1].id).unwrap();
    let got = c
        .find_by("pages", "legacyId", &json!(key(docs[2].id)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["id"], leaf.2);
    assert_eq!(got["parent"], mid.2);
    let m0 = ids.iter().find(|(i, ..)| *i == media[0]).unwrap();
    let post = c
        .find_by("posts", "legacyId", &json!(key(docs[3].id)))
        .await
        .unwrap()
        .unwrap();
    let upload = &post["content"]["root"]["children"][0];
    assert_eq!(upload["type"], "upload", "{post}");
    assert_eq!(upload["value"], m0.2, "{post}");
    let pic = c.get("media", &m0.2).await.unwrap().unwrap();
    assert!(pic["folder"].is_number(), "{pic}");
    let nav = c
        .get("globals", &json!("navigation"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        nav["main"][0]["children"][0]["url"],
        format!("/{run}-root/child/")
    );

    // Second run: everything updates, nothing new.
    let sum2 = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum2), 0, "{sum2:?}");
    assert_eq!(c.count("pages").await.unwrap(), pages0 + 3);
    assert_eq!(c.count("posts").await.unwrap(), posts0 + 2);
    assert_eq!(c.count("media").await.unwrap(), media0 + 2);
    assert_eq!(c.count("redirects").await.unwrap(), redirects0 + 2);
    assert_eq!(f.p.store().dest_ids().unwrap().len(), ids.len());
}

#[tokio::test]
async fn numeric_keys_must_be_unique() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let mut f = fx(&s.uri());
    f.cfg.mapping.types.get_mut("page").unwrap().key_type = porter_engine::mapping::KeyType::Number;
    // Variant, dedupe and positional ids: no unique number of their own.
    for sid in ["node:5:v1", "node:5~2", "item-3"] {
        f.put(&doc("page", sid, &sid.replace([':', '~'], "-"), None));
    }
    // Two sources ids carrying the same number: they would overwrite each other.
    f.put(&doc("page", "node:7", "a", None));
    f.put(&doc("page", "post:7", "b", None));
    f.put(&doc("page", "node:9", "c", None));
    let sum = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    for sid in ["node:5:v1", "node:5~2", "item-3"] {
        match f.status(did(sid)) {
            Some(ItemStatus::Failed { error }) => {
                assert!(error.contains("has no unique numeric key"), "{error}")
            }
            s => panic!("{sid}: {s:?}"),
        }
    }
    for sid in ["node:7", "post:7"] {
        match f.status(did(sid)) {
            Some(ItemStatus::Failed { error }) => {
                assert!(
                    error.contains("node:7") && error.contains("post:7"),
                    "{error}"
                )
            }
            s => panic!("{sid}: {s:?}"),
        }
    }
    assert_eq!(f.status(did("node:9")), Some(ItemStatus::Done));
    let keys: Vec<Value> = fake
        .writes()
        .iter()
        .filter(|(m, ..)| m == "POST")
        .map(|(.., b)| b["legacyId"].clone())
        .collect();
    assert_eq!(keys, vec![json!(9)]);
}

#[tokio::test]
async fn removed_from_source_is_flagged_not_deleted() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    f.put(&doc("page", "post:1", "a", None));
    f.put(&doc("page", "post:2", "b", None));
    f.load(true, CancellationToken::new()).await;
    assert_eq!(f.status(did("post:2")), Some(ItemStatus::Done));
    // normalise no longer saw post:2 at the source.
    f.p.store().soft_delete(did("post:2")).unwrap();
    let before = fake.log().len();
    let sum = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 0, "{sum:?}");
    assert_eq!(
        f.status(did("post:2")),
        Some(ItemStatus::Skipped {
            reason: "removed from source".into()
        })
    );
    let issues = issues_of(&f, did("post:2"));
    assert!(
        issues
            .iter()
            .any(|i| i.severity == porter_model::Severity::Warning
                && i.message.contains("still in destination at pages/2")),
        "{issues:#?}"
    );
    // Nothing deleted or rewritten for it.
    assert!(
        fake.log()[before..]
            .iter()
            .all(|(m, p, _)| m != "DELETE" && p != "pages/2"),
        "{:?}",
        &fake.log()[before..]
    );
}

#[tokio::test]
async fn undecodable_row_fails_not_vanishes() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    f.put(&doc("page", "post:1", "a", None));
    // A canonical row that is not a Document (corrupt or from an older porter).
    f.p.store()
        .put_item(
            ItemKind::Document,
            did("post:2"),
            &src("post:2"),
            &json!({"nope": 1}),
        )
        .unwrap();
    let sum = f.load(true, CancellationToken::new()).await;
    assert_eq!(exit_code(&sum), 2, "{sum:?}");
    assert_eq!(
        f.status(did("post:2")),
        Some(ItemStatus::Failed {
            error: "undecodable row".into()
        })
    );
    assert!(!issues_of(&f, did("post:2")).is_empty());
    assert_eq!(f.status(did("post:1")), Some(ItemStatus::Done));
}

#[tokio::test]
async fn not_read_this_run_is_not_called_removed() {
    let fake = Fake::default();
    let s = server(&fake).await;
    let f = fx(&s.uri());
    f.put(&doc("page", "post:1", "a", None));
    f.load(true, CancellationToken::new()).await;
    let st = f.p.store();
    st.soft_delete(did("post:1")).unwrap();
    let run = st.begin_run(Stage::Normalise).unwrap();
    let reason = "not produced this run (see extract issues)".to_string();
    st.set_status(
        run,
        Stage::Normalise,
        did("post:1"),
        &ItemStatus::Skipped {
            reason: reason.clone(),
        },
    )
    .unwrap();
    f.load(true, CancellationToken::new()).await;
    assert_eq!(
        f.status(did("post:1")),
        Some(ItemStatus::Skipped { reason })
    );
    let issues = issues_of(&f, did("post:1"));
    assert!(
        issues.iter().any(
            |i| i.message.starts_with("not produced this run") && i.message.contains("pages/1")
        ),
        "{issues:#?}"
    );
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("removed from source")),
        "{issues:#?}"
    );
}
