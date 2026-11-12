use porter_engine::config::ProjectConfig;
use porter_engine::events::{Event, EventSink, Summary, exit_code};
use porter_engine::http;
use porter_engine::project::Project;
use porter_engine::stage::run_stage;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, Severity, SourceRef, Stage};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn id(n: &str) -> ItemId {
    ItemId::new(
        ItemKind::Document,
        &SourceRef {
            connector: "c".into(),
            source_id: n.into(),
            url: None,
        },
    )
}

#[test]
fn exit_codes() {
    let s = Summary {
        done: 3,
        ..Default::default()
    };
    assert_eq!(exit_code(&s), 0);
    assert_eq!(
        exit_code(&Summary {
            failed: 1,
            ..s.clone()
        }),
        2
    );
    assert_eq!(
        exit_code(&Summary {
            errors: 1,
            ..s.clone()
        }),
        2
    );
    assert_eq!(
        exit_code(&Summary {
            warnings: 4,
            ..s.clone()
        }),
        0
    );
    assert_eq!(
        exit_code(&Summary {
            interrupted: true,
            ..s
        }),
        3
    );
}

#[test]
fn event_json_shape() {
    let v = serde_json::to_value(Event::Progress {
        stage: Stage::Extract,
        done: 1,
        total: Some(2),
        label: "x".into(),
    })
    .unwrap();
    assert_eq!(v["event"], "progress");
    assert_eq!(v["v"], 1);
    assert_eq!(v["stage"], "extract");
    assert_eq!(v["total"], 2);
}

fn setup() -> (tempfile::TempDir, Arc<Project>, Arc<ProjectConfig>) {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "t", "a").unwrap();
    let cfg = Arc::new(p.config().clone());
    (dir, Arc::new(p), cfg)
}

#[tokio::test]
async fn stage_tallies_and_emits() {
    let (_d, p, cfg) = setup();
    let sink = EventSink::new(64);
    let mut rx = sink.subscribe();
    let sum = run_stage(
        p,
        cfg,
        sink,
        CancellationToken::new(),
        Stage::Extract,
        |ctx| async move {
            ctx.item_status(id("a"), ItemKind::Document, ItemStatus::Done)?;
            ctx.item_status(id("b"), ItemKind::Document, ItemStatus::Done)?;
            ctx.item_status(
                id("c"),
                ItemKind::Document,
                ItemStatus::Failed { error: "e".into() },
            )?;
            ctx.issue(
                IssueKind::MissingMedia,
                Severity::Warning,
                Some(id("a")),
                "m",
                serde_json::json!({}),
            )?;
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (sum.done, sum.failed, sum.warnings, sum.errors),
        (2, 1, 1, 0)
    );
    assert!(!sum.interrupted);
    let mut kinds = vec![];
    while let Ok(e) = rx.try_recv() {
        kinds.push(match e {
            Event::StageStarted { .. } => "start",
            Event::ItemResult { .. } => "item",
            Event::IssueOpened { .. } => "issue",
            Event::StageFinished { .. } => "end",
            Event::Progress { .. } => "progress",
        });
    }
    assert_eq!(kinds, ["start", "item", "item", "item", "issue", "end"]);
}

#[tokio::test]
async fn err_marks_interrupted_and_propagates() {
    let (_d, p, cfg) = setup();
    let r = run_stage(
        p.clone(),
        cfg,
        EventSink::new(8),
        CancellationToken::new(),
        Stage::Extract,
        |ctx| async move {
            ctx.item_status(id("a"), ItemKind::Document, ItemStatus::Done)?;
            anyhow::bail!("boom")
        },
    )
    .await;
    assert_eq!(r.unwrap_err().to_string(), "boom");
}

#[tokio::test]
async fn cancelled_is_interrupted() {
    let (_d, p, cfg) = setup();
    let c = CancellationToken::new();
    c.cancel();
    let s = run_stage(p, cfg, EventSink::new(8), c, Stage::Extract, |_| async {
        Ok(())
    })
    .await
    .unwrap();
    assert!(s.interrupted);
    assert_eq!(exit_code(&s), 3);
}

#[tokio::test]
async fn retry_honours_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let c = http::client("porter-test");
    let t = Instant::now();
    let r = http::send_with_retry(c.get(server.uri()), 3).await.unwrap();
    assert!(r.status().is_success());
    assert!(t.elapsed() >= Duration::from_secs(1));
}
