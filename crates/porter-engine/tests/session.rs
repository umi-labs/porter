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
