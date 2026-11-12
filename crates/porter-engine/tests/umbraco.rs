use porter_engine::config::ConnectorConfig;
use porter_engine::events::EventSink;
use porter_engine::project::Project;
use porter_engine::sources::umbraco::UmbracoSource;
use porter_engine::stage::{Source, run_stage};
use porter_engine::store::IssueFilter;
use porter_model::{
    DocStatus, Document, FieldValue, IssueKind, ItemId, ItemKind, ItemStatus, RtBlock, SourceRef,
    Stage,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct Fx {
    _d: tempfile::TempDir,
    p: Arc<Project>,
}

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/umbraco/sample.json")
}

/// A project over `files` (written into its folder) plus any `extra` paths.
fn project(files: &[(&str, Value)], extra: Vec<PathBuf>) -> Fx {
    let d = tempfile::tempdir().unwrap();
    let mut paths: Vec<PathBuf> = files
        .iter()
        .map(|(n, v)| {
            std::fs::write(d.path().join(n), serde_json::to_vec(v).unwrap()).unwrap();
            PathBuf::from(n)
        })
        .collect();
    paths.extend(extra);
    let p = Project::init(d.path(), "t", "umbraco").unwrap();
    let mut cfg = p.config().clone();
    cfg.connectors = vec![ConnectorConfig::UmbracoJson { paths }];
    std::fs::write(d.path().join("porter.toml"), toml::to_string(&cfg).unwrap()).unwrap();
    Fx {
        p: Arc::new(Project::open(d.path()).unwrap()),
        _d: d,
    }
}

async fn stage(fx: &Fx, st: Stage) -> anyhow::Result<porter_engine::events::Summary> {
    let cfg = Arc::new(fx.p.config().clone());
    run_stage(
        fx.p.clone(),
        cfg,
        EventSink::new(4096),
        CancellationToken::new(),
        st,
        |ctx| async move {
            let src = UmbracoSource;
            match st {
                Stage::Normalise => src.normalise(&ctx),
                _ => {
                    for c in src.connectors(&ctx.config)? {
                        if st == Stage::Analyse {
                            c.analyse(&ctx).await?;
                        } else {
                            c.extract(&ctx).await?;
                        }
                    }
                    Ok(())
                }
            }
        },
    )
    .await
}

async fn all(fx: &Fx) {
    for st in [Stage::Analyse, Stage::Extract, Stage::Normalise] {
        stage(fx, st).await.unwrap();
    }
}

fn canon(sid: &str) -> ItemId {
    ItemId::new(
        ItemKind::Document,
        &SourceRef {
            connector: "umbraco".into(),
            source_id: sid.into(),
            url: None,
        },
    )
}

fn raw(sid: &str) -> ItemId {
    ItemId::new(
        ItemKind::Document,
        &SourceRef {
            connector: "umbraco-json".into(),
            source_id: sid.into(),
            url: None,
        },
    )
}

fn doc(fx: &Fx, sid: &str) -> Document {
    fx.p.store()
        .get_item(canon(sid))
        .unwrap()
        .unwrap_or_else(|| panic!("no document {sid}"))
}

fn issues(fx: &Fx, kind: IssueKind) -> Vec<porter_model::Issue> {
    fx.p.store()
        .issues(&IssueFilter {
            kind: Some(kind),
            ..IssueFilter::default()
        })
        .unwrap()
}

#[tokio::test]
async fn umbraco_variants_become_translations() {
    let fx = project(
        &[(
            "v.json",
            json!([{
                "id": 7, "name": "Home", "contentTypeAlias": "home", "published": true,
                "_variants": [
                    { "culture": "en", "name": "Home", "url": "/en/", "properties": { "tagline": "Hello" } },
                    { "culture": "de", "name": "Startseite", "url": "/de/", "properties": { "tagline": "Hallo" } },
                ],
            }]),
        )],
        vec![],
    );
    all(&fx).await;
    let (en, de) = (doc(&fx, "node:7"), doc(&fx, "node:7:de"));
    assert_eq!(en.doc_type, "home");
    assert_eq!(
        (en.locale.as_deref(), de.locale.as_deref()),
        (Some("en"), Some("de"))
    );
    assert_eq!(en.translations, [de.id]);
    assert_eq!(de.translations, [en.id]);
    assert_eq!((en.path.as_str(), de.path.as_str()), ("/en/", "/de/"));
    assert_eq!(de.title, "Startseite");
    assert_eq!(de.fields["tagline"], FieldValue::Text("Hallo".into()));
    let docs =
        fx.p.store()
            .items_decodable::<Document>(ItemKind::Document)
            .unwrap();
    assert_eq!(docs.len(), 2);
}

#[tokio::test]
async fn umbraco_rte_is_richtext() {
    let fx = project(
        &[(
            "r.json",
            json!({ "nodes": [{
                "id": "a1", "name": "Page", "contentTypeAlias": "page",
                "body": "<p>Hello <strong>world</strong></p>",
                "plain": "no markup < here", "count": 3, "on": false, "nothing": null,
                "list": ["x", 2], "obj": { "k": "<p>in</p>" },
            }] }),
        )],
        vec![],
    );
    all(&fx).await;
    let d = doc(&fx, "node:a1");
    let FieldValue::RichText(rt) = &d.fields["body"] else {
        panic!("{:?}", d.fields["body"]);
    };
    assert!(matches!(rt.blocks[0], RtBlock::Paragraph { .. }), "{rt:?}");
    assert_eq!(rt.plain_text(), "Hello world");
    assert_eq!(
        d.fields["plain"],
        FieldValue::Text("no markup < here".into())
    );
    assert_eq!(d.fields["count"], FieldValue::Number(3.0));
    assert_eq!(d.fields["on"], FieldValue::Bool(false));
    assert_eq!(d.fields["nothing"], FieldValue::Null);
    assert_eq!(
        d.fields["list"],
        FieldValue::List(vec![FieldValue::Text("x".into()), FieldValue::Number(2.0)])
    );
    let FieldValue::Object(o) = &d.fields["obj"] else {
        panic!("{:?}", d.fields["obj"]);
    };
    assert!(matches!(o["k"], FieldValue::RichText(_)));
}

#[tokio::test]
async fn umbraco_media_resolves_or_warns() {
    let fx = project(
        &[(
            "m.json",
            json!([{
                "id": 1, "name": "A", "contentTypeAlias": "page", "url": "https://example.com/a/",
                "hero": { "mediaType": "Image", "name": "Hero", "url": "/media/hero.jpg", "width": 800, "height": 600 },
                "gone": { "mediaType": "Image", "url": "/media/gone.jpg" },
                "body": "<p><img src=\"/media/inline.png\"></p>",
            }]),
        )],
        vec![],
    );
    // Already downloaded (the extract step skips what the store has).
    let store = fx.p.store();
    let hero = store
        .put_media_bytes(b"hero", "https://example.com/media/hero.jpg", "hero.jpg")
        .unwrap()
        .id;
    store
        .put_media_bytes(
            b"inline",
            "https://example.com/media/inline.png",
            "inline.png",
        )
        .unwrap();
    all(&fx).await;
    let d = doc(&fx, "node:1");
    assert_eq!(d.fields["hero"], FieldValue::Media(hero));
    // A miss keeps the URL and warns; never an empty value.
    assert_eq!(d.fields["gone"], FieldValue::Text("/media/gone.jpg".into()));
    let msgs: Vec<_> = issues(&fx, IssueKind::MissingMedia)
        .into_iter()
        .map(|i| i.message)
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("/media/gone.jpg")),
        "{msgs:?}"
    );
    assert!(!msgs.iter().any(|m| m.contains("hero.jpg")), "{msgs:?}");
    assert!(!msgs.iter().any(|m| m.contains("inline.png")), "{msgs:?}");
    let m: porter_model::Media = store.get_item(hero).unwrap().unwrap();
    assert_eq!(
        (m.title.as_deref(), m.width, m.height),
        (Some("Hero"), Some(800), Some(600))
    );
}

#[tokio::test]
async fn umbraco_sample_end_to_end() {
    let fx = project(&[], vec![sample()]);
    let s = stage(&fx, Stage::Analyse).await.unwrap();
    assert_eq!(s.failed, 1, "{s:?}");
    let analysis: Value = serde_json::from_slice(
        &std::fs::read(fx.p.dir().join("raw/umbraco/analysis.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(analysis["items_by_type"]["hotelPage"], 1);
    assert_eq!(analysis["items_by_type"]["listingPage"], 1);
    stage(&fx, Stage::Extract).await.unwrap();
    assert!(fx.p.dir().join("raw/umbraco/export-0.json").exists());
    stage(&fx, Stage::Normalise).await.unwrap();

    let store = fx.p.store();
    // 1000, 1100 (en + de), 1200; the string in the array is not a node.
    let docs = store
        .items_decodable::<Document>(ItemKind::Document)
        .unwrap();
    assert_eq!(docs.len(), 4);
    assert_eq!(
        store.status(Stage::Normalise, raw("item-4")).unwrap(),
        Some(ItemStatus::Failed {
            error: "item 4 is not an object".into()
        })
    );
    let en = doc(&fx, "node:1100");
    assert_eq!(en.slug, "harbour");
    assert_eq!(en.parent, Some(canon("node:1000")));
    assert_eq!(en.status, DocStatus::Published);
    assert_eq!(en.fields["stars"], FieldValue::Number(4.0));
    assert_eq!(en.fields["tagline"], FieldValue::Text("By the sea".into()));
    assert!(en.updated.is_some());
    // The link in the RTE resolves to the listing document.
    let body = serde_json::to_string(&en.fields["body"]).unwrap();
    assert!(body.contains(&canon("node:1000").to_string()), "{body}");
    assert_eq!(doc(&fx, "node:1200").status, DocStatus::Draft);
    // Nothing could be downloaded (no site host): each picker value warns.
    assert!(issues(&fx, IssueKind::MissingMedia).len() >= 3);
    for (id, _, _) in &docs {
        assert!(store.status(Stage::Normalise, *id).unwrap().is_some());
    }
}

#[tokio::test]
async fn umbraco_stale_nodes_are_soft_deleted() {
    let fx = project(
        &[(
            "s.json",
            json!([
                { "id": 1, "name": "One", "contentTypeAlias": "page", "url": "/one/" },
                { "id": 2, "name": "Two", "contentTypeAlias": "page", "url": "/two/" },
            ]),
        )],
        vec![],
    );
    all(&fx).await;
    assert!(
        fx.p.store()
            .get_item::<Document>(canon("node:2"))
            .unwrap()
            .is_some()
    );
    std::fs::write(
        fx.p.dir().join("s.json"),
        serde_json::to_vec(
            &json!([{ "id": 1, "name": "One", "contentTypeAlias": "page", "url": "/one/" }]),
        )
        .unwrap(),
    )
    .unwrap();
    all(&fx).await;
    assert!(
        fx.p.store()
            .get_item::<Document>(canon("node:2"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fx.p.store()
            .status(Stage::Normalise, canon("node:2"))
            .unwrap(),
        Some(ItemStatus::Skipped {
            reason: "no longer in the source".into()
        })
    );
    assert!(
        fx.p.store()
            .get_item::<Document>(canon("node:1"))
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn umbraco_oversize_file_is_a_clear_error() {
    let fx = project(&[], vec![]);
    let big = fx.p.dir().join("big.json");
    std::fs::File::create(&big)
        .unwrap()
        .set_len((1 << 30) + 1)
        .unwrap();
    let cfg = {
        let mut c = fx.p.config().clone();
        c.connectors = vec![ConnectorConfig::UmbracoJson { paths: vec![big] }];
        c
    };
    std::fs::write(
        fx.p.dir().join("porter.toml"),
        toml::to_string(&cfg).unwrap(),
    )
    .unwrap();
    let fx = Fx {
        p: Arc::new(Project::open(fx.p.dir()).unwrap()),
        _d: fx._d,
    };
    let err = stage(&fx, Stage::Analyse).await.unwrap_err();
    assert!(format!("{err:#}").contains("1 GB"), "{err:#}");
}

/// Real client export, gitignored: skipped when absent.
#[tokio::test]
async fn umbraco_local_fixture() {
    // Repo root (cargo runs tests from the crate folder).
    let f = &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/hotels.json");
    if !f.exists() {
        eprintln!("skipping: ./test-data/hotels.json not present");
        return;
    }
    let fx = project(&[], vec![f.canonicalize().unwrap()]);
    all(&fx).await;
    let docs =
        fx.p.store()
            .items_decodable::<Document>(ItemKind::Document)
            .unwrap();
    assert!(!docs.is_empty(), "should have read at least one document");
}
