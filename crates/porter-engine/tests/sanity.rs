use flate2::Compression;
use flate2::write::GzEncoder;
use porter_engine::config::{ConnectorConfig, SanityAssets};
use porter_engine::events::EventSink;
use porter_engine::project::Project;
use porter_engine::sources::sanity::SanitySource;
use porter_engine::stage::{Source, run_stage};
use porter_model::{ItemKind, Stage};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// The fixture's asset docs use the host `http://cdn.test`; later tests rewrite it to a mock
// server's URI and serve the bytes `asset-1`..`asset-4`, whose SHA-1s the fixture carries.

/// `PORTER_SANITY_API_BASE` is process-wide, so tests that set it take this lock.
static BASE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fx {
    d: tempfile::TempDir,
    p: Arc<Project>,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sanity/data.ndjson")
}

fn fixture() -> String {
    std::fs::read_to_string(fixture_path()).unwrap()
}

/// Non-asset documents in the fixture.
fn fixture_doc_count() -> usize {
    fixture()
        .lines()
        .filter(|l| !l.contains("\"sanity.imageAsset\"") && !l.contains("\"sanity.fileAsset\""))
        .count()
}

/// The fixture as `<top>/data.ndjson` in a tar.gz, as `sanity dataset export` writes it.
fn build_targz(dest: &std::path::Path) {
    let bytes = fixture().into_bytes();
    let gz = GzEncoder::new(std::fs::File::create(dest).unwrap(), Compression::default());
    let mut tar = tar::Builder::new(gz);
    let mut h = tar::Header::new_gnu();
    h.set_size(bytes.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    tar.append_data(&mut h, "production/data.ndjson", bytes.as_slice())
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
}

fn project(c: ConnectorConfig) -> Fx {
    let d = tempfile::tempdir().unwrap();
    let p = Project::init(d.path(), "t", "sanity").unwrap();
    let mut cfg = p.config().clone();
    cfg.connectors = vec![c];
    std::fs::write(d.path().join("porter.toml"), toml::to_string(&cfg).unwrap()).unwrap();
    Fx {
        p: Arc::new(Project::open(d.path()).unwrap()),
        d,
    }
}

fn api(token_env: &str) -> ConnectorConfig {
    ConnectorConfig::SanityApi {
        project_id: "proj1".into(),
        dataset: "production".into(),
        token_env: token_env.into(),
        api_version: "v2021-06-07".into(),
        assets: SanityAssets::Referenced,
    }
}

fn export(path: PathBuf) -> ConnectorConfig {
    ConnectorConfig::SanityExport {
        path,
        assets: SanityAssets::Referenced,
    }
}

async fn stage(fx: &Fx, st: Stage) -> anyhow::Result<porter_engine::events::Summary> {
    let cfg = Arc::new(fx.p.config().clone());
    run_stage(
        fx.p.clone(),
        cfg,
        EventSink::new(4096),
        CancellationToken::new(),
        st,
        |ctx| async move {
            let src = SanitySource;
            for c in src.connectors(&ctx.config)? {
                if st == Stage::Analyse {
                    c.analyse(&ctx).await?;
                } else {
                    c.extract(&ctx).await?;
                }
            }
            Ok(())
        },
    )
    .await
}

/// (source_id, connector, status) of every raw row, sorted.
fn rows(fx: &Fx) -> Vec<(String, String, Option<String>)> {
    let mut v: Vec<_> =
        fx.p.store()
            .items::<Value>(ItemKind::Document)
            .unwrap()
            .into_iter()
            .map(|(_, s, d)| {
                (
                    s.source_id,
                    s.connector,
                    d["status"].as_str().map(str::to_string),
                )
            })
            .collect();
    v.sort();
    v
}

#[tokio::test]
async fn api_export_sends_bearer_and_writes_raw_rows() {
    let _g = BASE_LOCK.lock().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2021-06-07/data/export/production"))
        .and(header("Authorization", "Bearer t0ken"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture()))
        .expect(1)
        .mount(&server)
        .await;
    unsafe {
        std::env::set_var("PORTER_SANITY_API_BASE", server.uri());
        std::env::set_var("PORTER_SANITY_TOKEN_T1", "t0ken");
    }
    let fx = project(api("PORTER_SANITY_TOKEN_T1"));
    stage(&fx, Stage::Analyse).await.unwrap();
    let r = rows(&fx);
    assert_eq!(r.len(), fixture_doc_count());
    assert!(r.iter().all(|(_, c, _)| c == "sanity-api"));
    assert!(fx.d.path().join("raw/sanity/data.ndjson").is_file());
    assert!(!fx.d.path().join("raw/sanity/data.ndjson.tmp").exists());
}

#[tokio::test]
async fn api_export_401_is_a_clear_error() {
    let _g = BASE_LOCK.lock().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    unsafe {
        std::env::set_var("PORTER_SANITY_API_BASE", server.uri());
        std::env::set_var("PORTER_SANITY_TOKEN", "bad");
    }
    let fx = project(api("PORTER_SANITY_TOKEN"));
    let e = stage(&fx, Stage::Analyse).await.unwrap_err().to_string();
    assert!(
        e.contains("PORTER_SANITY_TOKEN") && e.contains("401"),
        "{e}"
    );
    assert!(!fx.d.path().join("raw/sanity/data.ndjson").exists());
}

#[tokio::test]
async fn missing_token_env_is_a_clear_error() {
    let fx = project(api("PORTER_SANITY_TOKEN_UNSET_T3"));
    let e = stage(&fx, Stage::Analyse).await.unwrap_err().to_string();
    assert!(e.contains("PORTER_SANITY_TOKEN_UNSET_T3"), "{e}");
}

#[tokio::test]
async fn archive_and_folder_read_the_same_rows() {
    let d = tempfile::tempdir().unwrap();
    let tgz = d.path().join("production.tar.gz");
    build_targz(&tgz);
    let folder = d.path().join("export");
    std::fs::create_dir(&folder).unwrap();
    std::fs::copy(fixture_path(), folder.join("data.ndjson")).unwrap();

    let a = project(export(tgz));
    stage(&a, Stage::Analyse).await.unwrap();
    let f = project(export(folder));
    stage(&f, Stage::Analyse).await.unwrap();

    assert_eq!(rows(&a).len(), fixture_doc_count());
    assert_eq!(rows(&a), rows(&f));
    assert!(rows(&a).iter().all(|(_, c, _)| c == "sanity-export"));
}

#[tokio::test]
async fn drafts_keep_their_prefix_in_raw_rows() {
    let fx = project(export(fixture_path().parent().unwrap().to_path_buf()));
    stage(&fx, Stage::Analyse).await.unwrap();
    let r = rows(&fx);
    let status = |id: &str| r.iter().find(|x| x.0 == id).map(|x| x.2.clone());
    assert_eq!(status("drafts.hb-1"), Some(Some("draft".into())));
    assert_eq!(status("drafts.hb-new"), Some(Some("draft".into())));
    assert_eq!(status("hb-1"), Some(Some("published".into())));
    assert!(r.iter().all(|x| !x.0.starts_with("image-")));
}
