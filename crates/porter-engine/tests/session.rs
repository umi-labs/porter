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
