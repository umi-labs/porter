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
            if st == Stage::Normalise {
                return src.normalise(&ctx);
            }
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

// ---- asset fetch ----

fn asset_bytes(n: u8) -> Vec<u8> {
    format!("asset-{n}").into_bytes()
}

/// URL paths of the fixture's four assets, in `asset-1`..`asset-4` order.
const ASSET_PATHS: [&str; 4] = [
    "/images/abc/production/d3dfdcf650653fee4a8b89c828ccad98a8406fdd-800x600.jpg",
    "/images/abc/production/d267af74a142296235941b40d70bc38113a0481e-800x600.jpg",
    "/images/abc/production/365899501b2366f3fc75a025658705cc0f32deba-800x600.jpg",
    "/files/abc/production/ca7fcf72964eb65f1a947189036a170a1489a0bb.pdf",
];

/// A `sanity-export` project over a folder holding the fixture with `cdn.test` rewritten to
/// `cdn`. Returns the project and the folder's guard.
fn asset_project(cdn: &str, assets: SanityAssets) -> (Fx, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("data.ndjson"),
        fixture().replace("http://cdn.test", cdn),
    )
    .unwrap();
    let fx = project(ConnectorConfig::SanityExport {
        path: d.path().to_path_buf(),
        assets,
    });
    (fx, d)
}

async fn serve(server: &MockServer, which: &[usize]) {
    for &i in which {
        Mock::given(method("GET"))
            .and(path(ASSET_PATHS[i]))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(asset_bytes(i as u8 + 1)))
            .mount(server)
            .await;
    }
}

fn asset_statuses(fx: &Fx) -> Vec<porter_model::ItemStatus> {
    fx.p.store()
        .statuses(Stage::Extract)
        .unwrap()
        .into_iter()
        .map(|(_, s)| s)
        .collect()
}

#[tokio::test]
async fn assets_fetches_only_referenced_by_default() {
    let server = MockServer::start().await;
    serve(&server, &[0, 1, 2, 3]).await;
    let (fx, _d) = asset_project(&server.uri(), SanityAssets::Referenced);
    stage(&fx, Stage::Extract).await.unwrap();
    // 3 referenced assets downloaded, the unreferenced image (asset-3) left alone.
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert_eq!(
        fx.p.store()
            .items::<porter_model::Media>(ItemKind::Media)
            .unwrap()
            .len(),
        3
    );
    let m =
        fx.p.store()
            .items::<porter_model::Media>(ItemKind::Media)
            .unwrap();
    let f1 = m
        .iter()
        .find(|(_, _, m)| m.filename == "file1.jpg")
        .unwrap();
    assert_eq!((f1.2.width, f1.2.height), (Some(800), Some(600)));
    assert_eq!(f1.2.mime, "image/jpeg");

    let (fx, _d) = asset_project(&server.uri(), SanityAssets::All);
    server.reset().await;
    serve(&server, &[0, 1, 2, 3]).await;
    stage(&fx, Stage::Extract).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn assets_retries_a_500_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ASSET_PATHS[0]))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    serve(&server, &[0, 1, 3]).await;
    let (fx, _d) = asset_project(&server.uri(), SanityAssets::Referenced);
    let s = stage(&fx, Stage::Extract).await.unwrap();
    assert_eq!(s.failed, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
    assert!(
        asset_statuses(&fx)
            .iter()
            .all(|s| *s == porter_model::ItemStatus::Done)
    );
}

#[tokio::test]
async fn assets_sha1_mismatch_is_an_error() {
    let server = MockServer::start().await;
    serve(&server, &[0, 1, 3]).await;
    // asset-1 is served corrupted.
    server.reset().await;
    serve(&server, &[1, 3]).await;
    Mock::given(method("GET"))
        .and(path(ASSET_PATHS[0]))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"corrupt".to_vec()))
        .mount(&server)
        .await;
    let (fx, _d) = asset_project(&server.uri(), SanityAssets::Referenced);
    let s = stage(&fx, Stage::Extract).await.unwrap();
    assert_eq!((s.failed, s.errors), (1, 1));
    let issues = fx.p.store().issues(&Default::default()).unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, porter_model::IssueKind::MissingMedia);
    assert_eq!(issues[0].severity, porter_model::Severity::Error);
    assert_eq!(
        issues[0].message,
        "image-d3dfdcf650653fee4a8b89c828ccad98a8406fdd-800x600-jpg: downloaded file doesn't match Sanity's SHA-1"
    );
    // Nothing from the bad download was stored.
    assert_eq!(
        fx.p.store()
            .items::<porter_model::Media>(ItemKind::Media)
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn assets_second_extract_skips_downloaded() {
    let server = MockServer::start().await;
    serve(&server, &[0, 1, 3]).await;
    let (fx, _d) = asset_project(&server.uri(), SanityAssets::Referenced);
    stage(&fx, Stage::Extract).await.unwrap();
    server.reset().await;
    let s = stage(&fx, Stage::Extract).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
    assert_eq!((s.skipped, s.done, s.failed), (3, 0, 0));
}

#[tokio::test]
async fn assets_cancel_marks_remaining_partial() {
    let server = MockServer::start().await;
    serve(&server, &[0]).await;
    for i in [1, 3] {
        Mock::given(method("GET"))
            .and(path(ASSET_PATHS[i]))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(asset_bytes(i as u8 + 1))
                    .set_delay(std::time::Duration::from_secs(5)),
            )
            .mount(&server)
            .await;
    }
    let (fx, _d) = asset_project(&server.uri(), SanityAssets::Referenced);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let p2 = fx.p.clone();
    tokio::spawn(async move {
        // Cancel once the first asset has landed.
        while p2
            .store()
            .items::<porter_model::Media>(ItemKind::Media)
            .unwrap()
            .is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        c2.cancel();
    });
    let cfg = Arc::new(fx.p.config().clone());
    let s = run_stage(
        fx.p.clone(),
        cfg,
        EventSink::new(4096),
        cancel,
        Stage::Extract,
        |ctx| async move {
            for c in SanitySource.connectors(&ctx.config)? {
                c.extract(&ctx).await?;
            }
            Ok(())
        },
    )
    .await
    .unwrap();
    assert!(s.interrupted);
    assert_eq!((s.done, s.partial), (1, 2));
}

// ---- normalise ----

/// A project over the fixture with `enquiryForm` listed as submissions, analysed, with the
/// three referenced assets already in the store (as extract leaves them), then normalised.
async fn normalised() -> Fx {
    normalised_with(|_| {}).await
}

async fn normalised_with(tweak: impl FnOnce(&mut porter_engine::config::ProjectConfig)) -> Fx {
    let fx = project(export(fixture_path().parent().unwrap().to_path_buf()));
    let mut cfg = fx.p.config().clone();
    cfg.mapping.forms.submission_types = vec!["enquiryForm".into()];
    tweak(&mut cfg);
    std::fs::write(
        fx.d.path().join("porter.toml"),
        toml::to_string(&cfg).unwrap(),
    )
    .unwrap();
    let fx = Fx {
        p: Arc::new(Project::open(fx.d.path()).unwrap()),
        d: fx.d,
    };
    stage(&fx, Stage::Analyse).await.unwrap();
    for i in [0, 1, 3] {
        fx.p.store()
            .put_media_bytes(
                &asset_bytes(i as u8 + 1),
                &format!("http://cdn.test{}", ASSET_PATHS[i]),
                &format!("file{}", i + 1),
            )
            .unwrap();
    }
    stage(&fx, Stage::Normalise).await.unwrap();
    fx
}

fn canon(kind: ItemKind, sid: &str) -> porter_model::ItemId {
    porter_model::ItemId::new(
        kind,
        &porter_model::SourceRef {
            connector: "sanity".into(),
            source_id: sid.into(),
            url: None,
        },
    )
}

fn sdoc(fx: &Fx, sid: &str) -> porter_model::Document {
    fx.p.store()
        .get_item(canon(ItemKind::Document, sid))
        .unwrap()
        .unwrap_or_else(|| panic!("no document {sid}"))
}

fn all_issues(fx: &Fx) -> Vec<porter_model::Issue> {
    fx.p.store().issues(&Default::default()).unwrap()
}

fn sanity_docs(fx: &Fx) -> Vec<porter_model::Document> {
    let mut v: Vec<_> =
        fx.p.store()
            .items::<Value>(ItemKind::Document)
            .unwrap()
            .into_iter()
            .filter(|(_, s, _)| s.connector == "sanity")
            .map(|(_, _, d)| serde_json::from_value::<porter_model::Document>(d).unwrap())
            .collect();
    v.sort_by_key(|d| d.id.to_string());
    v
}

#[tokio::test]
async fn draft_and_published_merge_into_one_document() {
    use porter_model::{DocStatus, IssueKind, Severity};
    let fx = normalised().await;
    let docs = sanity_docs(&fx);
    assert_eq!(
        docs.iter().filter(|d| d.source.source_id == "hb-1").count(),
        1
    );
    let hb1 = sdoc(&fx, "hb-1");
    assert_eq!(hb1.status, DocStatus::Published);
    assert_eq!(hb1.title, "Beach Break");
    assert_eq!(hb1.slug, "beach-break");
    assert!(
        docs.iter()
            .all(|d| !d.source.source_id.starts_with("drafts."))
    );
    let warn: Vec<_> = all_issues(&fx)
        .into_iter()
        .filter(|i| {
            i.item == Some(hb1.id)
                && i.kind == IssueKind::UnmappedField
                && i.severity == Severity::Warning
        })
        .collect();
    assert_eq!(warn.len(), 1, "{warn:?}");
    assert_eq!(
        warn[0].message,
        "hotelBreak \"Beach Break\" has unpublished changes in Sanity; only the published version is loaded"
    );
    assert_eq!(sdoc(&fx, "hb-new").status, DocStatus::Draft);
}

#[tokio::test]
async fn dangling_strong_and_weak_references() {
    use porter_model::{FieldValue, IssueKind, Severity};
    let fx = normalised().await;
    let rel: Vec<_> = all_issues(&fx)
        .into_iter()
        .filter(|i| i.kind == IssueKind::UnresolvedRelation)
        .collect();
    assert_eq!(rel.len(), 2, "{rel:?}");
    let sev = |s: Severity| rel.iter().filter(|i| i.severity == s).count();
    assert_eq!((sev(Severity::Error), sev(Severity::Warning)), (1, 1));
    let hb2 = sdoc(&fx, "hb-2");
    assert_eq!(hb2.fields["dangling"], FieldValue::Null);
    assert_eq!(hb2.fields["weakDangling"], FieldValue::Null);
    // A resolved reference points at the canonical document.
    assert_eq!(
        hb2.fields["city"],
        FieldValue::Doc(canon(ItemKind::Document, "cityPage-ci1"))
    );
}

#[tokio::test]
async fn image_hotspot_becomes_focal_point() {
    use porter_model::{FieldValue, IssueKind, Media, Severity};
    let fx = normalised().await;
    let hb1 = sdoc(&fx, "hb-1");
    let FieldValue::Media(mid) = hb1.fields["mainImage"] else {
        panic!("{:?}", hb1.fields["mainImage"]);
    };
    let m: Media = fx.p.store().get_item(mid).unwrap().unwrap();
    assert_eq!(m.focal_x, Some(30.0));
    assert_eq!(m.focal_y, Some(40.0));
    assert_eq!(m.alt.as_deref(), Some("Image 1"));
    let crops: Vec<_> = all_issues(&fx)
        .into_iter()
        .filter(|i| i.item == Some(hb1.id) && i.severity == Severity::Info)
        .collect();
    assert_eq!(crops.len(), 1, "{crops:?}");
    assert_eq!(crops[0].kind, IssueKind::UnmappedField);
    assert_eq!(
        crops[0].message,
        "hotelBreak \"Beach Break\" crops image-d3dfdcf650653fee4a8b89c828ccad98a8406fdd-800x600-jpg; Payload has no crop, so the full image is used"
    );
    // The asset's own altText wins over the per-use alt.
    let FieldValue::RichText(rt) = &hb1.fields["description"] else {
        panic!()
    };
    let inline = rt
        .blocks
        .iter()
        .find_map(|b| match b {
            porter_model::RtBlock::Media { media, .. } => Some(*media),
            _ => None,
        })
        .unwrap();
    let m2: Media = fx.p.store().get_item(inline).unwrap().unwrap();
    assert_eq!(m2.alt.as_deref(), Some("A pic"));
    // The PDF on the homepage resolves too.
    assert!(matches!(
        sdoc(&fx, "homepage").fields["brochure"],
        FieldValue::Media(_)
    ));
    assert!(
        !all_issues(&fx)
            .iter()
            .any(|i| i.kind == IssueKind::MissingMedia),
        "{:?}",
        all_issues(&fx)
    );
}

#[tokio::test]
async fn builder_array_becomes_layout_with_modules() {
    use porter_model::{BlockKind, FieldValue};
    let fx = normalised().await;
    let FieldValue::Layout(blocks) = &sdoc(&fx, "hb-1").fields["flexibleContent"] else {
        panic!()
    };
    let modules: Vec<_> = blocks
        .iter()
        .map(|b| b.original.as_ref().unwrap().module.as_str())
        .collect();
    assert_eq!(
        modules,
        ["hero", "cardCarousel", "customCardCarousel", "accordion"]
    );
    assert_eq!(blocks[3].kind, BlockKind::Accordion);
    // An array field that only ever holds one item type is a list, not a builder.
    let FieldValue::List(cards) = &blocks[1].props["cards"] else {
        panic!("{:?}", blocks[1].props["cards"]);
    };
    let FieldValue::Object(card) = &cards[0] else {
        panic!("{:?}", cards[0]);
    };
    assert_eq!(card["_type"], FieldValue::Text("card".into()));
    assert_eq!(card["title"], FieldValue::Text("A".into()));
    assert!(!card.contains_key("_key"));
    assert!(matches!(blocks[3].props["items"], FieldValue::List(_)));
    assert_eq!(blocks[0].kind, BlockKind::Raw);
    assert_eq!(blocks[0].props["heading"], FieldValue::Text("Hello".into()));
    assert!(matches!(blocks[0].props["image"], FieldValue::Media(_)));
    assert!(!blocks[0].props.contains_key("_key"));
}

#[tokio::test]
async fn enquiries_become_form_and_submissions() {
    use porter_model::{Form, Submission};
    let fx = normalised().await;
    let store = fx.p.store();
    let forms = store.items::<Form>(ItemKind::Form).unwrap();
    assert_eq!(forms.len(), 1);
    let (fid, fsrc, form) = &forms[0];
    assert_eq!(fsrc.source_id, "form:enquiryForm");
    let names: Vec<_> = form.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"holidayDetails.destination"), "{names:?}");
    assert!(names.contains(&"holidayDetails.dates.from"), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("_type")), "{names:?}");
    let subs = store.items::<Submission>(ItemKind::Submission).unwrap();
    assert_eq!(subs.len(), 3);
    assert!(subs.iter().all(|(_, _, s)| s.form == *fid));
    let s1 = subs
        .iter()
        .find(|(_, s, _)| s.source_id == "enq-1")
        .unwrap();
    assert_eq!(s1.2.values["holidayDetails.guests"], "1");
    assert_eq!(s1.2.values["email"], "person1@example.com");
    assert!(sanity_docs(&fx).iter().all(|d| d.doc_type != "enquiryForm"));
}

#[tokio::test]
async fn normalise_golden() {
    let fx = normalised().await;
    let docs = serde_json::to_value(sanity_docs(&fx)).unwrap();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sanity/normalised.golden.json");
    if std::env::var_os("PORTER_BLESS").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&docs).unwrap() + "\n").unwrap();
    }
    let want: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(docs, want);
}

#[tokio::test]
async fn second_normalise_is_stable_and_drops_stale() {
    let fx = normalised().await;
    let before = sanity_docs(&fx);
    stage(&fx, Stage::Normalise).await.unwrap();
    assert_eq!(sanity_docs(&fx), before);
    // A document gone from the export is soft-deleted on the next run.
    let raw = fx.d.path().join("raw/sanity/data.ndjson");
    let kept: String = std::fs::read_to_string(&raw)
        .unwrap()
        .lines()
        .filter(|l| !l.contains("\"regionPage-r1\", \"_type\""))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&raw, kept).unwrap();
    stage(&fx, Stage::Normalise).await.unwrap();
    let after = sanity_docs(&fx);
    assert_eq!(after.len(), before.len() - 1);
    assert!(after.iter().all(|d| d.source.source_id != "regionPage-r1"));
}

#[tokio::test]
async fn plugin_documents_are_skipped_with_one_info() {
    use porter_model::{IssueKind, Severity};
    let fx = normalised().await;
    assert!(sanity_docs(&fx).iter().all(|d| d.doc_type != "media.tag"));
    let info: Vec<_> = all_issues(&fx)
        .into_iter()
        .filter(|i| i.message.starts_with("media.tag"))
        .collect();
    assert_eq!(info.len(), 1, "{info:?}");
    assert_eq!(
        (info[0].kind, info[0].severity),
        (IssueKind::UnmappedField, Severity::Info)
    );
    assert_eq!(
        info[0].message,
        "media.tag: plugin or system documents (1) aren't content; skipped"
    );
}

/// R17: after normalise, mapped types get an identity entry for each field the mapping
/// doesn't mention; renamed and dropped entries (and tidy drops) are left exactly as they are.
#[tokio::test]
async fn top_up_adds_only_unmentioned_fields() {
    let fx = normalised().await;
    let f = fx.d.path().join("porter.toml");
    let t = std::fs::read_to_string(&f).unwrap()
        + "\n# keep me\n[mapping.types.hotelBreak]\nto = \"hotelBreak\"\n\n[mapping.types.hotelBreak.fields]\ndescription = { to = \"body\" }\nhighlights = { drop = true }\n\n[[tidy]]\nid = \"x\"\nkind = \"merge-types\"\nfrom = [\"hotelBreak\"]\ninto = \"breaks\"\ndrop = { \"hotelBreak.city\" = \"not wanted\" }\n";
    std::fs::write(&f, t).unwrap();
    let added = porter_engine::mapping::top_up_fields(fx.d.path(), fx.p.store()).unwrap();
    assert!(
        added.contains(&"hotelBreak.mainImage".to_string()),
        "{added:?}"
    );
    assert!(
        added.iter().all(|a| a.starts_with("hotelBreak.")),
        "{added:?}"
    );
    let cfg = porter_engine::config::ProjectConfig::load(&f).unwrap();
    let fields = &cfg.mapping.types["hotelBreak"].fields;
    assert_eq!(fields["description"].to, "body");
    assert!(fields["highlights"].drop);
    assert!(!fields.contains_key("city"));
    assert_eq!(fields["mainImage"].to, "mainImage");
    assert!(std::fs::read_to_string(&f).unwrap().contains("# keep me"));
    // Nothing more to add the second time.
    assert!(
        porter_engine::mapping::top_up_fields(fx.d.path(), fx.p.store())
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn path_pattern_follows_references() {
    use porter_engine::mapping::TypeMap;
    use porter_model::{IssueKind, Severity};
    let fx = normalised_with(|c| {
        let tm = |p: &str| TypeMap {
            path: Some(p.into()),
            ..Default::default()
        };
        c.mapping.types.insert(
            "cityPage".into(),
            tm("/destinations/{country.region.slug}/{country.slug}/{slug}"),
        );
        // Spain has a region; this pattern can't resolve for the city without one.
        c.mapping
            .types
            .insert("countryPage".into(), tm("/c/{region.nope}/{slug}"));
    })
    .await;
    assert_eq!(
        sdoc(&fx, "cityPage-ci1").path,
        "/destinations/europe/spain/madrid"
    );
    // Unresolvable token: warning naming the token, fallback path.
    let c1 = sdoc(&fx, "countryPage-c1");
    assert_eq!(c1.path, "/countryPage/spain/");
    let w: Vec<_> = all_issues(&fx)
        .into_iter()
        .filter(|i| i.item == Some(c1.id) && i.kind == IssueKind::UnresolvedRelation)
        .collect();
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].severity, Severity::Warning);
    assert!(w[0].message.contains("{region.nope}"), "{}", w[0].message);
}

#[tokio::test]
async fn crawl_urls_merge_with_pattern_routes() {
    use porter_engine::mapping::TypeMap;
    use porter_model::{IssueKind, Route, RouteTarget, Severity};
    let old = MockServer::start().await;
    let locs: String = ["/hotel/hb-1/", "/about/"]
        .iter()
        .map(|p| format!("<url><loc>{}{p}</loc></url>", old.uri()))
        .collect();
    let xml =
        format!(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{locs}</urlset>"#);
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(xml, "application/xml"))
        .mount(&old)
        .await;
    for p in ["/hotel/hb-1/", "/about/"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<title>x</title>", "text/html"))
            .mount(&old)
            .await;
    }
    let fx = project(export(fixture_path().parent().unwrap().to_path_buf()));
    let mut cfg = fx.p.config().clone();
    cfg.connectors.push(ConnectorConfig::Crawl {
        url: old.uri(),
        concurrency: 2,
        delay_ms: 0,
        include: vec![],
        exclude: vec![],
        max_pages: Some(50),
        keep_query: vec![],
        max_media_bytes: 1024 * 1024,
    });
    cfg.verify.site_url = Some("https://new.test".into());
    cfg.mapping.forms.submission_types = vec!["enquiryForm".into()];
    cfg.mapping.types.insert(
        "hotelBreak".into(),
        TypeMap {
            path: Some("/hotel/{id}/".into()),
            ..Default::default()
        },
    );
    std::fs::write(
        fx.d.path().join("porter.toml"),
        toml::to_string(&cfg).unwrap(),
    )
    .unwrap();
    let fx = Fx {
        p: Arc::new(Project::open(fx.d.path()).unwrap()),
        d: fx.d,
    };
    stage(&fx, Stage::Analyse).await.unwrap();
    stage(&fx, Stage::Normalise).await.unwrap();

    let hb1 = canon(ItemKind::Document, "hb-1");
    let routes = fx.p.store().items::<Route>(ItemKind::Route).unwrap();
    let merged: Vec<_> = routes
        .iter()
        .filter(|(_, _, r)| r.target == RouteTarget::Document(hb1))
        .collect();
    assert_eq!(merged.len(), 1, "{routes:?}");
    assert_eq!(merged[0].2.http_status, Some(200));
    assert_eq!(merged[0].1.connector, "sanity");
    let about: Vec<_> = routes
        .iter()
        .filter(|(_, s, r)| s.connector == "sanity" && r.url.ends_with("/about/"))
        .collect();
    assert_eq!(about.len(), 1, "{routes:?}");
    assert_eq!(about[0].2.target, RouteTarget::None);
    assert_eq!(about[0].2.http_status, Some(200));
    let issues = all_issues(&fx);
    let warn: Vec<_> = issues
        .iter()
        .filter(|i| i.kind == IssueKind::UnresolvedLink && i.severity == Severity::Warning)
        .collect();
    assert_eq!(warn.len(), 1, "{warn:?}");
    assert_eq!(warn[0].severity, Severity::Warning);
    assert_eq!(warn[0].item, Some(about[0].0));
    assert_eq!(
        warn[0].message,
        format!("old site URL {}/about/ has no document", old.uri())
    );
    let info: Vec<_> = issues
        .iter()
        .filter(|i| i.message.ends_with("wasn't found on the old site"))
        .collect();
    assert_eq!(info.len(), 1, "{info:?}");
    assert_eq!(info[0].severity, Severity::Info);
    assert_eq!(
        info[0].message,
        "https://new.test/hotel/hb-2/ wasn't found on the old site"
    );
    assert!(
        !info.iter().any(|i| i.message.contains("/hotel/hb-1/")),
        "{info:?}"
    );
}

#[tokio::test]
async fn coverage_platform_is_sanity() {
    let fx = project(export(fixture_path().parent().unwrap().to_path_buf()));
    stage(&fx, Stage::Analyse).await.unwrap();
    let c = porter_engine::report::read_coverage(&fx.p)
        .unwrap()
        .unwrap();
    assert_eq!(c.platform.as_deref(), Some("sanity"));
    let k = &c.connectors[0];
    assert_eq!(k.name, "sanity");
    // Published: regionPage 1 + countryPage 2 + cityPage 3 + hotelBreak 2 + siteSettings 1
    // + homepage 1 + enquiryForm 3 = 13; 2 drafts; 4 assets.
    assert_eq!(k.contributed["documents"], 13);
    assert_eq!(k.contributed["drafts"], 2);
    assert_eq!(k.contributed["assets"], 4);
    assert_eq!(k.gaps.len(), 2);
    let a: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.p.raw_dir("sanity").join("analysis.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(a["skipped_types"]["media.tag"], 1);
    assert!(a["types"].get("media.tag").is_none());
    // Sanity's seed leaves other platforms' seeds alone and drops plugin types.
    let mut m = porter_engine::mapping::MappingConfig::seed_from(fx.p.store());
    assert!(m.types.contains_key("media.tag"));
    porter_engine::sources::sanity::seed_mapping(&fx.p, &mut m).unwrap();
    assert!(!m.types.contains_key("media.tag"));
    assert_eq!(m.blocks.len(), 4);
}

/// Files under `dir`, relative, sorted, with their text.
fn tree(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        out.push((name, std::fs::read_to_string(&p).unwrap()));
    }
    out.sort();
    out
}

/// `schema generate --into` over the fixture, with the seeded mapping, is what
/// tests/payload-app imports (the live round trip loads into it). `PORTER_BLESS=1` rewrites it.
#[tokio::test]
async fn generate_into_matches_test_app() {
    use porter_engine::destinations::payload::schema_generate::generate_into;
    let fx = normalised().await;
    let mut cfg = fx.p.config().clone();
    cfg.mapping = porter_engine::mapping::MappingConfig::seed_from(fx.p.store());
    porter_engine::sources::sanity::seed_mapping(&fx.p, &mut cfg.mapping).unwrap();
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/payload-app");
    let tmp = tempfile::tempdir().unwrap();
    let out = if std::env::var_os("PORTER_BLESS").is_some() {
        app.clone()
    } else {
        tmp.path().to_path_buf()
    };
    let o = out.clone();
    let summary = run_stage(
        fx.p.clone(),
        Arc::new(cfg),
        EventSink::new(4096),
        CancellationToken::new(),
        Stage::Schema,
        |ctx| async move {
            let r = generate_into(&ctx, &o)?;
            assert!(r.collisions.is_empty(), "{:?}", r.collisions);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(summary.errors, 0);
    for sub in ["src/collections/porter", "src/blocks/porter"] {
        let (got, want) = (tree(&out.join(sub)), tree(&app.join(sub)));
        assert!(
            !want.is_empty(),
            "{sub} is empty: bless with PORTER_BLESS=1"
        );
        assert_eq!(got, want, "{sub} differs: bless with PORTER_BLESS=1");
    }
}

/// Tidy-up rules over the fixture: compiled into the mapping, accounted before load.
mod tidy {
    use super::*;
    use porter_engine::config::ProjectConfig;
    use porter_engine::destinations::payload::PayloadDestination;
    use porter_engine::destinations::payload::ts::{CollectionSchema, DestField, DestSchema};
    use porter_engine::stage::Destination;
    use porter_engine::tidy::{Problem, account, effective_mapping};
    use porter_model::{IssueKind, Severity};

    /// Suggestions from the normalised fixture (analysis.json exists, so its singletons
    /// rule, not a document count of one, keeps `regionPage` in).
    mod suggest {
        use super::*;
        use porter_engine::tidy::TidyKind;
        use porter_engine::tidy::suggest::append;

        fn from_of(r: &porter_engine::tidy::TidyRule) -> Vec<String> {
            match &r.kind {
                TidyKind::MergeTypes { from, .. } | TidyKind::MergeBlocks { from, .. } => {
                    let mut f = from.clone();
                    f.sort();
                    f
                }
            }
        }

        #[tokio::test]
        async fn suggests_merging_the_linked_page_types() {
            let fx = normalised().await;
            let rules = append(&fx.p).unwrap();
            let r = rules
                .iter()
                .find(|r| matches!(r.kind, TidyKind::MergeTypes { .. }))
                .unwrap();
            // The fixture's cityPage adds `location` and drops nothing, so it only reaches
            // 0.4 against countryPage (below the linked bar); region and country link
            // at 0.67. (The three-way cluster on real data is the unit test.)
            assert_eq!(from_of(r), ["countryPage", "regionPage"]);
            assert!(!r.enabled);
            assert_eq!(
                r.why.as_deref(),
                Some("They share 2 of 3 fields and link to each other")
            );
            let TidyKind::MergeTypes {
                parent,
                discriminator,
                ..
            } = &r.kind
            else {
                unreachable!()
            };
            assert_eq!(parent["countryPage"], "region");
            assert_eq!(discriminator.as_ref().unwrap().field, "kind");
            let on_disk = ProjectConfig::load(&fx.d.path().join("porter.toml")).unwrap();
            assert_eq!(on_disk.tidy, rules);
        }

        #[tokio::test]
        async fn suggests_merging_card_carousels() {
            let fx = normalised().await;
            let rules = append(&fx.p).unwrap();
            let r = rules
                .iter()
                .find(|r| matches!(r.kind, TidyKind::MergeBlocks { .. }))
                .unwrap();
            assert_eq!(from_of(r), ["cardCarousel", "customCardCarousel"]);
            assert_eq!(r.id, "card-carousels");
            assert!(!r.enabled);
        }

        #[tokio::test]
        async fn no_duplicate_suggestions_on_rerun() {
            let fx = normalised().await;
            let first = append(&fx.p).unwrap();
            assert!(!first.is_empty());
            let p = Arc::new(Project::open(fx.d.path()).unwrap());
            assert!(append(&p).unwrap().is_empty());
            assert_eq!(p.config().tidy.len(), first.len());
        }
    }

    const MERGE: &str = r#"
[[tidy]]
id = "destinations"
kind = "merge-types"
enabled = true
from = ["regionPage", "countryPage", "cityPage"]
into = "destinations"
discriminator = { field = "kind", values = { regionPage = "region", countryPage = "country", cityPage = "city" } }
parent = { cityPage = "country", countryPage = "region" }

[[tidy]]
id = "card-carousels"
kind = "merge-blocks"
enabled = true
from = ["cardCarousel", "customCardCarousel"]
into = "card-carousel"
fields = { "customCardCarousel.cards" = "items" }
"#;

    /// The fixture (each document passed through `edit`) normalised, with the seeded
    /// mapping and `MERGE`'s rules in porter.toml.
    async fn fx(edit: impl Fn(&mut Value)) -> Fx {
        let fx = project(export(fixture_path().parent().unwrap().to_path_buf()));
        let src = fx.d.path().join("export");
        std::fs::create_dir_all(&src).unwrap();
        let data: String = fixture()
            .lines()
            .map(|l| {
                let mut v: Value = serde_json::from_str(l).unwrap();
                edit(&mut v);
                v.to_string() + "\n"
            })
            .collect();
        std::fs::write(src.join("data.ndjson"), data).unwrap();
        let mut cfg = fx.p.config().clone();
        cfg.connectors = vec![export(src)];
        cfg.mapping.forms.submission_types = vec!["enquiryForm".into()];
        let save = |cfg: &ProjectConfig| {
            std::fs::write(
                fx.d.path().join("porter.toml"),
                toml::to_string(cfg).unwrap(),
            )
            .unwrap();
            Arc::new(Project::open(fx.d.path()).unwrap())
        };
        let p = save(&cfg);
        let fx = Fx { p, d: fx.d };
        stage(&fx, Stage::Analyse).await.unwrap();
        stage(&fx, Stage::Normalise).await.unwrap();
        cfg.mapping = porter_engine::mapping::MappingConfig::seed_from(fx.p.store());
        porter_engine::sources::sanity::seed_mapping(&fx.p, &mut cfg.mapping).unwrap();
        cfg.tidy = toml::from_str::<ProjectConfig>(MERGE).unwrap().tidy;
        std::fs::write(
            fx.d.path().join("porter.toml"),
            toml::to_string(&cfg).unwrap(),
        )
        .unwrap();
        Fx {
            p: Arc::new(Project::open(fx.d.path()).unwrap()),
            d: fx.d,
        }
    }

    async fn dry_run(fx: &Fx) -> porter_engine::events::Summary {
        let cfg = Arc::new(fx.p.config().clone());
        run_stage(
            fx.p.clone(),
            cfg,
            EventSink::new(4096),
            CancellationToken::new(),
            Stage::Load,
            |ctx| async move {
                PayloadDestination::new(&ctx.config.destination)
                    .load(&ctx, false)
                    .await
            },
        )
        .await
        .unwrap()
    }

    fn previews(fx: &Fx, coll: &str) -> Vec<Value> {
        let dir = fx.d.path().join("raw/load-preview").join(coll);
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn merge_types_compiles_to_one_collection_with_discriminator() {
        let fx = fx(|_| {}).await;
        dry_run(&fx).await;
        let docs = previews(&fx, "destinations");
        assert_eq!(docs.len(), 6, "{docs:?}");
        for ty in ["regionPage", "countryPage", "cityPage"] {
            assert!(previews(&fx, ty).is_empty(), "{ty} not loaded on its own");
        }
        let by_key = |sid: &str| {
            let id = canon(ItemKind::Document, sid).to_string();
            docs.iter()
                .find(|d| d["legacyId"] == id.as_str())
                .unwrap_or_else(|| panic!("{sid} not in destinations"))
                .clone()
        };
        let dry = |sid: &str| json_str(&format!("dry-run:{}", canon(ItemKind::Document, sid)));
        let region = by_key("regionPage-r1");
        assert_eq!(region["kind"], "region");
        assert!(region["parent"].is_null(), "{region}");
        for c in ["countryPage-c1", "countryPage-c2"] {
            let d = by_key(c);
            assert_eq!(d["kind"], "country");
            assert_eq!(d["parent"], dry("regionPage-r1"));
        }
        for (city, country) in [
            ("cityPage-ci1", "countryPage-c1"),
            ("cityPage-ci2", "countryPage-c2"),
            ("cityPage-ci3", "countryPage-c2"),
        ] {
            let d = by_key(city);
            assert_eq!(d["kind"], "city");
            assert_eq!(d["parent"], dry(country), "{city}");
        }
        // Normalise stays a faithful copy: no parent recorded there.
        assert_eq!(sdoc(&fx, "cityPage-ci1").parent, None);
        let report: Value = serde_json::from_str(
            &std::fs::read_to_string(fx.p.reports_dir().join("tidy.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report[0]["rule"], "destinations");
    }

    fn json_str(s: &str) -> Value {
        Value::String(s.into())
    }

    #[tokio::test]
    async fn shape_conflict_blocks_load() {
        let fx = fx(|v| match v["_type"].as_str() {
            Some("cityPage") => v["images"] = serde_json::json!(["a", "b"]),
            Some("countryPage") => v["images"] = serde_json::json!("one"),
            _ => {}
        })
        .await;
        let s = dry_run(&fx).await;
        assert_eq!(porter_engine::events::exit_code(&s), 2);
        let errs: Vec<_> = all_issues(&fx)
            .into_iter()
            .filter(|i| i.kind == IssueKind::SchemaMismatch && i.severity == Severity::Error)
            .collect();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(
            errs[0].message,
            "tidy \"destinations\": images is text in countryPage but list in cityPage"
        );
        assert_eq!(errs[0].item, None);
        assert!(previews(&fx, "destinations").is_empty(), "nothing loaded");
    }

    #[tokio::test]
    async fn existing_target_missing_field_is_unaccounted() {
        let fx = fx(|_| {}).await;
        let cfg = fx.p.config().clone();
        let m = effective_mapping(&cfg).unwrap();
        let mut fields: Vec<DestField> = ["legacyId", "title", "slug"]
            .into_iter()
            .map(String::from)
            .chain(
                ["regionPage", "countryPage", "cityPage"]
                    .iter()
                    .flat_map(|t| m.types[*t].fields.values().map(|f| f.to.clone())),
            )
            .filter(|f| f != "location")
            .map(|name| DestField {
                name,
                kind: "json".into(),
                ..Default::default()
            })
            .collect();
        fields.dedup_by(|a, b| a.name == b.name);
        let mut dest = DestSchema::default();
        dest.collections.insert(
            "destinations".into(),
            CollectionSchema {
                fields,
                ..Default::default()
            },
        );
        let out = account(fx.p.store(), &cfg, Some(&dest)).unwrap();
        assert_eq!(
            out[0].problems,
            vec![Problem::Unaccounted {
                field: "location".into(),
                dest: "destinations".into()
            }]
        );
        // Against a destination porter generates there's nothing to miss.
        assert!(
            account(fx.p.store(), &cfg, None).unwrap()[0]
                .problems
                .is_empty()
        );
    }

    /// The flow graph over the fixture with `MERGE`'s rules.
    mod flow {
        use super::*;
        use porter_engine::flow::{FlowGraph, NodeKind, build};

        fn graph(fx: &Fx) -> FlowGraph {
            build(&fx.p, fx.p.config()).unwrap()
        }

        fn node<'a>(g: &'a FlowGraph, id: &str) -> &'a porter_engine::flow::FlowNode {
            g.nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("no node {id}"))
        }

        fn edges_from(g: &FlowGraph, id: &str) -> Vec<String> {
            g.edges
                .iter()
                .filter(|e| e.from == id)
                .map(|e| e.to.clone())
                .collect()
        }

        #[tokio::test]
        async fn flow_before_dry_run_has_no_counts() {
            let fx = fx(|_| {}).await;
            let g = graph(&fx);
            assert_eq!(g.v, 1);
            assert_eq!(g.counted_at, None);
            assert!(g.nodes.iter().all(|n| n.counts.is_none()), "{g:?}");
            assert!(g.edges.iter().all(|e| e.count.is_none()), "{g:?}");
            assert_eq!(edges_from(&g, "src:cityPage"), ["tidy:destinations"]);
            assert_eq!(edges_from(&g, "tidy:destinations"), ["dst:destinations"]);
            let t = node(&g, "tidy:destinations");
            assert!(t.enabled);
            assert_eq!(
                t.summary,
                "Merges City pages, Country pages and Region pages into Destinations. Each keeps what it was in Kind."
            );
        }

        #[tokio::test]
        async fn flow_after_dry_run_counts_match_load() {
            let fx = fx(|_| {}).await;
            dry_run(&fx).await;
            let g = graph(&fx);
            let run = fx.p.store().runs(Stage::Load).unwrap().pop().unwrap();
            assert_eq!(g.counted_at, run.ended_at);
            let c = |id: &str| node(&g, id).counts.clone().unwrap();
            let city = c("src:cityPage");
            assert_eq!((city.items_in, city.ok, city.failed), (3, 3, 0));
            let t = c("tidy:destinations");
            assert_eq!((t.items_in, t.ok), (6, 6));
            let d = c("dst:destinations");
            assert_eq!((d.items_in, d.ok), (6, 6));
            let e = g.edges.iter().find(|e| e.from == "src:cityPage").unwrap();
            assert_eq!(e.count, Some(3));
            // Every source node's statuses come from the load, and add up.
            for n in g.nodes.iter().filter(|n| n.id.starts_with("src:")) {
                let c = n.counts.as_ref().unwrap();
                assert!(c.ok + c.skipped + c.failed <= c.items_in, "{n:?}");
            }
            let a = c("src:enquiryForm");
            assert_eq!(a.items_in, 3);
        }

        #[tokio::test]
        async fn disabled_rule_is_drawn_and_bypassed() {
            let fx = fx(|_| {}).await;
            let mut cfg = fx.p.config().clone();
            cfg.tidy[0].enabled = false;
            let g = build(&fx.p, &cfg).unwrap();
            let t = node(&g, "tidy:destinations");
            assert!(!t.enabled);
            assert!(
                t.summary.contains("passes straight through"),
                "{}",
                t.summary
            );
            assert!(
                g.edges
                    .iter()
                    .all(|e| e.from != "tidy:destinations" && e.to != "tidy:destinations"),
                "{:?}",
                g.edges
            );
            let to = edges_from(&g, "src:cityPage");
            assert_eq!(to.len(), 1, "{to:?}");
            assert!(
                to[0].starts_with("dst:") && to[0] != "dst:destinations",
                "{to:?}"
            );
            // What the rule would do is still shown.
            assert!(t.example.is_some());
        }

        #[tokio::test]
        async fn submissions_never_have_examples() {
            let fx = fx(|_| {}).await;
            dry_run(&fx).await;
            let g = graph(&fx);
            let subs: Vec<_> = g
                .nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::Submissions | NodeKind::Form))
                .collect();
            assert!(!subs.is_empty(), "{:?}", g.nodes);
            assert!(subs.iter().all(|n| n.example.is_none()), "{subs:?}");
            let json = serde_json::to_string(&g).unwrap();
            for personal in ["person1@example.com", "Person 1", "Hello"] {
                assert!(!json.contains(personal), "{personal} leaked");
            }
            // Merged documents do get one.
            let ex = node(&g, "tidy:destinations").example.clone().unwrap();
            assert!(
                ex.changed
                    .iter()
                    .any(|(f, _, a)| f == "kind" && a == "region"),
                "{ex:?}"
            );
        }

        /// A type that looks like contact details never gives an example, even below the
        /// submission threshold and under a merge rule; nor does a link to one show its title.
        #[tokio::test]
        async fn contact_like_types_never_have_examples() {
            let fx = fx(|v| {
                if v["_type"] == "countryPage" {
                    v["email"] = serde_json::json!("jo.contact@example.test");
                    v["details"] = serde_json::json!({"_type": "details", "phone": "0700 900 123"});
                }
            })
            .await;
            dry_run(&fx).await;
            let mut cfg = fx.p.config().clone();
            // Drop and rename on the contact type too, so a value would otherwise show.
            if let porter_engine::tidy::TidyKind::MergeTypes { drop, .. } = &mut cfg.tidy[0].kind {
                drop.insert("countryPage.email".into(), "contact".into());
            }
            for on in [true, false] {
                cfg.tidy[0].enabled = on;
                let g = build(&fx.p, &cfg).unwrap();
                assert!(node(&g, "src:countryPage").example.is_none());
                assert!(node(&g, "tidy:destinations").example.is_none(), "on={on}");
                let json = serde_json::to_string(&g).unwrap();
                for v in [
                    "jo.contact@example.test",
                    "0700 900 123",
                    "Spain",
                    "Portugal",
                ] {
                    assert!(!json.contains(v), "{v} leaked (on={on}): {json}");
                }
            }
        }

        #[tokio::test]
        async fn flow_json_shape_golden() {
            let fx = fx(|_| {}).await;
            dry_run(&fx).await;
            let mut got = serde_json::to_value(graph(&fx)).unwrap();
            assert!(got["counted_at"].is_string());
            got.as_object_mut().unwrap().remove("counted_at");
            let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/sanity/flow.golden.json");
            if std::env::var("PORTER_BLESS").is_ok() {
                std::fs::write(&p, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
            }
            let want: Value = serde_json::from_str(
                &std::fs::read_to_string(&p).expect("bless with PORTER_BLESS=1"),
            )
            .unwrap();
            assert_eq!(got, want, "differs: bless with PORTER_BLESS=1");
        }
    }

    /// R18: under an enabled rule, a field (or block prop) holding data with no mapping
    /// entry and no drop is an error that blocks load.
    #[tokio::test]
    async fn unlisted_field_with_data_blocks_load() {
        let fx = fx(|_| {}).await;
        let mut cfg = fx.p.config().clone();
        cfg.mapping
            .types
            .get_mut("cityPage")
            .unwrap()
            .fields
            .shift_remove("location");
        let card = cfg
            .mapping
            .blocks
            .iter_mut()
            .find(|b| b.r#match.module.as_deref() == Some("cardCarousel"))
            .unwrap();
        card.fields.shift_remove("cards");
        let out = account(fx.p.store(), &cfg, None).unwrap();
        assert_eq!(
            out[0].problems,
            vec![Problem::Unlisted {
                item_type: "cityPage".into(),
                field: "location".into(),
                count: 1
            }]
        );
        assert_eq!(
            out[1].problems,
            vec![Problem::Unlisted {
                item_type: "cardCarousel".into(),
                field: "cards".into(),
                count: 3
            }]
        );
        assert_eq!(
            out[0].problems[0].message("destinations"),
            "tidy \"destinations\": cityPage.location holds data in 1 item(s) but has no mapping entry; map it or drop it"
        );
    }

    /// R18: outside tidy rules, unmapped data is a warning per type and field, with a count.
    #[tokio::test]
    async fn straight_move_warns_on_unmapped_data() {
        let fx = fx(|_| {}).await;
        let mut cfg = fx.p.config().clone();
        cfg.tidy.clear();
        cfg.mapping
            .types
            .get_mut("hotelBreak")
            .unwrap()
            .fields
            .shift_remove("city");
        std::fs::write(
            fx.d.path().join("porter.toml"),
            toml::to_string(&cfg).unwrap(),
        )
        .unwrap();
        let fx = Fx {
            p: Arc::new(Project::open(fx.d.path()).unwrap()),
            d: fx.d,
        };
        dry_run(&fx).await;
        let w: Vec<_> = all_issues(&fx)
            .into_iter()
            .filter(|i| i.kind == IssueKind::UnmappedField && i.message.contains("hotelBreak.city"))
            .collect();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].severity, Severity::Warning);
        assert_eq!(
            w[0].message,
            "`hotelBreak.city` holds data in 3 document(s) but has no mapping entry: not loaded; map it or set drop = true"
        );
    }

    #[tokio::test]
    async fn accounting_counts_in_out() {
        let fx = fx(|_| {}).await;
        let out = account(fx.p.store(), fx.p.config(), None).unwrap();
        let d = &out[0];
        assert_eq!(d.rule, "destinations");
        assert_eq!((d.items_in, d.items_out, d.dropped), (6, 6, 0));
        assert!(d.carried > 0 && d.problems.is_empty(), "{d:?}");
        let b = &out[1];
        assert_eq!(b.rule, "card-carousels");
        assert!(b.items_in >= 2 && b.items_in == b.items_out, "{b:?}");
        assert!(b.renamed >= 1, "cards -> items: {b:?}");
        assert!(b.problems.is_empty(), "{b:?}");
    }
}
