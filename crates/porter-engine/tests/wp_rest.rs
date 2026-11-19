use porter_engine::config::ConnectorConfig;
use porter_engine::connectors::RawItem;
use porter_engine::events::EventSink;
use porter_engine::project::Project;
use porter_engine::sources::wordpress::rest::WpRestConnector;
use porter_engine::stage::{Connector, run_stage};
use porter_engine::store::IssueFilter;
use porter_model::{ItemId, ItemKind, SourceRef, Stage};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Env vars are process-wide, so every test holds this.
static ENV: Mutex<()> = Mutex::const_new(());

const VARS: [&str; 6] = [
    "PORTER_WP_AUTH_TYPE",
    "PORTER_WP_BEARER_TOKEN",
    "PORTER_WP_USERNAME",
    "PORTER_WP_PASSWORD",
    "PORTER_WP_API_KEY",
    "PORTER_WP_API_KEY_HEADER",
];

async fn lock() -> MutexGuard<'static, ()> {
    let g = ENV.lock().await;
    for v in VARS {
        unsafe { std::env::remove_var(v) };
    }
    // No test may touch the real keychain.
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    g
}

async fn run(
    server: &MockServer,
    keyring: Option<&str>,
    stage: Stage,
) -> (tempfile::TempDir, Arc<Project>) {
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(dir.path(), "t", "wordpress").unwrap());
    let conn = Arc::new(WpRestConnector::new(&ConnectorConfig::WpRest {
        url: server.uri(),
        auth: None,
        keyring: keyring.map(String::from),
    }));
    run_stage(
        p.clone(),
        Arc::new(p.config().clone()),
        EventSink::new(256),
        CancellationToken::new(),
        stage,
        |ctx| async move {
            if stage == Stage::Analyse {
                conn.analyse(&ctx).await
            } else {
                conn.extract(&ctx).await
            }
        },
    )
    .await
    .unwrap();
    (dir, p)
}

async fn mock_json(server: &MockServer, p: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mock_root(server: &MockServer, namespaces: Value) {
    mock_json(
        server,
        "/wp-json",
        json!({"namespaces": namespaces, "routes": {}}),
    )
    .await;
    mock_json(
        server,
        "/wp-json/wp/v2/types",
        json!({
            "post": {"slug": "post", "rest_base": "posts"},
            "page": {"slug": "page", "rest_base": "pages"},
            "attachment": {"slug": "attachment", "rest_base": "media"},
        }),
    )
    .await;
    mock_json(
        server,
        "/wp-json/wp/v2/taxonomies",
        json!({"category": {"slug": "category", "rest_base": "categories"}}),
    )
    .await;
}

fn raw_files(p: &Project, route: &str) -> usize {
    std::fs::read_dir(p.raw_dir("wp-rest").join(route))
        .map(|d| d.count())
        .unwrap_or(0)
}

fn issue_messages(p: &Project) -> Vec<String> {
    p.store()
        .issues(&IssueFilter::default())
        .unwrap()
        .into_iter()
        .filter(|i| i.kind == porter_model::IssueKind::AccessGap)
        .map(|i| i.message)
        .collect()
}

#[tokio::test]
async fn rest_paginates_by_total_pages() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2"])).await;
    for (page, n, start) in [(1, 100, 0), (2, 100, 100), (3, 50, 200)] {
        let recs: Vec<Value> = (0..n)
            .map(|i| json!({"id": start + i + 1, "status": "publish", "link": "https://example.com/x/", "title": {"rendered": "T"}}))
            .collect();
        Mock::given(method("GET"))
            .and(path("/wp-json/wp/v2/posts"))
            .and(query_param("page", page.to_string()))
            .and(query_param("per_page", "100"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-WP-TotalPages", "3")
                    .insert_header("X-WP-Total", "250")
                    .set_body_json(recs),
            )
            .mount(&s)
            .await;
    }
    let (_d, p) = run(&s, None, Stage::Extract).await;
    assert_eq!(raw_files(&p, "posts"), 250);
    let src = SourceRef {
        connector: "wp-rest".into(),
        source_id: "posts:42".into(),
        url: None,
    };
    let id = ItemId::new(ItemKind::Document, &src);
    let row: RawItem = p.store().get_item(id).unwrap().unwrap();
    assert_eq!(row.label, "post");
    assert_eq!(row.raw_path.as_deref(), Some("posts/42.json"));
}

#[tokio::test]
async fn rest_unauthenticated_reports_gaps() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2"])).await;
    let (_d, p) = run(&s, None, Stage::Extract).await;
    let m = issue_messages(&p);
    assert!(m.iter().any(|m| m.contains("drafts")), "{m:?}");
    assert!(m.iter().any(|m| m.contains("menus")), "{m:?}");
    // Unauthenticated: no draft status filter and no menu requests.
    let reqs = s.received_requests().await.unwrap();
    assert!(!reqs.iter().any(|r| r.url.path().contains("menu")));
    assert!(
        !reqs
            .iter()
            .any(|r| r.url.query().unwrap_or("").contains("status=any"))
    );
}

#[tokio::test]
async fn rest_env_overrides_keyring() {
    let _g = lock().await;
    unsafe { std::env::set_var("PORTER_WP_BEARER_TOKEN", "tok-123") };
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2"])).await;
    // Only the authorised request gets records; anything else would 401.
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/posts"))
        .and(header("authorization", "Bearer tok-123"))
        .and(query_param("status", "any"))
        .and(query_param("context", "edit"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-WP-TotalPages", "1")
                .set_body_json(json!([{"id": 1, "status": "draft", "title": {"raw": "D"}}])),
        )
        .mount(&s)
        .await;
    let (_d, p) = run(&s, Some("site"), Stage::Extract).await;
    assert_eq!(raw_files(&p, "posts"), 1);
    // The mock keyring has no entry: reported as info, token still used.
    let gaps = p.store().issues(&IssueFilter::default()).unwrap();
    assert!(gaps.iter().any(|i| i.message.contains("keyring")));
    assert!(!gaps.iter().any(|i| i.message.contains("tok-123")));
    // Authenticated, so no blanket draft gap.
    assert!(!issue_messages(&p).iter().any(|m| m.contains("drafts")));
}

#[tokio::test]
async fn rest_denied_endpoint_is_a_gap_not_a_failure() {
    let _g = lock().await;
    unsafe { std::env::set_var("PORTER_WP_BEARER_TOKEN", "t") };
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2"])).await;
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/users"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&s)
        .await;
    let (_d, p) = run(&s, None, Stage::Extract).await;
    assert!(
        issue_messages(&p)
            .iter()
            .any(|m| m.contains("/wp/v2/users"))
    );
}

#[tokio::test]
async fn rest_optional_namespaces() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2", "redirection/v1"])).await;
    mock_json(
        &s,
        "/wp-json/redirection/v1/redirect",
        json!({"items": [{"id": 12, "url": "/old", "action_data": {"url": "/new"}, "action_code": 301, "status": "enabled"}], "total": 1}),
    )
    .await;
    let (_d, p) = run(&s, None, Stage::Extract).await;
    assert_eq!(raw_files(&p, "redirection"), 1);
    let id = ItemId::new(
        ItemKind::Redirect,
        &SourceRef {
            connector: "wp-rest".into(),
            source_id: "redirection:12".into(),
            url: None,
        },
    );
    assert!(p.store().get_item::<RawItem>(id).unwrap().is_some());

    let s2 = MockServer::start().await;
    mock_root(&s2, json!(["wp/v2"])).await;
    let (_d2, p2) = run(&s2, None, Stage::Extract).await;
    assert_eq!(raw_files(&p2, "redirection"), 0);
    let reqs = s2.received_requests().await.unwrap();
    assert!(!reqs.iter().any(|r| r.url.path().contains("redirection")));
}

#[tokio::test]
async fn rest_analyse_counts_types() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2", "yoast/v1"])).await;
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/posts"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-WP-Total", "7")
                .set_body_json(json!([])),
        )
        .mount(&s)
        .await;
    let (_d, p) = run(&s, None, Stage::Analyse).await;
    let a: Value =
        serde_json::from_slice(&std::fs::read(p.raw_dir("wp-rest").join("analysis.json")).unwrap())
            .unwrap();
    assert_eq!(a["types"]["post"]["total"], 7);
    assert_eq!(a["namespaces"][1], "yoast/v1");
}

fn issues_of(p: &Project, kind: porter_model::IssueKind) -> Vec<String> {
    p.store()
        .issues(&IssueFilter::default())
        .unwrap()
        .into_iter()
        .filter(|i| i.kind == kind)
        .map(|i| i.message)
        .collect()
}

fn files_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[tokio::test]
async fn apikey_not_sent_cross_host() {
    let _g = lock().await;
    unsafe {
        std::env::set_var("PORTER_WP_AUTH_TYPE", "apikey");
        std::env::set_var("PORTER_WP_API_KEY", "secret-key");
    }
    let other = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(0)
        .mount(&other)
        .await;
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2"])).await;
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/posts"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/wp-json/wp/v2/posts", other.uri())),
        )
        .mount(&s)
        .await;
    let (_d, p) = run(&s, None, Stage::Extract).await;
    let m = issue_messages(&p);
    assert!(
        m.iter()
            .any(|m| m.contains("/wp/v2/posts") && m.contains("redirected")),
        "{m:?}"
    );
    assert!(!m.iter().any(|m| m.contains("secret-key")));
    // `other` verifies expect(0) when dropped.
}

#[tokio::test]
async fn hostile_rest_base_stays_inside_raw() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_json(
        &s,
        "/wp-json",
        json!({"namespaces": ["wp/v2"], "routes": {}}),
    )
    .await;
    mock_json(
        &s,
        "/wp-json/wp/v2/types",
        json!({
            "a": {"slug": "a", "rest_base": "../../evil"},
            "b": {"slug": "b", "rest_base": "font-families/(?P<id>[\\d]+)/font-faces"},
        }),
    )
    .await;
    mock_json(
        &s,
        "/wp-json/wp/v2/taxonomies",
        json!({"t": {"slug": "../../tax", "rest_base": "../../tax"}}),
    )
    .await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-WP-TotalPages", "1")
                .set_body_json(json!([{"id": 1, "status": "draft"}])),
        )
        .mount(&s)
        .await;
    let (d, _p) = run(&s, None, Stage::Extract).await;
    let mut files = vec![];
    files_under(d.path(), &mut files);
    let raw = d.path().join("raw/wp-rest");
    let mut inside = 0;
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        if f.starts_with(&raw) {
            inside += 1;
        } else {
            assert!(
                name == "porter.toml" || name.starts_with("project.db"),
                "stray file {f:?}"
            );
        }
    }
    assert!(inside >= 2);
    assert!(!d.path().join("evil").exists() && !d.path().join("tax").exists());
    assert!(!d.path().parent().unwrap().join("evil").exists());
}

#[tokio::test]
async fn types_failure_reports_gap() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_json(
        &s,
        "/wp-json",
        json!({"namespaces": ["wp/v2"], "routes": {}}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/types"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/taxonomies"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&s)
        .await;
    let (_d, p) = run(&s, None, Stage::Extract).await;
    let m = issue_messages(&p);
    assert!(
        m.iter()
            .any(|m| m.contains("custom post types unknown") && m.contains("only post/page")),
        "{m:?}"
    );
    assert!(
        m.iter().any(|m| m.contains("custom taxonomies unknown")),
        "{m:?}"
    );
}

#[tokio::test]
async fn orphan_menu_items_reported() {
    let _g = lock().await;
    unsafe { std::env::set_var("PORTER_WP_BEARER_TOKEN", "t") };
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2"])).await;
    mock_json(
        &s,
        "/wp-json/wp/v2/menus",
        json!([{"id": 1, "name": "Main"}, {"name": "No id"}]),
    )
    .await;
    mock_json(
        &s,
        "/wp-json/wp/v2/menu-items",
        json!([{"id": 10, "menus": 1}, {"id": 11, "menus": 99}]),
    )
    .await;
    let (_d, p) = run(&s, None, Stage::Extract).await;
    assert_eq!(raw_files(&p, "menus"), 2); // 1.json + orphans.json
    let raw = p.raw_dir("wp-rest").join("menus");
    let orphans: Value =
        serde_json::from_slice(&std::fs::read(raw.join("orphans.json")).unwrap()).unwrap();
    assert_eq!(orphans.as_array().unwrap().len(), 1);
    let m = issues_of(&p, porter_model::IssueKind::ParentMissing);
    assert!(m.iter().any(|m| m.starts_with("1 menu items")), "{m:?}");
    let failed = ItemId::new(
        ItemKind::Menu,
        &SourceRef {
            connector: "wp-rest".into(),
            source_id: "menus:unknown-2".into(),
            url: None,
        },
    );
    assert!(p.store().get_item::<RawItem>(failed).unwrap().is_some());
}

#[tokio::test]
async fn yoast_404_does_not_stop_loop() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2", "yoast/v1"])).await;
    let posts: Vec<Value> = (1..=3)
        .map(
            |i| json!({"id": i, "status": "publish", "link": format!("https://example.com/p{i}/")}),
        )
        .collect();
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/posts"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-WP-TotalPages", "1")
                .set_body_json(posts),
        )
        .mount(&s)
        .await;
    for i in 1..=3 {
        let r = if i == 1 {
            ResponseTemplate::new(404)
        } else {
            ResponseTemplate::new(200).set_body_json(json!({"json": {"title": "x"}}))
        };
        Mock::given(method("GET"))
            .and(path("/wp-json/yoast/v1/get_head"))
            .and(query_param("url", format!("https://example.com/p{i}/")))
            .respond_with(r)
            .mount(&s)
            .await;
    }
    let (_d, p) = run(&s, None, Stage::Extract).await;
    assert_eq!(raw_files(&p, "yoast"), 2);
    let m = issue_messages(&p);
    assert!(
        m.iter().any(|m| m.contains("no SEO head for 1 published")),
        "{m:?}"
    );
}

#[tokio::test]
async fn pagination_stops_when_page_ignored() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2"])).await;
    let recs: Vec<Value> = (1..=100)
        .map(|i| json!({"id": i, "status": "publish"}))
        .collect();
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/posts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(recs))
        .mount(&s)
        .await;
    let (_d, p) = run(&s, None, Stage::Extract).await;
    assert_eq!(raw_files(&p, "posts"), 100);
    let m = issue_messages(&p);
    assert!(
        m.iter().any(|m| m.contains("ignores the page parameter")),
        "{m:?}"
    );
    let n = s
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/wp-json/wp/v2/posts")
        .count();
    assert_eq!(n, 2);
}

#[tokio::test]
async fn total_mismatch_reported() {
    let _g = lock().await;
    let s = MockServer::start().await;
    mock_root(&s, json!(["wp/v2"])).await;
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/posts"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-WP-TotalPages", "1")
                .insert_header("X-WP-Total", "5")
                .set_body_json(json!([{"id": 1}])),
        )
        .mount(&s)
        .await;
    let (_d, p) = run(&s, None, Stage::Extract).await;
    let m = issue_messages(&p);
    assert!(
        m.iter()
            .any(|m| m.contains("fetched 1 records but X-WP-Total says 5")),
        "{m:?}"
    );
}
