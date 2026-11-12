use porter_engine::config::ProjectConfig;
use porter_engine::events::Summary;
use porter_engine::project::Project;
use porter_engine::store::IssueFilter;
use porter_model::{Issue, IssueKind, ItemId, ItemKind, Resolution, Severity, SourceRef, Stage};
use serde::{Deserialize, Serialize};
use std::fs;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Thing {
    title: String,
}

fn src(id: &str) -> SourceRef {
    SourceRef {
        connector: "conn-a".into(),
        source_id: id.into(),
        url: Some(format!("https://example.com/{id}/")),
    }
}

fn issue(msg: &str) -> Issue {
    Issue {
        id: Uuid::now_v7(),
        kind: IssueKind::UnresolvedLink,
        severity: Severity::Warning,
        item: Some(ItemId::new(ItemKind::Document, &src("1"))),
        stage: Stage::Normalise,
        message: msg.into(),
        details: serde_json::json!({}),
        resolution: Resolution::Open,
    }
}

#[test]
fn init_then_open() {
    let dir = tempfile::tempdir().unwrap();
    Project::init(dir.path(), "demo", "example-src").unwrap();
    assert!(dir.path().join("porter.toml").exists());
    assert!(dir.path().join("project.db").exists());
    let p = Project::open(dir.path()).unwrap();
    assert_eq!(p.config().project.name, "demo");
    assert_eq!(p.config().source.adapter, "example-src");
    assert!(Project::init(dir.path(), "demo", "example-src").is_err());
}

#[test]
fn put_get_item_upserts() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "demo", "example-src").unwrap();
    let s = src("1");
    let id = ItemId::new(ItemKind::Document, &s);
    p.store()
        .put_item(
            ItemKind::Document,
            id,
            &s,
            &Thing {
                title: "one".into(),
            },
        )
        .unwrap();
    p.store()
        .put_item(
            ItemKind::Document,
            id,
            &s,
            &Thing {
                title: "two".into(),
            },
        )
        .unwrap();
    let all = p.store().items::<Thing>(ItemKind::Document).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, id);
    assert_eq!(all[0].1, s);
    assert_eq!(all[0].2.title, "two");
    assert_eq!(
        p.store().get_item::<Thing>(id).unwrap().unwrap().title,
        "two"
    );
    assert!(
        p.store()
            .items::<Thing>(ItemKind::Media)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn status_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "demo", "example-src").unwrap();
    let id = ItemId::new(ItemKind::Document, &src("1"));
    let run = p.store().begin_run(Stage::Extract).unwrap();
    assert_eq!(p.store().status(Stage::Extract, id).unwrap(), None);
    let st = porter_model::ItemStatus::Skipped {
        reason: "draft".into(),
    };
    p.store().set_status(run, Stage::Extract, id, &st).unwrap();
    p.store()
        .set_status(run, Stage::Extract, id, &porter_model::ItemStatus::Done)
        .unwrap();
    assert_eq!(
        p.store().status(Stage::Extract, id).unwrap(),
        Some(porter_model::ItemStatus::Done)
    );
    assert_eq!(p.store().status(Stage::Load, id).unwrap(), None);
}

#[test]
fn rerun_resolves_missing_issues() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "demo", "example-src").unwrap();
    let st = p.store();
    let run1 = st.begin_run(Stage::Normalise).unwrap();
    let a = issue("A");
    let b = issue("B");
    st.open_issue(run1, a.clone()).unwrap();
    st.open_issue(run1, b.clone()).unwrap();
    st.end_run(run1, &Summary::default()).unwrap();
    st.resolve_issue(
        b.id,
        Resolution::HandledManually {
            note: "fixed by hand".into(),
        },
    )
    .unwrap();

    let run2 = st.begin_run(Stage::Normalise).unwrap();
    st.end_run(run2, &Summary::default()).unwrap();

    let all = st.issues(&IssueFilter::default()).unwrap();
    assert_eq!(all.len(), 2);
    let get = |id: Uuid| all.iter().find(|i| i.id == id).unwrap().resolution.clone();
    assert_eq!(get(a.id), Resolution::FixedByRerun);
    assert_eq!(
        get(b.id),
        Resolution::HandledManually {
            note: "fixed by hand".into()
        }
    );

    let open = st
        .issues(&IssueFilter {
            resolution: Some(Resolution::Open),
            ..Default::default()
        })
        .unwrap();
    assert!(open.is_empty());
}

#[test]
fn reopened_issue_survives_end_run() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "demo", "example-src").unwrap();
    let st = p.store();
    let run1 = st.begin_run(Stage::Normalise).unwrap();
    st.open_issue(run1, issue("A")).unwrap();
    st.end_run(run1, &Summary::default()).unwrap();
    let run2 = st.begin_run(Stage::Normalise).unwrap();
    st.open_issue(run2, issue("A")).unwrap();
    st.end_run(run2, &Summary::default()).unwrap();
    let all = st.issues(&IssueFilter::default()).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].resolution, Resolution::Open);
}

#[test]
fn issue_dedupes() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "demo", "example-src").unwrap();
    let st = p.store();
    let run = st.begin_run(Stage::Normalise).unwrap();
    st.open_issue(run, issue("A")).unwrap();
    st.open_issue(run, issue("A")).unwrap();
    let all = st.issues(&IssueFilter::default()).unwrap();
    assert_eq!(all.len(), 1);
    let none = st
        .issues(&IssueFilter {
            stage: Some(Stage::Load),
            ..Default::default()
        })
        .unwrap();
    assert!(none.is_empty());
}

#[test]
fn clean_only_owned_paths() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "demo", "example-src").unwrap();
    fs::write(dir.path().join("notes.txt"), "keep me").unwrap();
    fs::create_dir_all(p.raw_dir("conn-a")).unwrap();
    fs::write(p.raw_dir("conn-a").join("x.json"), "{}").unwrap();
    fs::create_dir_all(p.media_dir()).unwrap();
    p.clean().unwrap();
    let mut left: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["notes.txt", "porter.toml"]);
}

#[test]
fn config_validate_names_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("porter.toml");
    fs::write(
        &path,
        r#"
[project]
name = "demo"

[source]
adapter = "example-src"

[[connectors]]
kind = "crawl"
start = "https://example.com/"

[destination]
adapter = "example-dest"
api_key_env = "EXAMPLE_KEY"

[[redirects]]
from = "/old"
to = "/new"
"#,
    )
    .unwrap();
    let cfg = ProjectConfig::load(&path).unwrap();
    assert_eq!(cfg.destination.auth_collection, "users");
    assert_eq!(cfg.redirects[0].code, 301);
    assert_eq!(cfg.connectors.len(), 1);
    let errs = cfg.validate();
    assert!(
        errs.iter().any(|e| e.contains("destination.url")),
        "{errs:?}"
    );
    assert_eq!(errs.len(), 1, "{errs:?}");
}
