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
    RedirectOrigin, Severity, SourceRef, Stage,
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
                "content": "<p>See <a href=\"https://example.com/hello-world/\">hello</a> and <a href=\"https://example.com/five/\">five</a></p>",
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
            post(5, "page", "publish", "five", json!({})),
            post(25, "page", "publish", "photo", json!({
                "content": "<p><img src=\"https://example.com/wp-content/uploads/2024/05/big-123x45.jpg\"></p>",
            })),
            post(26, "wp_block", "publish", "reusable", json!({
                "content": "<!-- wp:paragraph --><p>Reusable text</p><!-- /wp:paragraph -->",
            })),
            post(27, "page", "publish", "uses-block", json!({
                "content": "<!-- wp:block {\"ref\":26} /-->",
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

async fn setup(redirects: &[(u64, &str, &str)]) -> Fx {
    setup_with(redirects, |_| {}).await
}

async fn normalise(fx: &Fx) {
    stage(fx, Stage::Normalise, |ctx| async move {
        WordPressSource.normalise(&ctx)
    })
    .await;
}

fn write_json(path: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(v).unwrap()).unwrap();
}

/// basic.xml extracted (page 2's raw content swapped for Divi), a REST record of page 2 with
/// rendered HTML only, a synthetic wp-db export (and its uploads folder) and Redirection rows
/// `(id, from, to)`; `tweak` gets the raw/ folder before normalise runs.
async fn setup_with(redirects: &[(u64, &str, &str)], tweak: impl FnOnce(&std::path::Path)) -> Fx {
    setup_full(redirects, tweak, false, |_| {}).await
}

/// `uploads_apart`: the uploads folder lives outside `wp_root`, named by `uploads_dir`.
async fn setup_full(
    redirects: &[(u64, &str, &str)],
    tweak: impl FnOnce(&std::path::Path),
    uploads_apart: bool,
    configure: impl FnOnce(&mut porter_engine::config::ProjectConfig),
) -> Fx {
    let d = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(d.path(), "t", "wordpress").unwrap());
    let root = d.path().join("wproot");
    std::fs::create_dir_all(&root).unwrap();
    let uploads = match uploads_apart {
        true => d.path().join("uploads-copy"),
        false => root.join("wp-content/uploads"),
    };
    let up = uploads.join("2024/03");
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
            uploads_dir: uploads_apart.then_some(uploads),
        },
        ConnectorConfig::WpRest {
            url: "https://example.com".into(),
            auth: None,
            keyring: None,
        },
    ];
    configure(&mut cfg);
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
        // Page 2: Divi in the WXR's raw content, only rendered HTML over REST.
        let item = ctx.project.raw_dir("wxr").join("items/2.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&item)?)?;
        v["content"] = json!("[et_pb_section][et_pb_row][et_pb_column type=\"4_4\"][et_pb_text]<p>Raw</p>[/et_pb_text][/et_pb_column][/et_pb_row][/et_pb_section]");
        std::fs::write(&item, serde_json::to_vec(&v)?)?;
        let rest = json!({
            "id": 2, "type": "page", "status": "publish", "slug": "sample-page",
            "link": "https://example.com/sample-page/", "title": {"rendered": "Sample Page"},
            "content": {"rendered": "<div class=\"et_pb_section\"><p>Rendered</p></div>"},
        });
        write_json(&ctx.project.raw_dir("wp-rest").join("pages/2.json"), &rest);
        let src = SourceRef {
            connector: "wp-rest".into(),
            source_id: "pages:2".into(),
            url: Some("https://example.com/sample-page/".into()),
        };
        let row = RawItem {
            label: "page".into(),
            title: Some("Sample Page".into()),
            status: Some("publish".into()),
            url: src.url.clone(),
            raw_path: Some("pages/2.json".into()),
        };
        let iid = ItemId::new(ItemKind::Document, &src);
        ctx.project
            .store()
            .put_item(ItemKind::Document, iid, &src, &row)?;
        ctx.item_status(iid, ItemKind::Document, ItemStatus::Done)?;
        // A file the crawl already downloaded: the scaled original of attachment 60.
        ctx.project.store().put_media_bytes(
            b"big-bytes",
            "https://example.com/wp-content/uploads/2024/05/big-scaled.jpg",
            "big-scaled.jpg",
        )?;
        Ok(())
    })
    .await;
    tweak(&fx.p.dir().join("raw"));
    normalise(&fx).await;
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
async fn uploads_dir_outside_wp_root() {
    let fx = setup_full(&[], |_| {}, true, |_| {}).await;
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
    let media: Media = fx.p.store().get_item(*m).unwrap().unwrap();
    assert_eq!(media.folder.as_deref(), Some("2024/03"));
    assert_eq!(
        std::fs::read(fx.p.store().media_path(*m).unwrap()).unwrap(),
        b"jpeg-a"
    );
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

#[tokio::test]
async fn rerun_soft_deletes_removed_posts() {
    let fx = setup(&[]).await;
    let five = canon(ItemKind::Document, "post:5");
    assert!(fx.p.store().get_item::<Document>(five).unwrap().is_some());
    let unresolved = |fx: &Fx| {
        issues(fx, IssueKind::UnresolvedLink)
            .iter()
            .any(|i| i.message.contains("/five/"))
    };
    assert!(!unresolved(&fx));
    // Post 5 leaves the source.
    let path = fx.p.raw_dir("wp-db").join("export.json");
    let mut v = export();
    v["posts"].as_array_mut().unwrap().retain(|p| p["id"] != 5);
    write_json(&path, &v);
    normalise(&fx).await;
    let s = fx.p.store();
    assert!(
        s.items_decodable::<Document>(ItemKind::Document)
            .unwrap()
            .iter()
            .all(|(id, _, _)| *id != five)
    );
    assert!(s.get_item::<Document>(five).unwrap().is_none());
    assert_eq!(
        s.status(Stage::Normalise, five).unwrap(),
        Some(ItemStatus::Skipped {
            reason: "no longer in the source".into()
        })
    );
    assert_eq!(s.lookup_url("https://example.com/five/").unwrap(), None);
    assert!(unresolved(&fx));
}

#[tokio::test]
async fn unlisted_size_resolves_to_original() {
    let fx = setup(&[]).await;
    let photo = doc(&fx, 25);
    let big =
        fx.p.store()
            .media_by_url("https://example.com/wp-content/uploads/2024/05/big.jpg")
            .unwrap()
            .unwrap();
    let json = serde_json::to_string(&photo.fields["content"]).unwrap();
    assert!(json.contains(&big.to_string()), "{json}");
    assert!(
        !issues(&fx, IssueKind::MissingMedia)
            .iter()
            .any(|i| i.item == Some(photo.id)),
    );
}

#[tokio::test]
async fn rendered_rest_does_not_beat_wxr_raw() {
    let fx = setup(&[]).await;
    match &doc(&fx, 2).fields["content"] {
        FieldValue::Layout(b) => {
            let json = serde_json::to_string(b).unwrap();
            assert!(json.contains("Raw") && !json.contains("Rendered"), "{json}");
        }
        other => panic!("{other:?}"),
    }
}

/// `[mapping.redirects] from_fields`: a live document's old URL field redirects to it; a
/// published claim beats a private one, other sites and drafts don't count.
#[tokio::test]
async fn old_url_field_becomes_redirect() {
    let fx = setup_full(
        &[],
        |raw| {
            let path = raw.join("wp-db/export.json");
            let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let posts = v["posts"].as_array_mut().unwrap();
            let old = |p: &mut Value, url: &str| p["acf"] = json!({ "old_url": url });
            for p in posts.iter_mut() {
                match p["id"].as_u64() {
                    Some(5) => old(p, "/en/five.html"),
                    Some(22) => old(p, "https://example.com/en/five.html"),
                    Some(24) => old(p, "https://elsewhere.test/divi/"),
                    Some(21) => old(p, "/en/drafty.html"),
                    _ => {}
                }
            }
            std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        },
        false,
        |cfg| cfg.mapping.redirects.from_fields = vec!["acf.old_url".into()],
    )
    .await;
    let reds: Vec<Redirect> =
        fx.p.store()
            .items_decodable::<Redirect>(ItemKind::Redirect)
            .unwrap()
            .into_iter()
            .map(|r| r.2)
            .filter(|r| r.origin == RedirectOrigin::Field)
            .collect();
    assert_eq!(reds.len(), 1, "{reds:?}");
    assert_eq!(reds[0].from, "/en/five.html");
    assert_eq!(reds[0].to, "https://example.com/five/");
}

/// On disk only `Photo.jpg`; the attachment asks for `photo.jpg`. A case-insensitive disk
/// would open the sibling: it must be a missing file, never the sibling's bytes as Done.
#[tokio::test]
async fn upload_read_needs_exact_case() {
    let fx = setup_with(&[], |raw| {
        let up = raw
            .parent()
            .unwrap()
            .join("wproot/wp-content/uploads/2024/03");
        std::fs::rename(up.join("a.jpg"), up.join("tmp.jpg")).unwrap();
        std::fs::rename(up.join("tmp.jpg"), up.join("Photo.jpg")).unwrap();
        let path = raw.join("wp-db/export.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // The sibling is an attachment of its own, read first: its URL must not stand in.
        v["posts"].as_array_mut().unwrap().push(post(
            49,
            "attachment",
            "inherit",
            "photo-cap",
            json!({
                "link": "https://example.com/wp-content/uploads/2024/03/Photo.jpg",
                "mime": "image/jpeg",
                "meta": { "_wp_attached_file": "2024/03/Photo.jpg" },
                "attachment": { "file": "2024/03/Photo.jpg", "metadata": { "sizes": {} } },
            }),
        ));
        for p in v["posts"].as_array_mut().unwrap() {
            if p["id"] == 50 {
                p["link"] = json!("https://example.com/wp-content/uploads/2024/03/photo.jpg");
                p["meta"]["_wp_attached_file"] = json!("2024/03/photo.jpg");
                p["attachment"]["file"] = json!("2024/03/photo.jpg");
            }
        }
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    })
    .await;
    let s = fx.p.store();
    // Only the sibling's own attachment holds its bytes.
    let holders: Vec<_> = s
        .items::<Media>(ItemKind::Media)
        .unwrap()
        .into_iter()
        .filter(|m| std::fs::read(s.media_path(m.0).unwrap()).unwrap() == b"jpeg-a")
        .collect();
    assert_eq!(holders.len(), 1);
    assert_eq!(holders[0].2.filename, "Photo.jpg");
    assert!(matches!(
        s.status(Stage::Normalise, canon(ItemKind::Document, "post:50"))
            .unwrap(),
        Some(ItemStatus::Skipped { .. })
    ));
    let miss = issues(&fx, IssueKind::MissingMedia);
    assert!(
        miss.iter().any(|i| i
            .message
            .contains("2024/03/photo.jpg not found with exact case (found Photo.jpg)")),
        "{miss:?}"
    );
}

/// Two attachments, one file, different alt text: one item, and an info issue says which
/// values were kept.
#[tokio::test]
async fn shared_file_attachments_reported() {
    let fx = setup_with(&[], |raw| {
        let up = raw.parent().unwrap().join("wproot/wp-content/uploads/2024/04");
        std::fs::create_dir_all(&up).unwrap();
        std::fs::write(up.join("copy.jpg"), b"jpeg-a").unwrap();
        let path = raw.join("wp-db/export.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["posts"].as_array_mut().unwrap().push(post(51, "attachment", "inherit", "copy", json!({
            "link": "https://example.com/wp-content/uploads/2024/04/copy.jpg",
            "mime": "image/jpeg",
            "meta": { "_wp_attached_file": "2024/04/copy.jpg", "_wp_attachment_image_alt": "Alt B" },
            "attachment": { "file": "2024/04/copy.jpg", "metadata": { "sizes": {} } },
        })));
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    })
    .await;
    let info: Vec<_> = issues(&fx, IssueKind::MissingMedia)
        .into_iter()
        .filter(|i| i.severity == Severity::Info && i.message.contains("share one media item"))
        .collect();
    assert_eq!(info.len(), 1, "{info:?}");
    assert!(info[0].message.contains("[50, 51]"), "{}", info[0].message);
}

#[tokio::test]
async fn post_shadowed_by_page_at_same_url_is_skipped() {
    let fx = setup_with(&[], |raw| {
        let path = raw.join("wp-db/export.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let posts = v["posts"].as_array_mut().unwrap();
        // Same URL as live page 5 ("five"); same URL as draft page 21 ("drafty").
        posts.push(post(40, "post", "publish", "five", json!({})));
        posts.push(post(41, "post", "publish", "drafty", json!({})));
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    })
    .await;
    let s = fx.p.store();
    let id = canon(ItemKind::Document, "post:40");
    assert!(s.get_item::<Document>(id).unwrap().is_none());
    match s.status(Stage::Normalise, id).unwrap() {
        Some(ItemStatus::Skipped { reason }) => assert!(reason.contains("page 5"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // A draft page doesn't shadow: WordPress serves the post there.
    assert_eq!(doc(&fx, 41).doc_type, "post");
    assert_eq!(doc(&fx, 5).doc_type, "page");
}

#[tokio::test]
async fn unreadable_export_is_an_error() {
    let fx = setup_with(&[], |raw| {
        std::fs::write(raw.join("wp-db/export.json"), b"{ not json").unwrap();
    })
    .await;
    let errs: Vec<_> = issues(&fx, IssueKind::AccessGap)
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(
        errs.iter()
            .any(|i| i.message.starts_with("wp-db export unreadable:")),
        "{errs:?}"
    );
    // The other connectors still normalised.
    assert!(matches!(
        doc(&fx, 1).fields.get("content"),
        Some(FieldValue::RichText(_))
    ));
}

#[tokio::test]
async fn internal_types_are_skipped() {
    let fx = setup(&[]).await;
    let block = canon(ItemKind::Document, "post:26");
    let s = fx.p.store();
    assert!(s.get_item::<Document>(block).unwrap().is_none());
    assert_eq!(
        s.status(Stage::Normalise, block).unwrap(),
        Some(ItemStatus::Skipped {
            reason: "internal WordPress type wp_block".into()
        })
    );
    assert_eq!(s.lookup_url("https://example.com/reusable/").unwrap(), None);
    // Still expanded where it is used.
    let json = serde_json::to_string(&doc(&fx, 27).fields["content"]).unwrap();
    assert!(json.contains("Reusable text"), "{json}");
}

/// A symlink in the uploads folder is never followed: the attachment is missing media.
#[tokio::test]
async fn uploads_symlink_not_followed() {
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret");
    std::fs::write(&secret, b"secret-bytes").unwrap();
    let fx = setup_full(
        &[],
        |_| {},
        false,
        |cfg| {
            let Some(ConnectorConfig::WpDb {
                wp_root: Some(root),
                ..
            }) = cfg.connectors.get(1)
            else {
                panic!("no wp-db connector")
            };
            let a = root.join("wp-content/uploads/2024/03/a.jpg");
            std::fs::remove_file(&a).unwrap();
            std::os::unix::fs::symlink(&secret, &a).unwrap();
        },
    )
    .await;
    let about = doc(&fx, 20);
    let FieldValue::List(rows) = &about.fields["acf.slides"] else {
        panic!("{:?}", about.fields["acf.slides"]);
    };
    let FieldValue::Object(row) = &rows[0] else {
        panic!("{rows:?}");
    };
    assert!(
        !matches!(row.get("image"), Some(FieldValue::Media(_))),
        "{row:?}"
    );
    let missing = issues(&fx, IssueKind::MissingMedia);
    assert!(
        missing.iter().any(
            |i| i.message.contains("2024/03/a.jpg") && i.message.contains("not a regular file")
        ),
        "{missing:#?}"
    );
}

/// An entry whose date doesn't parse keeps no date (never 1970) and says so.
#[tokio::test]
async fn submission_bad_date_is_none_with_issue() {
    let fx = setup_with(&[], |raw| {
        std::fs::write(
            raw.join("wp-db/submissions.ndjson"),
            "{\"source\":\"gf\",\"id\":7,\"form_id\":3,\"date\":\"garbage\",\"fields\":{\"1\":\"x\"}}\n",
        )
        .unwrap();
    })
    .await;
    let id = canon(ItemKind::Submission, "submission:gf:7");
    let sub: Value =
        fx.p.store()
            .get_item(id)
            .unwrap()
            .expect("submission stored");
    assert!(sub["created"].is_null(), "{sub}");
    let all =
        fx.p.store()
            .issues(&IssueFilter {
                stage: Some(Stage::Normalise),
                ..Default::default()
            })
            .unwrap();
    assert!(
        all.iter().any(|i| i.item == Some(id)
            && i.severity == Severity::Info
            && i.message.contains("garbage")),
        "{all:#?}"
    );
}

/// A post whose row failed extract this run wasn't removed from the source: the stale
/// canonical item says so instead of "no longer in the source".
#[tokio::test]
async fn failed_extract_is_not_removal() {
    let fx = setup(&[]).await;
    let five = canon(ItemKind::Document, "post:5");
    let path = fx.p.raw_dir("wp-db").join("export.json");
    let mut v = export();
    v["posts"].as_array_mut().unwrap().retain(|p| p["id"] != 5);
    write_json(&path, &v);
    let row = ItemId::new(
        ItemKind::Document,
        &SourceRef {
            connector: "wp-db".into(),
            source_id: "post:5".into(),
            url: None,
        },
    );
    let s = fx.p.store();
    let run = s.begin_run(Stage::Extract).unwrap();
    s.set_status(
        run,
        Stage::Extract,
        row,
        &ItemStatus::Failed {
            error: "unreadable".into(),
        },
    )
    .unwrap();
    normalise(&fx).await;
    assert_eq!(
        s.status(Stage::Normalise, five).unwrap(),
        Some(ItemStatus::Skipped {
            reason: "not produced this run (see extract issues)".into()
        })
    );
}

/// The media store's URL table says which file a URL is now; a media item's own variants
/// list keeps every URL it ever had, so it must not decide.
#[tokio::test]
async fn media_index_follows_the_url_table() {
    let fx = setup(&[]).await;
    let s = fx.p.store();
    let x = "https://example.com/wp-content/uploads/2024/05/big-scaled.jpg";
    let a = s.media_by_url(x).unwrap().unwrap();
    // The file at x changed (a later extract), then changed back.
    let b = s
        .put_media_bytes(b"big-bytes-v2", x, "big-scaled.jpg")
        .unwrap()
        .id;
    normalise(&fx).await;
    assert_eq!(
        s.put_media_bytes(b"big-bytes", x, "big-scaled.jpg")
            .unwrap()
            .id,
        a
    );
    assert_ne!(a, b);
    normalise(&fx).await;
    let big = "https://example.com/wp-content/uploads/2024/05/big.jpg";
    assert_eq!(s.media_by_url(big).unwrap(), Some(a));
}
