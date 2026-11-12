use porter_engine::config::{ConnectorConfig, ProjectConfig};
use porter_engine::connectors::RawItem;
use porter_engine::events::EventSink;
use porter_engine::project::Project;
use porter_engine::sources::wordpress::WordPressSource;
use porter_engine::sources::wordpress::db::record_export;
use porter_engine::sources::wordpress::wxr::WxrConnector;
use porter_engine::stage::{Connector, Source, run_stage};
use porter_engine::store::IssueFilter;
use porter_model::{
    DocStatus, Document, FieldValue, IssueKind, ItemId, ItemKind, ItemStatus, Media, Redirect,
    Severity, SourceRef, Stage,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct Fx {
    _d: tempfile::TempDir,
    p: Arc<Project>,
    cfg: ProjectConfig,
}

fn wxr_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wxr/basic.xml")
}

fn post(id: u64, ty: &str, status: &str, slug: &str, extra: Value) -> Value {
    let mut p = json!({
        "id": id, "type": ty, "status": status, "slug": slug,
        "title": slug.to_uppercase(), "link": format!("https://example.com/{slug}/"),
        "parent": null, "menu_order": 0, "author": 1,
        "date_gmt": "2024-02-01 10:00:00", "modified_gmt": "2024-02-02 10:00:00",
        "content": "<p>Text</p>", "excerpt": "", "meta": {}, "acf": null, "terms": {},
    });
    if let (Some(o), Value::Object(e)) = (p.as_object_mut(), extra) {
        o.extend(e);
    }
    p
}

fn export() -> Value {
    json!({
        "site": {
            "home": "https://example.com", "siteurl": "https://example.com", "blogname": "Site",
            "page_on_front": 20, "permalink_structure": "/%postname%/",
            "wpseo_titles": { "separator": "sc-ndash" },
        },
        "post_types": {},
        "posts": [
            post(20, "page", "publish", "about", json!({
                "title": "About",
                "content": "<p>See <a href=\"https://example.com/hello-world/\">hello</a></p>",
                "meta": {
                    "_yoast_wpseo_title": "%%title%% %%sep%% %%sitename%%",
                    "_yoast_wpseo_metadesc": "About us %%bogus%%",
                },
                "acf": { "slides": [ { "image": 50, "caption": "First" } ] },
            })),
            post(21, "page", "draft", "drafty", json!({})),
            post(22, "page", "private", "secret", json!({})),
            post(23, "post", "future", "soon", json!({})),
            post(24, "page", "publish", "divi", json!({
                "content": "[et_pb_section][et_pb_row][et_pb_column type=\"4_4\"][et_pb_text]<p>Hi</p>[/et_pb_text][/et_pb_column][/et_pb_row][/et_pb_section]",
            })),
            post(30, "page", "publish", "loop-a", json!({ "parent": 31 })),
            post(31, "page", "publish", "loop-b", json!({ "parent": 30 })),
            post(50, "attachment", "inherit", "a", json!({
                "link": "https://example.com/wp-content/uploads/2024/03/a.jpg",
                "mime": "image/jpeg",
                "meta": { "_wp_attached_file": "2024/03/a.jpg", "_wp_attachment_image_alt": "Alt A" },
                "attachment": { "file": "2024/03/a.jpg", "metadata": { "width": 800, "height": 600, "sizes": {} } },
            })),
            post(60, "attachment", "inherit", "big", json!({
                "link": "https://example.com/wp-content/uploads/2024/05/big-scaled.jpg",
                "mime": "image/jpeg",
                "meta": { "_wp_attached_file": "2024/05/big-scaled.jpg" },
                "attachment": { "file": "2024/05/big-scaled.jpg", "metadata": {
                    "width": 2560, "height": 1700, "original_image": "big.jpg",
                    "sizes": { "medium": { "file": "big-300x200.jpg", "width": 300, "height": 200 } },
                } },
            })),
            post(61, "attachment", "inherit", "gone", json!({
                "link": "https://example.com/wp-content/uploads/2024/05/gone.jpg",
                "attachment": { "file": "2024/05/gone.jpg", "metadata": null },
            })),
        ],
        "taxonomies": {},
        "terms": [
            { "id": 1, "taxonomy": "category", "slug": "uncategorized", "name": "Uncategorized", "parent": null },
        ],
        "users": [ { "id": 1, "login": "admin", "nicename": "admin", "display_name": "Admin", "email": "admin@example.com" } ],
        "menus": [ { "id": 5, "name": "Main", "slug": "main", "items": [
            { "id": 90, "parent": null, "order": 1, "title": "About", "url": "https://example.com/about/", "type": "post_type", "object": "page", "object_id": 20 },
            { "id": 91, "parent": null, "order": 2, "title": "Elsewhere", "url": "https://other.test/", "type": "custom", "object": "custom", "object_id": 91 },
        ] } ],
        "menu_locations": {},
        "acf_field_groups": [ { "key": "group_1", "title": "Page", "fields": [
            { "key": "field_1", "name": "slides", "type": "repeater", "sub_fields": [
                { "key": "field_2", "name": "image", "type": "image" },
                { "key": "field_3", "name": "caption", "type": "text" },
            ] },
        ] } ],
        "forms": { "gf": [], "nf": [] },
        "translations": null,
    })
}

async fn stage<F, Fut>(fx: &Fx, stage: Stage, f: F) -> porter_engine::events::Summary
where
    F: FnOnce(porter_engine::stage::StageCtx) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    run_stage(
        fx.p.clone(),
        Arc::new(fx.cfg.clone()),
        EventSink::new(4096),
        CancellationToken::new(),
        stage,
        f,
    )
    .await
    .unwrap()
}

/// basic.xml extracted, plus a synthetic wp-db export (and its uploads folder) and Redirection
/// rows `(id, from, to)`; then normalise.
async fn setup(redirects: &[(u64, &str, &str)]) -> Fx {
    let d = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(d.path(), "t", "wordpress").unwrap());
    let root = d.path().join("wproot");
    let up = root.join("wp-content/uploads/2024/03");
    std::fs::create_dir_all(&up).unwrap();
    std::fs::write(up.join("a.jpg"), b"jpeg-a").unwrap();
    let mut cfg = p.config().clone();
    cfg.connectors = vec![
        ConnectorConfig::Wxr {
            path: wxr_fixture(),
        },
        ConnectorConfig::WpDb {
            dump: "dump.sql".into(),
            wp_root: Some(root),
            table_prefix: "wp_".into(),
            site_url: None,
        },
    ];
    let fx = Fx { _d: d, p, cfg };
    let wxr = WxrConnector::new(&fx.cfg.connectors[0]);
    stage(&fx, Stage::Extract, |ctx| async move {
        wxr.extract(&ctx).await?;
        let dir = ctx.project.raw_dir("wp-db");
        std::fs::create_dir_all(&dir)?;
        let v = export();
        std::fs::write(dir.join("export.json"), serde_json::to_vec(&v)?)?;
        record_export(&ctx, &v)?;
        let mut nd = String::new();
        for (id, from, to) in redirects {
            let row = json!({
                "id": id, "url": from, "match_type": "url", "regex": 0, "action_type": "url",
                "action_code": 301, "action_data": to, "status": "enabled",
            });
            nd.push_str(&format!("{row}\n"));
            let src = SourceRef {
                connector: "wp-db".into(),
                source_id: format!("redirect:{id}"),
                url: None,
            };
            let raw = RawItem {
                label: "redirect".into(),
                title: Some(from.to_string()),
                status: Some("enabled".into()),
                url: None,
                raw_path: Some("redirection.ndjson".into()),
            };
            let iid = ItemId::new(ItemKind::Redirect, &src);
            ctx.project
                .store()
                .put_item(ItemKind::Redirect, iid, &src, &raw)?;
            ctx.item_status(iid, ItemKind::Redirect, ItemStatus::Done)?;
        }
        std::fs::write(dir.join("redirection.ndjson"), nd)?;
        // A file the crawl already downloaded: the scaled original of attachment 60.
        ctx.project.store().put_media_bytes(
            b"big-bytes",
            "https://example.com/wp-content/uploads/2024/05/big-scaled.jpg",
            "big-scaled.jpg",
        )?;
        Ok(())
    })
    .await;
    stage(&fx, Stage::Normalise, |ctx| async move {
        WordPressSource.normalise(&ctx)
    })
    .await;
    fx
}

fn canon(kind: ItemKind, sid: &str) -> ItemId {
    ItemId::new(
        kind,
        &SourceRef {
            connector: "wordpress".into(),
            source_id: sid.into(),
            url: None,
        },
    )
}

fn doc(fx: &Fx, wp: u64) -> Document {
    fx.p.store()
        .get_item(canon(ItemKind::Document, &format!("post:{wp}")))
        .unwrap()
        .unwrap_or_else(|| panic!("no document for post {wp}"))
}

fn issues(fx: &Fx, kind: IssueKind) -> Vec<porter_model::Issue> {
    fx.p.store()
        .issues(&IssueFilter {
            kind: Some(kind),
            stage: Some(Stage::Normalise),
            ..Default::default()
        })
        .unwrap()
}

fn redirects(fx: &Fx) -> Vec<Redirect> {
    fx.p.store()
        .items_decodable::<Redirect>(ItemKind::Redirect)
        .unwrap()
        .into_iter()
        .filter(|(_, s, _)| s.connector == "wordpress")
        .map(|(_, _, r)| r)
        .collect()
}

#[tokio::test]
async fn post_statuses_mapped() {
    let fx = setup(&[]).await;
    for (id, want) in [
        (20, DocStatus::Published),
        (21, DocStatus::Draft),
        (22, DocStatus::Private),
        (23, DocStatus::Scheduled),
    ] {
        assert_eq!(doc(&fx, id).status, want, "post {id}");
    }
    let about = doc(&fx, 20);
    assert_eq!(about.doc_type, "page");
    assert_eq!(about.slug, "about");
    assert_eq!(about.path, "/about/");
    assert_eq!(about.author, Some(canon(ItemKind::Author, "author:admin")));
}

#[tokio::test]
async fn divi_page_becomes_layout() {
    let fx = setup(&[]).await;
    assert!(
        matches!(doc(&fx, 24).fields.get("content"), Some(FieldValue::Layout(b)) if !b.is_empty()),
        "{:?}",
        doc(&fx, 24).fields.get("content")
    );
}

#[tokio::test]
async fn classic_post_becomes_richtext() {
    let fx = setup(&[]).await;
    // Post 1 comes from the WXR export only.
    let hello = doc(&fx, 1);
    match hello.fields.get("content") {
        Some(FieldValue::RichText(rt)) => {
            assert!(rt.plain_text().starts_with("Welcome to WordPress"))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        hello.fields.get("terms.category"),
        Some(&FieldValue::List(vec![FieldValue::Doc(canon(
            ItemKind::Term,
            "term:category:1"
        ))]))
    );
    // The link in About resolves to the canonical post, not a connector row.
    let about = doc(&fx, 20);
    let json = serde_json::to_string(&about.fields["content"]).unwrap();
    assert!(json.contains(&hello.id.to_string()), "{json}");
}

#[tokio::test]
async fn acf_repeater_typed() {
    let fx = setup(&[]).await;
    let about = doc(&fx, 20);
    let FieldValue::List(rows) = &about.fields["acf.slides"] else {
        panic!("{:?}", about.fields["acf.slides"]);
    };
    let FieldValue::Object(row) = &rows[0] else {
        panic!("{rows:?}");
    };
    let Some(FieldValue::Media(m)) = row.get("image") else {
        panic!("{row:?}");
    };
    assert_eq!(row.get("caption"), Some(&FieldValue::Text("First".into())));
    // The file came from wp_root's uploads folder.
    let media: Media = fx.p.store().get_item(*m).unwrap().unwrap();
    assert_eq!(media.alt.as_deref(), Some("Alt A"));
    assert_eq!(media.folder.as_deref(), Some("2024/03"));
}

#[tokio::test]
async fn yoast_vars_substituted() {
    let fx = setup(&[]).await;
    let about = doc(&fx, 20);
    assert_eq!(about.seo.title.as_deref(), Some("About – Site"));
    assert_eq!(about.seo.description.as_deref(), Some("About us"));
    assert!(
        issues(&fx, IssueKind::UnmappedField)
            .iter()
            .any(|i| i.severity == Severity::Info && i.message.contains("%%bogus%%"))
    );
}

#[tokio::test]
async fn redirect_chain_collapsed() {
    let fx = setup(&[(1, "/a/", "/b/"), (2, "/b/", "/c/")]).await;
    let rs = redirects(&fx);
    let a = rs.iter().find(|r| r.from.ends_with("/a/")).expect("a kept");
    assert!(a.to.ends_with("/c/"), "{a:?}");
    assert!(
        issues(&fx, IssueKind::RedirectChain)
            .iter()
            .any(|i| i.severity == Severity::Info)
    );
}

#[tokio::test]
async fn redirect_loop_dropped() {
    let fx = setup(&[(1, "/a/", "/b/"), (2, "/b/", "/a/")]).await;
    assert!(redirects(&fx).is_empty(), "{:?}", redirects(&fx));
    assert!(
        issues(&fx, IssueKind::RedirectLoop)
            .iter()
            .any(|i| i.severity == Severity::Warning)
    );
}

#[tokio::test]
async fn parent_cycle_is_issue() {
    let fx = setup(&[]).await;
    let missing = issues(&fx, IssueKind::ParentMissing);
    for id in [30, 31] {
        let d = doc(&fx, id);
        assert_eq!(d.parent, None, "post {id}");
        assert!(missing.iter().any(|i| i.item == Some(d.id)), "post {id}");
    }
}

#[tokio::test]
async fn media_variants_registered() {
    let fx = setup(&[]).await;
    let s = fx.p.store();
    let id = s
        .media_by_url("https://example.com/wp-content/uploads/2024/05/big-scaled.jpg")
        .unwrap()
        .unwrap();
    for u in [
        "https://example.com/wp-content/uploads/2024/05/big-300x200.jpg",
        "https://example.com/wp-content/uploads/2024/05/big.jpg",
        "http://www.example.com/wp-content/uploads/2024/05/BIG.jpg",
    ] {
        assert_eq!(s.media_by_url(u).unwrap(), Some(id), "{u}");
    }
    // An attachment with no file anywhere is skipped, with a missing-media warning.
    let gone = canon(ItemKind::Document, "post:61");
    assert!(matches!(
        s.status(Stage::Normalise, gone).unwrap(),
        Some(ItemStatus::Skipped { .. })
    ));
    assert!(
        issues(&fx, IssueKind::MissingMedia)
            .iter()
            .any(|i| i.item == Some(gone))
    );
}

#[tokio::test]
async fn every_item_has_status() {
    let fx = setup(&[(1, "/a/", "/b/")]).await;
    let s = fx.p.store();
    let prov: HashMap<ItemId, ItemId> = serde_json::from_slice(
        &std::fs::read(fx.p.raw_dir("normalise").join("provenance.json")).unwrap(),
    )
    .unwrap();
    let rows = s.item_rows().unwrap();
    assert!(rows.len() > 10);
    for (id, kind, connector) in rows {
        let target = match connector.as_str() {
            "wordpress" | "media" => id,
            _ => prov.get(&id).copied().unwrap_or(id),
        };
        assert!(
            s.status(Stage::Normalise, target).unwrap().is_some(),
            "{kind} {id} from {connector} has no normalise status"
        );
    }
    // Menus, terms, settings, front page.
    let settings: porter_model::Settings = s
        .get_item(canon(ItemKind::Settings, "settings"))
        .unwrap()
        .unwrap();
    assert_eq!(settings.front_page, Some(doc(&fx, 20).id));
    let menu: porter_model::Menu = s
        .get_item(canon(ItemKind::Menu, "menu:5"))
        .unwrap()
        .unwrap();
    assert_eq!(
        menu.items[0].target,
        porter_model::MenuTarget::Doc(doc(&fx, 20).id)
    );
    assert!(matches!(
        &menu.items[1].target,
        porter_model::MenuTarget::Url(u) if u == "https://other.test/"
    ));
}
