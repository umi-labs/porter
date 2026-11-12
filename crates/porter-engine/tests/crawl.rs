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
    cfg_full(url, exclude, &[], Some(10_000), 200 * 1024 * 1024)
}

fn cfg_full(
    url: &str,
    exclude: &[&str],
    keep_query: &[&str],
    max_pages: Option<usize>,
    max_media_bytes: u64,
) -> ConnectorConfig {
    ConnectorConfig::Crawl {
        url: url.into(),
        concurrency: 4,
        delay_ms: 0,
        include: vec![],
        exclude: exclude.iter().map(|s| s.to_string()).collect(),
        max_pages,
        keep_query: keep_query.iter().map(|s| s.to_string()).collect(),
        max_media_bytes,
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
    // exactly one analyse status per route
    let st = p.store().statuses(Stage::Analyse).unwrap();
    for (id, ..) in p.store().items::<Route>(ItemKind::Route).unwrap() {
        assert_eq!(st.iter().filter(|(i, _)| *i == id).count(), 1, "{id}");
    }
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

#[tokio::test]
async fn off_host_redirect_not_crawled() {
    let ext = MockServer::start().await;
    page(&ext, "/landing", r#"<a href="/x">x</a><a href="/y">y</a>"#).await;
    page(&ext, "/x", "x").await;
    let s = MockServer::start().await;
    page(&s, "/", r#"<a href="/out">o</a>"#).await;
    Mock::given(method("GET"))
        .and(path("/out"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("Location", format!("{}/landing", ext.uri())),
        )
        .mount(&s)
        .await;
    let (_d, p) = run(cfg(&s.uri(), &[]), Stage::Analyse, false).await;
    let rs = routes(&p);
    assert_eq!(rs.len(), 2, "{rs:?}");
    assert!(rs.iter().all(|r| !r.url.starts_with(&ext.uri())));
    let reds = p.store().items::<Redirect>(ItemKind::Redirect).unwrap();
    assert_eq!(reds.len(), 1);
    assert_eq!(reds[0].2.to, format!("{}/landing", ext.uri()));
    assert!(ext.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn query_variants_collapsed() {
    let s = MockServer::start().await;
    let links: String = (1..=50)
        .map(|n| format!(r#"<a href="/cal?month={n}">m</a>"#))
        .collect();
    page(&s, "/", &links).await;
    page(&s, "/cal", "cal").await;
    let (_d, p) = run(cfg(&s.uri(), &[]), Stage::Analyse, false).await;
    assert_eq!(routes(&p).len(), 2);
    let c = cfg_full(&s.uri(), &[], &["month"], Some(10_000), 1 << 20);
    let (_d, p) = run(c, Stage::Analyse, false).await;
    assert_eq!(routes(&p).len(), 51);
}

#[tokio::test]
async fn max_pages_stops_with_issue() {
    let s = MockServer::start().await;
    let links: String = (1..=5)
        .map(|n| format!(r#"<a href="/p{n}">p</a>"#))
        .collect();
    page(&s, "/", &links).await;
    for n in 1..=5 {
        page(&s, &format!("/p{n}"), "p").await;
    }
    let (_d, p) = run(
        cfg_full(&s.uri(), &[], &[], Some(3), 1 << 20),
        Stage::Analyse,
        false,
    )
    .await;
    assert_eq!(routes(&p).len(), 3);
    let issues = p.store().issues(&Default::default()).unwrap();
    let gap: Vec<_> = issues
        .iter()
        .filter(|i| i.kind == porter_model::IssueKind::AccessGap)
        .collect();
    assert_eq!(gap.len(), 1);
    assert_eq!(gap[0].message, "crawl stopped at max_pages (3)");
}

/// Serves one chunked (no Content-Length) response per connection.
async fn chunked_server() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = sock.write_all(head.as_bytes()).await;
                for _ in 0..5 {
                    if sock.write_all(b"10\r\n0123456789abcdef\r\n").await.is_err() {
                        return;
                    }
                }
                let _ = sock.write_all(b"0\r\n\r\n").await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn media_over_cap_skipped() {
    let chunked = chunked_server().await;
    let s = MockServer::start().await;
    page(
        &s,
        "/p",
        &format!(r#"<img src="/big.jpg"><img src="{chunked}/c.jpg">"#),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/big.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![7u8; 100], "image/jpeg"))
        .mount(&s)
        .await;
    let c = cfg_full(&format!("{}/p", s.uri()), &[], &[], Some(100), 40);
    let (_d, p) = run(c, Stage::Extract, false).await;
    assert!(
        p.store()
            .items::<Media>(ItemKind::Media)
            .unwrap()
            .is_empty()
    );
    let skipped = p
        .store()
        .statuses(Stage::Extract)
        .unwrap()
        .into_iter()
        .filter(|(_, s)| matches!(s, porter_model::ItemStatus::Skipped { reason } if reason == "over size cap"))
        .count();
    assert_eq!(skipped, 2);
    let issues = p.store().issues(&Default::default()).unwrap();
    let over = issues
        .iter()
        .filter(|i| i.message.starts_with("file over size cap"))
        .count();
    assert_eq!(over, 2);
}

#[tokio::test]
async fn cancel_stops_sitemap_walk() {
    let s = MockServer::start().await;
    let xml = format!(
        r#"<sitemapindex>{}</sitemapindex>"#,
        (1..=3)
            .map(|n| format!("<sitemap><loc>{}/s{n}.xml</loc></sitemap>", s.uri()))
            .collect::<String>()
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(xml, "application/xml")
                .set_delay(std::time::Duration::from_secs(5)),
        )
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(dir.path(), "t", "a").unwrap());
    let conn = Arc::new(CrawlConnector::new(&cfg(&s.uri(), &[])));
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        c2.cancel();
    });
    let t = std::time::Instant::now();
    run_stage(
        p.clone(),
        Arc::new(p.config().clone()),
        EventSink::new(16),
        cancel,
        Stage::Analyse,
        |ctx| async move { conn.analyse(&ctx).await },
    )
    .await
    .unwrap();
    assert!(
        t.elapsed() < std::time::Duration::from_millis(2500),
        "{:?}",
        t.elapsed()
    );
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}

/// A live crawl's `raw/crawl` and `media/`, copied to a snapshot folder, replay through a
/// `file://` url with no network: the same routes, saved HTML and media files.
#[tokio::test]
async fn crawl_from_snapshot_dir() {
    let s = MockServer::start().await;
    page(&s, "/", r#"<title>Home</title><a href="/p">p</a>"#).await;
    page(
        &s,
        "/p",
        r#"<title>P</title><img src="/a-300x200.jpg" srcset="/a-300x200.jpg 300w, /a.jpg 1200w">"#,
    )
    .await;
    for (p, body) in [("/a-300x200.jpg", "small"), ("/a.jpg", "big")] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "image/jpeg"))
            .mount(&s)
            .await;
    }
    let (_live_dir, live) = run(cfg(&format!("{}/", s.uri()), &[]), Stage::Extract, false).await;

    let snap = tempfile::tempdir().unwrap();
    copy_dir(&live.raw_dir("crawl"), &snap.path().join("crawl"));
    copy_dir(&live.dir().join("media"), &snap.path().join("media"));
    // One file the snapshot lost: reported, not fetched.
    let lines = std::fs::read_to_string(snap.path().join("crawl/media.ndjson")).unwrap();
    let kept: Vec<&str> = lines.lines().filter(|l| !l.contains("a-300x200")).collect();
    std::fs::write(snap.path().join("crawl/media.ndjson"), kept.join("\n")).unwrap();
    drop(s); // the server is gone: nothing may reach the network

    let url = url::Url::from_directory_path(snap.path())
        .unwrap()
        .to_string();
    let (_d, p) = run(cfg(&url, &[]), Stage::Extract, false).await;
    let key = |rs: Vec<Route>| {
        let mut v: Vec<(String, Option<u16>, Option<String>)> = rs
            .into_iter()
            .map(|r| (r.url, r.http_status, r.title))
            .collect();
        v.sort();
        v
    };
    let replayed = key(routes(&p));
    assert_eq!(replayed, key(routes(&live)));
    assert_eq!(replayed.len(), 2, "{replayed:?}");
    let html = std::fs::read_dir(p.raw_dir("crawl").join("html")).unwrap();
    assert_eq!(html.count(), 2);
    let media = p.store().items::<Media>(ItemKind::Media).unwrap();
    assert_eq!(media.len(), 1, "{media:?}");
    let big = p
        .store()
        .media_by_url(&format!("{}/a.jpg", live_uri(&live)))
        .unwrap();
    assert_eq!(
        std::fs::read(p.store().media_path(big.unwrap()).unwrap()).unwrap(),
        b"big"
    );
    let issues = p
        .store()
        .issues(&porter_engine::store::IssueFilter::default())
        .unwrap();
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("a-300x200.jpg is not in the snapshot")),
        "{issues:?}"
    );

    // Analyse replays too, and writes what it detected.
    let (_d2, a) = run(cfg(&url, &[]), Stage::Analyse, false).await;
    assert_eq!(key(routes(&a)).len(), 2);
    assert!(a.raw_dir("crawl").join("detect.json").exists());
}

/// The live crawl's origin, from the routes it recorded.
fn live_uri(p: &Project) -> String {
    let u = url::Url::parse(&routes(p)[0].url).unwrap();
    u.origin().ascii_serialization()
}
