use porter_engine::project::Project;
use porter_engine::session::{ItemQuery, Page, RunPlan, Session, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::{ItemId, ItemKind, SourceRef, Stage};
use std::path::Path;
use std::time::Duration;

/// A project reading the WXR fixture; `dest` is spliced in after `[destination]`.
fn wxr_project(dest: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    Project::init(d.path(), "t", "wordpress").unwrap();
    let wxr = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wxr/basic.xml");
    let toml = d.path().join("porter.toml");
    let text = std::fs::read_to_string(&toml)
        .unwrap()
        .replace("[destination]\n", &format!("[destination]\n{dest}"));
    std::fs::write(
        &toml,
        format!(
            "{text}\n[[connectors]]\nkind = \"wxr\"\npath = {:?}\n",
            wxr.display().to_string()
        ),
    )
    .unwrap();
    d
}

fn analyse() -> RunPlan {
    RunPlan::Stage(
        Stage::Analyse,
        StageOpts {
            refresh: false,
            apply: false,
            schema_mode: SchemaMode::Check,
        },
    )
}

#[tokio::test]
async fn session_runs_one_stage_at_a_time() {
    let d = wxr_project("");
    let s = Session::open(d.path()).unwrap();
    let h = s.run(analyse()).unwrap();
    assert!(s.running().is_some());
    let err = s.run(analyse()).err().expect("second run refused");
    assert!(err.to_string().contains("already running"), "{err}");
    let out = h.wait().await;
    assert_eq!(out.code, 0, "{out:?}");
    assert!(s.running().is_none());
    // Free again once the first run is done.
    s.run(analyse()).unwrap().wait().await;
}

#[tokio::test]
async fn session_cancel_gives_code_3() {
    let d = wxr_project("");
    let s = Session::open(d.path()).unwrap();
    let h = s.run(analyse()).unwrap();
    h.cancel();
    let out = tokio::time::timeout(Duration::from_secs(30), h.wait())
        .await
        .unwrap();
    assert_eq!(out.code, 3, "{out:?}");
}

#[tokio::test]
async fn session_all_stops_at_first_failure() {
    // Destination named but missing its url / key / auth collection.
    let d = wxr_project("adapter = \"payload\"\n");
    let s = Session::open(d.path()).unwrap();
    let out = s
        .run(RunPlan::All {
            keep_going: false,
            apply: true,
        })
        .unwrap()
        .wait()
        .await;
    assert_eq!(out.code, 1, "{out:?}");
    let last = out.per_stage.last().unwrap();
    assert_eq!((last.0, last.1), (Stage::Schema, 1), "{out:?}");
    assert!(last.2.as_deref().unwrap().contains("destination.url"));
    assert!(!out.per_stage.iter().any(|p| p.0 == Stage::Verify));
    assert!(!out.per_stage.iter().any(|p| p.0 == Stage::Load));
}

/// The first Run all seeds `[mapping]` after analyse, and the stages after it run with the
/// seed: nothing reads as stale once it ends, and schema and load recorded the seeded
/// mapping (the paths it adds are what load renders).
#[tokio::test]
async fn first_run_all_is_not_stale() {
    use porter_engine::config::ProjectConfig;
    use porter_engine::fingerprint::changed;
    use porter_engine::session::stage_view;
    // A destination nothing answers at: schema and load start (and record), then fail.
    let d = wxr_project(
        "adapter = \"payload\"\nurl = \"http://127.0.0.1:9\"\napi_key_env = \"PORTER_TEST_NO_SUCH_KEY\"\nauth_collection = \"users\"\n",
    );
    let s = Session::open(d.path()).unwrap();
    let out = s
        .run(RunPlan::All {
            keep_going: true,
            apply: false,
        })
        .unwrap()
        .wait()
        .await;
    assert!(out.seeded, "{out:?}");
    let ov = s.overview().await.unwrap();
    for stage in [Stage::Analyse, Stage::Extract, Stage::Normalise] {
        let v = stage_view(&ov, stage);
        assert!(v.last_run.is_some(), "{stage:?} ran: {out:?}");
        assert_eq!(v.stale, None, "{stage:?}");
    }
    let cfg = ProjectConfig::load(&d.path().join("porter.toml")).unwrap();
    assert!(!cfg.mapping.types.is_empty(), "seeded");
    for stage in [Stage::Schema, Stage::Load] {
        let run = s.project().store().runs(stage).unwrap().pop();
        let run = run.unwrap_or_else(|| panic!("{stage:?} started: {out:?}"));
        assert_eq!(
            changed(run.config_fp.as_deref(), &cfg, stage),
            None,
            "{stage:?} ran with the seeded mapping"
        );
    }
}

#[tokio::test]
async fn items_paged() {
    let d = tempfile::tempdir().unwrap();
    {
        let p = Project::init(d.path(), "t", "wordpress").unwrap();
        for n in 0..1200 {
            let src = SourceRef {
                connector: "c".into(),
                source_id: format!("{n}"),
                url: None,
            };
            let id = ItemId::new(ItemKind::Document, &src);
            p.store()
                .put_item(
                    ItemKind::Document,
                    id,
                    &src,
                    &serde_json::json!({"title": format!("T{n}")}),
                )
                .unwrap();
        }
    }
    let s = Session::open(d.path()).unwrap();
    let page = s
        .items(
            ItemQuery::default(),
            Page {
                offset: 0,
                limit: 500,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 500);
    assert_eq!(page.total, 1200);
    let last = s
        .items(
            ItemQuery::default(),
            Page {
                offset: 1000,
                limit: 500,
            },
        )
        .await
        .unwrap();
    assert_eq!(last.rows.len(), 200);
    assert_eq!(last.rows[0].title.as_deref(), Some("T1000"));
}

fn doc(
    connector: &str,
    sid: &str,
    ty: &str,
    title: &str,
    parent: Option<ItemId>,
) -> (ItemId, SourceRef, porter_model::Document) {
    let src = SourceRef {
        connector: connector.into(),
        source_id: sid.into(),
        url: Some(format!("https://old.test/{sid}")),
    };
    let id = ItemId::new(ItemKind::Document, &src);
    let d = porter_model::Document {
        id,
        doc_type: ty.into(),
        status: porter_model::DocStatus::Published,
        locale: None,
        translations: vec![],
        slug: sid.into(),
        path: format!("/{sid}"),
        parent,
        title: title.into(),
        fields: Default::default(),
        seo: Default::default(),
        created: None,
        updated: None,
        author: None,
        original: serde_json::json!({}),
        source: src.clone(),
    };
    (id, src, d)
}

/// The content tree's paged query: 1,200 normalised documents, nested three deep, plus
/// rows it must leave out (a connector's raw row, a deleted document).
#[tokio::test]
async fn children_pages_the_document_tree() {
    use porter_engine::session::DocNode;
    use porter_model::ItemStatus;
    let d = tempfile::tempdir().unwrap();
    Project::init(d.path(), "t", "wordpress").unwrap();
    let s = Session::open(d.path()).unwrap();
    let store = s.project().store();
    let put = |(id, src, doc): (ItemId, SourceRef, porter_model::Document)| {
        store.put_item(ItemKind::Document, id, &src, &doc).unwrap();
        id
    };
    let roots: Vec<ItemId> = (0..999)
        .map(|i| {
            put(doc(
                "wordpress",
                &format!("p{i}"),
                "page",
                &format!("Page {i:03}"),
                None,
            ))
        })
        .collect();
    let gone = SourceRef {
        connector: "wordpress".into(),
        source_id: "missing".into(),
        url: None,
    };
    let orphan = put(doc(
        "wordpress",
        "orphan",
        "page",
        "Zz orphan",
        Some(ItemId::new(ItemKind::Document, &gone)),
    ));
    let kids: Vec<ItemId> = (0..150)
        .map(|i| {
            put(doc(
                "wordpress",
                &format!("c{i}"),
                "page",
                &format!("Child {i:03}"),
                Some(roots[0]),
            ))
        })
        .collect();
    for i in 0..45 {
        put(doc(
            "wordpress",
            &format!("g{i}"),
            "page",
            &format!("Grand {i:02}"),
            Some(kids[0]),
        ));
    }
    for i in 0..5 {
        put(doc(
            "wordpress",
            &format!("n{i}"),
            "post",
            &format!("Post {i}"),
            None,
        ));
    }
    // Not canonical, or not live: never in the tree.
    let raw = put(doc("wp-rest", "p0", "page", "Raw row", None));
    let deleted = put(doc("wordpress", "deleted", "page", "Deleted", None));
    store.soft_delete(deleted).unwrap();
    let run = store.begin_run(Stage::Load).unwrap();
    store
        .set_status(run, Stage::Extract, kids[0], &ItemStatus::Done)
        .unwrap();
    store
        .set_status(
            run,
            Stage::Load,
            kids[0],
            &ItemStatus::Failed { error: "x".into() },
        )
        .unwrap();
    store
        .set_status(run, Stage::Extract, roots[0], &ItemStatus::Done)
        .unwrap();

    assert_eq!(
        s.doc_types().await.unwrap(),
        [("page".to_string(), 1195), ("post".to_string(), 5)]
    );

    let page = |offset| Page { offset, limit: 200 };
    let first = s.children(None, Some("page"), page(0)).await.unwrap();
    assert_eq!(first.total, 1000);
    assert_eq!(first.rows.len(), 200);
    assert_eq!(first.rows[0].title, "Page 000");
    assert_eq!(first.rows[0].path, "/p0");
    assert_eq!(first.rows[0].doc_type, "page");
    assert!(first.rows[0].has_children);
    assert_eq!(first.rows[0].worst_status, Some(ItemStatus::Done));
    assert!(!first.rows[1].has_children);
    assert_eq!(first.rows[1].worst_status, None);
    // Every root exactly once across the pages, the orphan included.
    let mut all: Vec<DocNode> = first.rows;
    for off in [200, 400, 600, 800] {
        all.extend(
            s.children(None, Some("page"), page(off))
                .await
                .unwrap()
                .rows,
        );
    }
    assert!(
        s.children(None, Some("page"), page(1000))
            .await
            .unwrap()
            .rows
            .is_empty()
    );
    let ids: std::collections::HashSet<ItemId> = all.iter().map(|n| n.id).collect();
    assert_eq!(ids.len(), 1000);
    assert!(ids.contains(&orphan));
    assert!(!ids.contains(&raw) && !ids.contains(&deleted));

    let c = s.children(Some(roots[0]), None, page(0)).await.unwrap();
    assert_eq!((c.total, c.rows.len()), (150, 150));
    assert_eq!(c.rows[0].id, kids[0]);
    assert!(c.rows[0].has_children);
    assert_eq!(
        c.rows[0].worst_status,
        Some(ItemStatus::Failed { error: "x".into() })
    );
    let g = s.children(Some(kids[0]), None, page(0)).await.unwrap();
    assert_eq!(g.total, 45);
    assert!(g.rows.iter().all(|n| !n.has_children));
    assert_eq!(
        s.children(None, Some("post"), page(0)).await.unwrap().total,
        5
    );
}

/// The media grid's paged query: canonical, live media only, by filename.
#[tokio::test]
async fn media_pages_canonical_live_only() {
    use porter_model::Media;
    let d = tempfile::tempdir().unwrap();
    Project::init(d.path(), "t", "wordpress").unwrap();
    let s = Session::open(d.path()).unwrap();
    let store = s.project().store();
    let put = |connector: &str, sid: &str, name: &str| {
        let src = SourceRef {
            connector: connector.into(),
            source_id: sid.into(),
            url: None,
        };
        let id = ItemId::new(ItemKind::Media, &src);
        let m = Media {
            id,
            hash: sid.into(),
            variants: vec![
                "https://old.test/a.png".into(),
                "https://old.test/b.png".into(),
            ],
            filename: name.into(),
            mime: "image/png".into(),
            width: Some(4),
            height: Some(3),
            alt: None,
            caption: None,
            title: None,
            credit: None,
            folder: None,
            usages: vec![],
        };
        store.put_item(ItemKind::Media, id, &src, &m).unwrap();
        id
    };
    for i in 0..5 {
        put("media", &format!("h{i}"), &format!("pic{i}.png"));
    }
    put("crawl", "raw", "raw.png");
    let gone = put("media", "gone", "gone.png");
    store.soft_delete(gone).unwrap();
    let a = s
        .media(Page {
            offset: 0,
            limit: 3,
        })
        .await
        .unwrap();
    assert_eq!((a.total, a.rows.len()), (5, 3));
    assert_eq!(a.rows[0].filename, "pic0.png");
    assert_eq!(a.rows[0].hash, "h0");
    assert_eq!(a.rows[0].variants, 2);
    assert_eq!(a.rows[0].mime, "image/png");
    let b = s
        .media(Page {
            offset: 3,
            limit: 3,
        })
        .await
        .unwrap();
    assert_eq!(b.rows.len(), 2);
    assert!(
        b.rows
            .iter()
            .all(|r| r.filename != "raw.png" && r.filename != "gone.png")
    );
}

/// The URL table: old URL, status, target and new URL, with the unresolved, chain and
/// 404 filters done in SQL and counted honestly.
#[tokio::test]
async fn routes_filter_and_page() {
    use porter_engine::session::RouteFilter;
    use porter_model::{IssueKind, Redirect, RedirectOrigin, Route, RouteTarget, Severity};
    let d = tempfile::tempdir().unwrap();
    Project::init(d.path(), "t", "wordpress").unwrap();
    let s = Session::open(d.path()).unwrap();
    let store = s.project().store();
    let src = |kind: &str, sid: &str, url: Option<&str>| SourceRef {
        connector: "wordpress".into(),
        source_id: format!("{kind}:{sid}"),
        url: url.map(Into::into),
    };
    let route = |sid: &str, status: Option<u16>, target: RouteTarget| {
        let url = format!("https://old.test/{sid}");
        let sr = src("route", sid, Some(&url));
        let id = ItemId::new(ItemKind::Route, &sr);
        let r = Route {
            url,
            target,
            http_status: status,
            title: None,
        };
        store.put_item(ItemKind::Route, id, &sr, &r).unwrap();
        id
    };
    let (doc, _) = {
        let sr = src("post", "1", None);
        let id = ItemId::new(ItemKind::Document, &sr);
        let mut v = serde_json::json!({"path": "/new-1", "title": "One", "type": "post"});
        v["id"] = serde_json::json!(id);
        store.put_item(ItemKind::Document, id, &sr, &v).unwrap();
        (id, ())
    };
    let redirect = |sid: &str, to: &str| {
        let sr = src("redirect", sid, None);
        let id = ItemId::new(ItemKind::Redirect, &sr);
        let r = Redirect {
            from: format!("/{sid}"),
            to: to.into(),
            code: 301,
            origin: RedirectOrigin::Plugin,
        };
        store.put_item(ItemKind::Redirect, id, &sr, &r).unwrap();
        id
    };
    let r_ok = redirect("a", "/new-1");
    let r_chain = redirect("b", "/new-1");
    let r_gone = redirect("c", "/new-1");
    store.soft_delete(r_gone).unwrap();
    route("doc", Some(200), RouteTarget::Document(doc));
    route("none200", Some(200), RouteTarget::None);
    route("nonenull", None, RouteTarget::None);
    route("nf", Some(404), RouteTarget::None);
    route("red-ok", Some(301), RouteTarget::Redirect(r_ok));
    route("red-chain", Some(301), RouteTarget::Redirect(r_chain));
    route("red-gone", Some(301), RouteTarget::Redirect(r_gone));
    // Unresolved by the one definition: a redirect to nowhere, a document with no path, a
    // media target that is really a document.
    let r_nowhere = redirect("d", "/nowhere");
    route("red-nowhere", Some(301), RouteTarget::Redirect(r_nowhere));
    let pathless = {
        let sr = src("post", "2", None);
        let id = ItemId::new(ItemKind::Document, &sr);
        store
            .put_item(
                ItemKind::Document,
                id,
                &sr,
                &serde_json::json!({"title": "x"}),
            )
            .unwrap();
        id
    };
    route("doc-nopath", Some(200), RouteTarget::Document(pathless));
    // An absolute `to` on another site is external: resolved, never matched against this
    // site's paths. The same site's absolute URL is local.
    let r_offsite = redirect("e", "https://elsewhere.test/nowhere");
    route("red-offsite", Some(301), RouteTarget::Redirect(r_offsite));
    let r_samesite = redirect("f", "https://www.old.test/new-1");
    route("red-samesite", Some(301), RouteTarget::Redirect(r_samesite));
    let r_samesite_gone = redirect("g", "https://old.test/nowhere");
    route(
        "red-samesite-gone",
        Some(301),
        RouteTarget::Redirect(r_samesite_gone),
    );
    route("wrong-kind", Some(200), RouteTarget::Media(doc));
    // Dead links the old site already had are 404s, not unresolved.
    route("nf-target", Some(410), RouteTarget::None);
    // Another connector's raw route never shows.
    let raw = SourceRef {
        connector: "wp-rest".into(),
        source_id: "route:raw".into(),
        url: None,
    };
    let rid = ItemId::new(ItemKind::Route, &raw);
    let rr = Route {
        url: "https://old.test/raw".into(),
        target: RouteTarget::None,
        http_status: Some(404),
        title: None,
    };
    store.put_item(ItemKind::Route, rid, &raw, &rr).unwrap();
    let run = store.begin_run(Stage::Normalise).unwrap();
    store
        .open_issue(
            run,
            porter_model::Issue {
                id: uuid::Uuid::now_v7(),
                kind: IssueKind::RedirectChain,
                severity: Severity::Info,
                item: Some(r_chain),
                stage: Stage::Normalise,
                message: "went through 2 redirects".into(),
                details: serde_json::json!({"hops": 2}),
                resolution: porter_model::Resolution::Open,
            },
        )
        .unwrap();

    let all = Page {
        offset: 0,
        limit: 100,
    };
    let got = |f| {
        let s = &s;
        async move { s.routes(f, all).await.unwrap() }
    };
    let every = got(RouteFilter::All).await;
    assert_eq!(
        (every.matching, every.total, every.rows.len()),
        (14, 14, 14)
    );
    let doc_row = every.rows.iter().find(|r| r.url.ends_with("/doc")).unwrap();
    assert_eq!(doc_row.status, Some(200));
    assert_eq!(doc_row.target, RouteTarget::Document(doc));
    assert_eq!(doc_row.new_url.as_deref(), Some("/new-1"));
    let ok = every
        .rows
        .iter()
        .find(|r| r.url.ends_with("/red-ok"))
        .unwrap();
    assert_eq!(ok.new_url.as_deref(), Some("/new-1"));
    assert!(!ok.chain);

    let un = got(RouteFilter::Unresolved).await;
    let mut urls: Vec<_> = un
        .rows
        .iter()
        .map(|r| r.url.rsplit('/').next().unwrap())
        .collect();
    urls.sort();
    assert_eq!(
        urls,
        [
            "doc-nopath",
            "none200",
            "nonenull",
            "red-gone",
            "red-nowhere",
            "red-samesite-gone",
            "wrong-kind"
        ]
    );
    assert_eq!((un.matching, un.total), (7, 14));
    // The filter and the row flag are one definition: the count is what All flags.
    assert_eq!(
        every.rows.iter().filter(|r| r.unresolved).count() as u64,
        un.matching
    );
    assert!(un.rows.iter().all(|r| r.unresolved));
    // The Unresolved filter pages too.
    let tail = s
        .routes(
            RouteFilter::Unresolved,
            Page {
                offset: 4,
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!((tail.rows.len(), tail.matching), (3, 7));

    let ch = got(RouteFilter::Chains).await;
    assert_eq!(ch.rows.len(), 1);
    assert!(ch.rows[0].chain && ch.rows[0].url.ends_with("/red-chain"));

    let nf = got(RouteFilter::NotFound).await;
    assert_eq!((nf.matching, nf.rows.len()), (1, 1));
    assert!(nf.rows[0].url.ends_with("/nf"));

    // Paging is over the filtered set; `total` stays the unfiltered count.
    let p = s
        .routes(
            RouteFilter::All,
            Page {
                offset: 5,
                limit: 5,
            },
        )
        .await
        .unwrap();
    assert_eq!((p.rows.len(), p.matching, p.total), (5, 14, 14));
}

/// A parent cycle (A -> B -> A) has no top; both documents must still be reachable from the
/// tree's roots.
#[tokio::test]
async fn children_reach_documents_in_a_parent_cycle() {
    use porter_engine::session::Page;
    let d = tempfile::tempdir().unwrap();
    Project::init(d.path(), "t", "wordpress").unwrap();
    let s = Session::open(d.path()).unwrap();
    let store = s.project().store();
    let id_of = |sid: &str| {
        ItemId::new(
            ItemKind::Document,
            &SourceRef {
                connector: "wordpress".into(),
                source_id: sid.into(),
                url: Some(format!("https://old.test/{sid}")),
            },
        )
    };
    let (a, b) = (id_of("a"), id_of("b"));
    for (id, src, doc) in [
        doc("wordpress", "a", "page", "A", Some(b)),
        doc("wordpress", "b", "page", "B", Some(a)),
        doc("wordpress", "c", "page", "C", Some(a)),
    ] {
        store.put_item(ItemKind::Document, id, &src, &doc).unwrap();
    }
    let page = || Page {
        offset: 0,
        limit: 50,
    };
    let roots = s.children(None, None, page()).await.unwrap();
    let ids: Vec<ItemId> = roots.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![a.min(b)], "one root for the cycle");
    let top = ids[0];
    let kids = s.children(Some(top), None, page()).await.unwrap();
    let other = if top == a { b } else { a };
    assert!(
        kids.rows.iter().any(|r| r.id == other),
        "the rest of the cycle is beneath it"
    );
}
