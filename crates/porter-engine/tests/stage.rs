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

#[tokio::test]
async fn err_emits_stage_finished_interrupted() {
    let (_d, p, cfg) = setup();
    let sink = EventSink::new(8);
    let mut rx = sink.subscribe();
    let r = run_stage(
        p,
        cfg,
        sink,
        CancellationToken::new(),
        Stage::Extract,
        |_| async { anyhow::bail!("boom") },
    )
    .await;
    assert_eq!(r.unwrap_err().to_string(), "boom");
    let mut last = None;
    while let Ok(e) = rx.try_recv() {
        last = Some(e);
    }
    match last {
        Some(Event::StageFinished { summary, .. }) => assert!(summary.interrupted),
        other => panic!("expected StageFinished, got {other:?}"),
    }
}

#[tokio::test]
async fn reopened_issue_event_has_stored_id() {
    use porter_model::Resolution;
    let (_d, p, cfg) = setup();
    let open = |ctx: porter_engine::stage::StageCtx| {
        ctx.issue(
            IssueKind::WriteFailed,
            Severity::Error,
            Some(id("a")),
            "m",
            serde_json::json!({}),
        )
    };
    let sink1 = EventSink::new(8);
    let mut rx1 = sink1.subscribe();
    run_stage(
        p.clone(),
        cfg.clone(),
        sink1,
        CancellationToken::new(),
        Stage::Load,
        |ctx| async move { open(ctx) },
    )
    .await
    .unwrap();
    let first = loop {
        if let Event::IssueOpened { issue } = rx1.try_recv().unwrap() {
            break issue;
        }
    };
    p.store()
        .resolve_issue(first.id, Resolution::HandledManually { note: "n".into() })
        .unwrap();

    let sink2 = EventSink::new(8);
    let mut rx2 = sink2.subscribe();
    let s2 = run_stage(
        p,
        cfg,
        sink2,
        CancellationToken::new(),
        Stage::Load,
        |ctx| async move { open(ctx) },
    )
    .await
    .unwrap();
    let second = loop {
        if let Event::IssueOpened { issue } = rx2.try_recv().unwrap() {
            break issue;
        }
    };
    assert_eq!(second.id, first.id);
    assert_eq!(
        second.resolution,
        Resolution::HandledManually { note: "n".into() }
    );
    assert_eq!(s2.errors, 0);
    assert_eq!(exit_code(&s2), 0);
}

#[tokio::test]
async fn retry_exhausts_on_5xx() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .expect(3)
        .mount(&server)
        .await;
    let c = http::client("porter-test");
    let r = http::send_with_retry(c.get(server.uri()), 3).await.unwrap();
    assert_eq!(r.status().as_u16(), 503);
}

#[tokio::test]
async fn retry_without_retry_after_backs_off() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
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
    assert!(t.elapsed() >= Duration::from_millis(500));
}
