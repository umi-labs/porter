use porter_engine::config::ConnectorConfig;
use porter_engine::connectors::crawl::{CrawlConnector, detect_platform};
use porter_engine::events::EventSink;
use porter_engine::project::Project;
use porter_engine::stage::{Connector, run_stage};
use porter_model::{ItemKind, Media, Redirect, RedirectOrigin, Route, Stage};
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cfg(url: &str, exclude: &[&str]) -> ConnectorConfig {
    ConnectorConfig::Crawl {
        url: url.into(),
        concurrency: 4,
        delay_ms: 0,
        include: vec![],
        exclude: exclude.iter().map(|s| s.to_string()).collect(),
        max_pages: None,
    }
}

async fn page(server: &MockServer, p: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body.to_string(), "text/html"))
        .mount(server)
        .await;
}

async fn sitemap(server: &MockServer, paths: &[&str]) {
    let locs: String = paths
        .iter()
        .map(|p| format!("<url><loc>{}{p}</loc></url>", server.uri()))
        .collect();
    let xml = format!(
        r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{locs}</urlset>"#
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(xml, "application/xml"))
        .mount(server)
        .await;
}

async fn run(c: ConnectorConfig, stage: Stage, refresh: bool) -> (tempfile::TempDir, Arc<Project>) {
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(dir.path(), "t", "a").unwrap());
    let conn = CrawlConnector::new(&c);
    let conn = Arc::new(conn);
    run_stage(
        p.clone(),
        Arc::new(p.config().clone()),
        EventSink::new(256),
        CancellationToken::new(),
        stage,
        |mut ctx| async move {
            ctx.refresh = refresh;
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

fn routes(p: &Project) -> Vec<Route> {
    p.store()
        .items::<Route>(ItemKind::Route)
        .unwrap()
        .into_iter()
        .map(|r| r.2)
        .collect()
}

#[tokio::test]
async fn crawl_follows_sitemap_and_links() {
    let s = MockServer::start().await;
    sitemap(&s, &["/", "/old"]).await;
    page(&s, "/", r#"<title>Home</title><a href="/team">t</a>"#).await;
    page(&s, "/team", "<title>Team</title>").await;
    page(&s, "/about", "<title>About</title>").await;
    Mock::given(method("GET"))
        .and(path("/old"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", "/about"))
        .mount(&s)
        .await;
    let (_d, p) = run(cfg(&s.uri(), &[]), Stage::Analyse, false).await;
    let rs = routes(&p);
    assert_eq!(rs.len(), 4, "{rs:?}");
    assert!(rs.iter().all(|r| r.http_status.is_some()));
    let reds = p.store().items::<Redirect>(ItemKind::Redirect).unwrap();
    assert_eq!(reds.len(), 1);
    assert_eq!(reds[0].2.origin, RedirectOrigin::Crawl);
    assert_eq!(reds[0].2.code, 301);
    assert!(rs.iter().any(|r| r.title.as_deref() == Some("About")));
    assert!(p.raw_dir("crawl").join("detect.json").exists());
    // every route has an analyse status
    let st = p.store().statuses(Stage::Analyse).unwrap();
    assert!(st.len() > 4);
}

#[tokio::test]
async fn crawl_respects_exclude() {
    let s = MockServer::start().await;
    sitemap(&s, &["/", "/private/a"]).await;
    page(&s, "/", r#"<a href="/private/b">x</a><a href="/ok">y</a>"#).await;
    page(&s, "/ok", "ok").await;
    page(&s, "/private/a", "no").await;
    page(&s, "/private/b", "no").await;
    let (_d, p) = run(cfg(&s.uri(), &["/private/*"]), Stage::Analyse, false).await;
    let rs = routes(&p);
    assert_eq!(rs.len(), 2, "{rs:?}");
    assert!(rs.iter().all(|r| !r.url.contains("/private/")));
}

#[tokio::test]
async fn crawl_honours_robots() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "User-agent: porter\nDisallow: /secret/\n\nUser-agent: *\nDisallow: /\n",
        ))
        .mount(&s)
        .await;
    page(&s, "/", r#"<a href="/secret/x">x</a><a href="/ok">y</a>"#).await;
    page(&s, "/ok", "ok").await;
    let (_d, p) = run(cfg(&s.uri(), &[]), Stage::Analyse, false).await;
    let secret = routes(&p)
        .into_iter()
        .find(|r| r.url.contains("/secret/"))
        .unwrap();
    assert_eq!(secret.http_status, None);
    let hits = s.received_requests().await.unwrap();
    assert!(hits.iter().all(|r| !r.url.path().starts_with("/secret")));
    assert!(hits.iter().any(|r| r.url.path() == "/ok"));
}

#[test]
fn detect_wordpress_divi_yoast() {
    let html = r#"<html><head><meta name="generator" content="WordPress 6.4">
        <!-- This site is optimized with the Yoast SEO plugin v21 -->
        <link rel="stylesheet" href="/wp-content/themes/Divi/style.css"></head>
        <body><div class="et_pb_section"><div class="gform_wrapper"></div></div></body></html>"#;
    let d = detect_platform(html, &HeaderMap::new());
    assert_eq!(d.platform.as_deref(), Some("wordpress"));
    assert!(d.builders.contains(&"divi".to_string()));
    assert!(d.plugins.contains(&"yoast".to_string()));
    assert!(d.plugins.contains(&"gravity-forms".to_string()));
    let d = detect_platform(r#"<html data-wf-site="x"></html>"#, &HeaderMap::new());
    assert_eq!(d.platform.as_deref(), Some("webflow"));
}

async fn media_site(same_bytes: bool) -> MockServer {
    let s = MockServer::start().await;
    page(
        &s,
        "/p",
        r#"<img src="/a-300x200.jpg" srcset="/a-300x200.jpg 300w, /a.jpg 1200w">"#,
    )
    .await;
    for (p, body) in [
        ("/a-300x200.jpg", "same"),
        ("/a.jpg", if same_bytes { "same" } else { "different" }),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "image/jpeg"))
            .mount(&s)
            .await;
    }
    s
}

#[tokio::test]
async fn extract_downloads_srcset_variants_once() {
    let s = media_site(true).await;
    let (_d, p) = run(cfg(&format!("{}/p", s.uri()), &[]), Stage::Extract, false).await;
    let media = p.store().items::<Media>(ItemKind::Media).unwrap();
    assert_eq!(media.len(), 1);
    assert_eq!(media[0].2.variants.len(), 2);
    let html = std::fs::read_dir(p.raw_dir("crawl").join("html")).unwrap();
    assert_eq!(html.count(), 1);

    let s = media_site(false).await;
    let (_d, p) = run(cfg(&format!("{}/p", s.uri()), &[]), Stage::Extract, false).await;
    assert_eq!(p.store().items::<Media>(ItemKind::Media).unwrap().len(), 2);
}

#[tokio::test]
async fn redirect_loop_fails_route_with_issue() {
    let s = MockServer::start().await;
    for (a, b) in [("/a", "/b"), ("/b", "/a")] {
        Mock::given(method("GET"))
            .and(path(a))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", b))
            .mount(&s)
            .await;
    }
    let (_d, p) = run(cfg(&format!("{}/a", s.uri()), &[]), Stage::Analyse, false).await;
    let st = p.store().statuses(Stage::Analyse).unwrap();
    assert!(
        st.iter()
            .any(|(_, s)| matches!(s, porter_model::ItemStatus::Failed { .. }))
    );
    let issues = p.store().issues(&Default::default()).unwrap();
    assert!(
        issues
            .iter()
            .any(|i| i.kind == porter_model::IssueKind::RedirectLoop)
    );
}
