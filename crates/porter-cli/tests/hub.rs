//! `porter hub …` against a fake hub and a fake S3 (two wiremock servers, so two origins).

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Output;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "tok-SECRET-1234567890";
const SLUG: &str = "acme";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn b64(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// Runs porter off the async runtime; returns (exit code, stdout, stderr).
async fn porter(
    dir: &Path,
    hub: &str,
    env_url: Option<&str>,
    args: &[&str],
) -> (i32, String, String) {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_porter"));
    c.arg("--project").arg(dir).args(args);
    c.args(["--url", hub, "--slug", SLUG]);
    c.env("PORTER_HUB_TOKEN", TOKEN);
    match env_url {
        Some(u) => c.env("PORTER_HUB_URL", u),
        None => c.env_remove("PORTER_HUB_URL"),
    };
    c.stdin(std::process::Stdio::null());
    let out: Output = tokio::task::spawn_blocking(move || c.output().unwrap())
        .await
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

async fn run(dir: &Path, hub: &MockServer, args: &[&str]) -> (i32, String, String) {
    porter(dir, &hub.uri(), Some(&hub.uri()), args).await
}

fn project(dir: &Path, media: &[&[u8]]) -> Vec<String> {
    std::fs::write(dir.join("porter.toml"), "[project]\nname = \"demo\"\n").unwrap();
    std::fs::write(dir.join("notes.txt"), "hello").unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    media
        .iter()
        .map(|b| {
            let h = sha(b);
            let p = dir.join("media").join(&h[0..2]).join(&h[2..4]);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join(&h), b).unwrap();
            h
        })
        .collect()
}

fn api(p: &str) -> String {
    format!("/api/projects/{SLUG}/{p}")
}

fn no_auth(reqs: &[Request]) {
    for r in reqs {
        assert!(
            !r.headers.contains_key("authorization"),
            "auth header sent to storage: {}",
            r.url
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn push_refuses_on_409() {
    let hub = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(api("versions/init")))
        .respond_with(
            ResponseTemplate::new(409).set_body_json(json!({"error": "stale", "latest": 3})),
        )
        .expect(1)
        .mount(&hub)
        .await;
    let d = tempfile::tempdir().unwrap();
    project(d.path(), &[]);
    let (code, out, err) = run(d.path(), &hub, &["--json", "hub", "push"]).await;
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("hub has version 3; pull first or --force"),
        "{err}"
    );
    assert_eq!(out, "");
    assert!(!d.path().join(".porter-hub.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn push_uploads_only_missing_media_and_treats_412_as_uploaded() {
    let (hub, s3) = (MockServer::start().await, MockServer::start().await);
    let d = tempfile::tempdir().unwrap();
    let hashes = project(d.path(), &[b"one", b"two"]);
    let (have, missing) = (&hashes[0], &hashes[1]);
    Mock::given(method("POST"))
        .and(path(api("versions/init")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "upload_id": "11111111-1111-1111-1111-111111111111",
            "missing": [missing],
            "snapshot_put_url": format!("{}/snap?X-Amz-Signature=abc", s3.uri()),
        })))
        .mount(&hub)
        .await;
    Mock::given(method("POST"))
        .and(path(api(
            "versions/uploads/11111111-1111-1111-1111-111111111111/media-urls",
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"urls": {missing.as_str(): format!("{}/media/{missing}", s3.uri())}}),
        ))
        .expect(1)
        .mount(&hub)
        .await;
    Mock::given(method("POST"))
        .and(path(api("versions/commit")))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"number": 4, "created_at": "x"})),
        )
        .expect(1)
        .mount(&hub)
        .await;
    // The snapshot is "already there" (a retried push): 412 counts as uploaded.
    Mock::given(method("PUT"))
        .and(path("/snap"))
        .and(header("if-none-match", "*"))
        .respond_with(ResponseTemplate::new(412))
        .expect(1)
        .mount(&s3)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/media/{missing}")))
        .and(header("if-none-match", "*"))
        .and(header(
            "x-amz-checksum-sha256",
            b64(&Sha256::digest(b"two")).as_str(),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s3)
        .await;

    let (code, out, err) = run(d.path(), &hub, &["--json", "hub", "push", "-m", "hi"]).await;
    assert_eq!(code, 0, "{err}");
    let events: Vec<Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events.last().unwrap()["event"], "hub-pushed");
    assert_eq!(events.last().unwrap()["version"], 4);

    let s3_reqs = s3.received_requests().await.unwrap();
    assert_eq!(s3_reqs.len(), 2, "only the snapshot and the missing media");
    assert!(!s3_reqs.iter().any(|r| r.url.path().contains(have.as_str())));
    no_auth(&s3_reqs);

    let hub_reqs = hub.received_requests().await.unwrap();
    let init: Value = serde_json::from_slice(&hub_reqs[0].body).unwrap();
    assert_eq!(init["media_hashes"].as_array().unwrap().len(), 2);
    assert_eq!(init["base_version"], Value::Null);
    // The snapshot holds porter.toml and notes.txt, never media/ or dot-entries.
    let snap = &s3_reqs
        .iter()
        .find(|r| r.url.path() == "/snap")
        .unwrap()
        .body;
    assert_eq!(init["snapshot_sha256"], sha(snap));
    let mut ar = tar::Archive::new(zstd::Decoder::new(&snap[..]).unwrap());
    let names: Vec<String> = ar
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().display().to_string())
        .collect();
    assert_eq!(names, ["notes.txt", "porter.toml"]);
    let commit: Value = serde_json::from_slice(&hub_reqs.last().unwrap().body).unwrap();
    assert_eq!(commit["message"], "hi");

    let sync: Value =
        serde_json::from_str(&std::fs::read_to_string(d.path().join(".porter-hub.json")).unwrap())
            .unwrap();
    assert_eq!(sync["version"], 4);
    assert_eq!(sync["project"], SLUG);
    assert_eq!(
        sync["porter_toml_sha256"],
        sha(b"[project]\nname = \"demo\"\n")
    );
}

fn tar_zst(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(zstd::Encoder::new(Vec::new(), 3).unwrap());
    for (name, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        // Raw name bytes, so a hostile `../x` can be written (set_path refuses it).
        let raw = &mut h.as_old_mut().name;
        raw[..name.len()].copy_from_slice(name.as_bytes());
        h.set_cksum();
        b.append(&h, *data).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

async fn serve_version(hub: &MockServer, s3: &MockServer, n: u64, snap: &[u8], media: &[&[u8]]) {
    let hashes: Vec<String> = media.iter().map(|m| sha(m)).collect();
    Mock::given(method("GET"))
        .and(path(api(&format!("versions/{n}"))))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": n, "snapshot_sha256": sha(snap),
            "snapshot_get_url": format!("{}/snap/{n}", s3.uri()),
            "media_hashes": hashes,
        })))
        .mount(hub)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/snap/{n}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(snap.to_vec()))
        .mount(s3)
        .await;
    let urls: serde_json::Map<String, Value> = hashes
        .iter()
        .map(|h| (h.clone(), json!(format!("{}/m/{h}", s3.uri()))))
        .collect();
    Mock::given(method("POST"))
        .and(path(api("media/urls")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"urls": urls, "missing": []})),
        )
        .mount(hub)
        .await;
    for (h, m) in hashes.iter().zip(media) {
        Mock::given(method("GET"))
            .and(path(format!("/m/{h}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(m.to_vec()))
            .mount(s3)
            .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_downloads_verifies_and_records_the_sync() {
    let (hub, s3) = (MockServer::start().await, MockServer::start().await);
    let snap = tar_zst(&[
        ("porter.toml", b"[project]\nname = \"p\"\n"),
        ("raw/a.json", b"{}"),
    ]);
    serve_version(&hub, &s3, 7, &snap, &[b"img"]).await;
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("project.db-wal"), "stale").unwrap();
    let (code, out, err) = run(
        d.path(),
        &hub,
        &["--json", "hub", "pull", "--version", "7", "--force"],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("\"hub-pulled\""), "{out}");
    assert_eq!(std::fs::read(d.path().join("raw/a.json")).unwrap(), b"{}");
    let h = sha(b"img");
    assert_eq!(
        std::fs::read(
            d.path()
                .join("media")
                .join(&h[0..2])
                .join(&h[2..4])
                .join(&h)
        )
        .unwrap(),
        b"img"
    );
    no_auth(&s3.received_requests().await.unwrap());
    // A clean pull is followed by another one without --force.
    let (code, _, err) = run(d.path(), &hub, &["hub", "pull", "--version", "7"]).await;
    assert_eq!(code, 0, "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_refuses_on_local_changes() {
    let hub = MockServer::start().await;
    let d = tempfile::tempdir().unwrap();
    project(d.path(), &[]);
    // No sync file, non-empty project.
    let (code, _, err) = run(d.path(), &hub, &["hub", "pull"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("--force"), "{err}");
    // A sync file whose porter.toml hash no longer matches.
    std::fs::write(
        d.path().join(".porter-hub.json"),
        json!({"hub": hub.uri(), "project": SLUG, "version": 2,
               "project_db_sha256": null, "porter_toml_sha256": sha(b"old")})
        .to_string(),
    )
    .unwrap();
    let (code, _, err) = run(d.path(), &hub, &["hub", "pull"]).await;
    assert_eq!(code, 1);
    assert!(
        err.contains("porter.toml changed since the last sync (v2)"),
        "{err}"
    );
    assert!(hub.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_rejects_an_unsafe_tar_entry() {
    let (hub, s3) = (MockServer::start().await, MockServer::start().await);
    let snap = tar_zst(&[("porter.toml", b"x"), ("../escaped.txt", b"pwned")]);
    serve_version(&hub, &s3, 1, &snap, &[]).await;
    let parent = tempfile::tempdir().unwrap();
    let d = parent.path().join("proj");
    std::fs::create_dir(&d).unwrap();
    let (code, _, err) = run(&d, &hub, &["hub", "pull", "--version", "1"]).await;
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("unsafe path in snapshot: ../escaped.txt"),
        "{err}"
    );
    assert!(!parent.path().join("escaped.txt").exists());
    assert!(!d.join("porter.toml").exists(), "nothing is swapped in");
    let leftovers: Vec<_> = std::fs::read_dir(&d).unwrap().collect();
    assert!(leftovers.is_empty(), "temp dir cleaned up");
}

const EV1: &str = r#"{"v":1,"event":"stage-started","stage":"analyse","run":"0192f3a0-0000-7000-8000-000000000000"}"#;
const EV2: &str = r#"{"event":"hub-note","kind":"access-gap","message":"wp-db skipped"}"#;
const EV3: &str =
    r#"{"v":1,"event":"progress","stage":"analyse","done":1,"total":2,"label":"pages"}"#;
const RUN_ID: &str = "22222222-2222-2222-2222-222222222222";

fn end_event(state: &str, exit: i64) -> String {
    format!(
        "event: end\ndata: {}\n\n",
        json!({"id": RUN_ID, "state": state, "exit_code": exit, "result_version": null, "error": null})
    )
}

async fn mount_run(hub: &MockServer) {
    Mock::given(method("POST"))
        .and(path(api("runs")))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"id": RUN_ID, "state": "queued", "base_version": 1})),
        )
        .mount(hub)
        .await;
}

fn sse(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

#[tokio::test(flavor = "multi_thread")]
async fn run_streams_events_as_ndjson() {
    let hub = MockServer::start().await;
    mount_run(&hub).await;
    Mock::given(method("GET"))
        .and(path(api(&format!("runs/{RUN_ID}/events"))))
        .respond_with(sse(format!(
            "retry: 10\n\nid: 1\ndata: {EV1}\n\n: ping\n\nid: 2\ndata: {EV2}\n\n{}",
            end_event("finished", 2)
        )))
        .mount(&hub)
        .await;
    let d = tempfile::tempdir().unwrap();
    let (code, out, err) = run(d.path(), &hub, &["--json", "hub", "run", "analyse"]).await;
    assert_eq!(code, 2, "finished with porter exit 2: {err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[1], EV1, "porter events verbatim");
    assert_eq!(lines[2], EV2, "hub events too");
    for l in &lines {
        serde_json::from_str::<Value>(l).expect("pure NDJSON");
    }
    let post: Value =
        serde_json::from_slice(&hub.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(post["stages"], json!(["analyse"]));
    assert_eq!(post["apply"], false);

    // Human mode renders like a local run, on stderr.
    let (code, out, err) = run(d.path(), &hub, &["hub", "run", "analyse"]).await;
    assert_eq!(code, 2);
    assert_eq!(out, "");
    assert!(err.contains("analyse: started"), "{err}");
    assert!(err.contains("hub: wp-db skipped"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_reconnects_with_last_event_id() {
    let hub = MockServer::start().await;
    mount_run(&hub).await;
    let events = api(&format!("runs/{RUN_ID}/events"));
    Mock::given(method("GET"))
        .and(path(events.clone()))
        .and(header("last-event-id", "2"))
        .respond_with(sse(format!(
            "id: 3\ndata: {EV3}\n\n{}",
            end_event("cancelled", 3)
        )))
        .with_priority(1)
        .mount(&hub)
        .await;
    // The first connection drops after two events.
    Mock::given(method("GET"))
        .and(path(events))
        .respond_with(sse(format!(
            "retry: 10\n\nid: 1\ndata: {EV1}\n\nid: 2\ndata: {EV2}\n\n"
        )))
        .with_priority(2)
        .mount(&hub)
        .await;
    let d = tempfile::tempdir().unwrap();
    let (code, out, err) = run(d.path(), &hub, &["--json", "hub", "run", "all"]).await;
    assert_eq!(code, 3, "cancelled maps to 3: {err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(&lines[1..4], &[EV1, EV2, EV3]);
    let post: Value =
        serde_json::from_slice(&hub.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(post["stages"], "all");
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_without_confirm_host_is_refused() {
    let hub = MockServer::start().await;
    let d = tempfile::tempdir().unwrap();
    let (code, out, err) = run(d.path(), &hub, &["--json", "hub", "run", "load", "--apply"]).await;
    assert_eq!(code, 2);
    assert_eq!(out, "");
    assert!(err.contains("--confirm-host"), "{err}");
    assert!(hub.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn token_never_printed() {
    let (hub, s3) = (MockServer::start().await, MockServer::start().await);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "unauthorized"})))
        .mount(&hub)
        .await;
    Mock::given(method("POST"))
        .and(path(api("versions/init")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "upload_id": "11111111-1111-1111-1111-111111111111", "missing": [],
            "snapshot_put_url": format!("{}/snap", s3.uri()),
        })))
        .mount(&hub)
        .await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string("<Error><Code>AccessDenied</Code></Error>"),
        )
        .mount(&s3)
        .await;
    let d = tempfile::tempdir().unwrap();
    project(d.path(), &[]);
    for args in [
        &["-vvv", "hub", "whoami"][..],
        &["-vvv", "--json", "hub", "uploads"],
        &["-vvv", "hub", "push"],
        &["-vvv", "hub", "pull", "--force"],
    ] {
        let (code, out, err) = run(d.path(), &hub, args).await;
        assert_eq!(code, 1, "{args:?}: {err}");
        assert!(
            !out.contains(TOKEN) && !err.contains(TOKEN),
            "{args:?} leaked the token"
        );
        assert!(!err.contains("SECRET"), "{args:?}");
    }
    // The hub did get it, as a bearer token; storage never did.
    let reqs = hub.received_requests().await.unwrap();
    assert_eq!(
        reqs[0].headers["authorization"],
        format!("Bearer {TOKEN}").as_str()
    );
    no_auth(&s3.received_requests().await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn env_token_only_goes_to_the_hub_named_by_porter_hub_url() {
    let (hub, other) = (MockServer::start().await, MockServer::start().await);
    Mock::given(method("GET"))
        .and(path("/api/auth/get-session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"user": {"id": "u", "email": "a@b.c", "name": "A"}})),
        )
        .mount(&hub)
        .await;
    let d = tempfile::tempdir().unwrap();
    // porter.toml points at another origin than PORTER_HUB_URL: refused, nothing sent.
    std::fs::write(
        d.path().join("porter.toml"),
        format!("[hub]\nurl = \"{}\"\nproject = \"{SLUG}\"\n", other.uri()),
    )
    .unwrap();
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_porter"));
    c.arg("--project")
        .arg(d.path())
        .args(["hub", "whoami"])
        .env("PORTER_HUB_TOKEN", TOKEN)
        .env("PORTER_HUB_URL", hub.uri());
    let out = tokio::task::spawn_blocking(move || c.output().unwrap())
        .await
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        err.contains("PORTER_HUB_TOKEN") && err.contains("PORTER_HUB_URL"),
        "{err}"
    );
    assert!(err.contains(&other.uri()), "{err}");
    assert!(!err.contains(TOKEN));
    assert!(other.received_requests().await.unwrap().is_empty());
    // Unset PORTER_HUB_URL: refused too.
    let (code, _, err) = porter(d.path(), &other.uri(), None, &["hub", "whoami"]).await;
    assert_eq!(code, 1, "{err}");
    assert!(other.received_requests().await.unwrap().is_empty());
    // Matching PORTER_HUB_URL: works.
    let (code, out, err) = run(d.path(), &hub, &["hub", "whoami"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("a@b.c"));
}

#[tokio::test(flavor = "multi_thread")]
async fn link_keeps_comments() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("porter.toml"),
        "# mine\n[project]\nname = \"x\" # keep\n",
    )
    .unwrap();
    let (code, _, err) = porter(
        d.path(),
        "https://hub.example.com/",
        None,
        &["hub", "link", "acme"],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    let t = std::fs::read_to_string(d.path().join("porter.toml")).unwrap();
    assert!(t.contains("# mine") && t.contains("# keep"), "{t}");
    assert!(
        t.contains("[hub]\nurl = \"https://hub.example.com\"\nproject = \"acme\""),
        "{t}"
    );
}

/// Against a real hub: `PORTER_HUB_TEST_URL`, `PORTER_HUB_TOKEN`, `PORTER_HUB_TEST_SLUG`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn real_hub_whoami_and_pull() {
    let Ok(url) = std::env::var("PORTER_HUB_TEST_URL") else {
        return;
    };
    let slug = std::env::var("PORTER_HUB_TEST_SLUG").unwrap_or_else(|_| "demo-wxr".into());
    let d = tempfile::tempdir().unwrap();
    for args in [vec!["hub", "whoami"], vec!["hub", "pull", "--force"]] {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_porter"));
        c.arg("--project")
            .arg(d.path())
            .args(&args)
            .args(["--url", &url, "--slug", &slug]);
        c.env("PORTER_HUB_URL", &url);
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(d.path().join("porter.toml").exists());
}
