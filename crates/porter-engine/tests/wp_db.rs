//! Docker tests run only with PORTER_DOCKER_TESTS=1. They need the `mariadb:11` and
//! `wordpress:cli` images and, the first time, network access to download WordPress core and
//! ACF into target/ (versions in tests/fixtures/wp-db/versions).
use porter_engine::config::ConnectorConfig;
use porter_engine::connectors::RawItem;
use porter_engine::events::{EventSink, Summary};
use porter_engine::project::Project;
use porter_engine::sources::wordpress::db::WpDbConnector;
use porter_engine::stage::{Connector, run_stage};
use porter_engine::store::IssueFilter;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, SourceRef, Stage};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const EXPORT_PHP: &str = include_str!("../src/sources/wordpress/export.php");

fn docker_tests() -> bool {
    let on = std::env::var("PORTER_DOCKER_TESTS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set PORTER_DOCKER_TESTS=1 to run Docker tests");
    }
    on
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wp-db")
}

fn version(key: &str) -> String {
    let text = std::fs::read_to_string(fixture_dir().join("versions")).unwrap();
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")))
        .unwrap()
        .trim()
        .to_string()
}

fn id_of(flag: &str) -> String {
    let out = Command::new("id").arg(flag).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// WordPress core + ACF, downloaded once into target/ and reused.
fn wp_root() -> PathBuf {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (wp, acf) = (version("WP_VERSION"), version("ACF_VERSION"));
    let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wp-db-fixture");
    let root = cache.join(format!("wp-{wp}-acf-{acf}"));
    if root
        .join("wp-content/plugins/advanced-custom-fields/acf.php")
        .exists()
    {
        return root;
    }
    let tmp = cache.join(format!("building-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let script = format!(
        "php -d memory_limit=1G /usr/local/bin/wp core download --version={wp} --quiet \
         && cd wp-content/plugins \
         && curl -fsSL -o acf.zip https://downloads.wordpress.org/plugin/advanced-custom-fields.{acf}.zip \
         && unzip -q acf.zip && rm acf.zip"
    );
    let ok = Command::new("docker")
        .args(["run", "--rm", "-u"])
        .arg(format!("{}:{}", id_of("-u"), id_of("-g")))
        .args(["-e", "HOME=/tmp", "-v"])
        .arg(format!(
            "{}:/var/www/html",
            tmp.canonicalize().unwrap().display()
        ))
        .args(["wordpress:cli", "sh", "-c", &script])
        .status()
        .unwrap();
    assert!(ok.success(), "downloading WordPress core and ACF failed");
    std::fs::rename(&tmp, &root).unwrap();
    root
}

async fn extract(
    name: &str,
    dump: &Path,
    wp_root: Option<PathBuf>,
) -> (tempfile::TempDir, Arc<Project>, anyhow::Result<Summary>) {
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(dir.path(), name, "wordpress").unwrap());
    let conn = Arc::new(WpDbConnector::new(&ConnectorConfig::WpDb {
        dump: dump.to_path_buf(),
        wp_root,
        table_prefix: "wp_".into(),
        site_url: None,
    }));
    let r = run_stage(
        p.clone(),
        Arc::new(p.config().clone()),
        EventSink::new(4096),
        CancellationToken::new(),
        Stage::Extract,
        |ctx| async move { conn.extract(&ctx).await },
    )
    .await;
    (dir, p, r)
}

fn export_json(p: &Project) -> Value {
    let f = std::fs::File::open(p.raw_dir("wp-db").join("export.json")).unwrap();
    serde_json::from_reader(std::io::BufReader::new(f)).unwrap()
}

fn posts_of<'a>(v: &'a Value, ty: &str) -> Vec<&'a Value> {
    v["posts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["type"] == ty)
        .collect()
}

fn item(kind: ItemKind, source_id: &str) -> ItemId {
    let src = SourceRef {
        connector: "wp-db".into(),
        source_id: source_id.into(),
        url: None,
    };
    ItemId::new(kind, &src)
}

#[tokio::test]
async fn wpdb_exports_everything_with_wp_root() {
    if !docker_tests() {
        return;
    }
    let root = wp_root();
    let (_d, p, r) = extract("wpdb full", &fixture_dir().join("dump.sql.gz"), Some(root)).await;
    let summary = r.unwrap();
    assert_eq!(summary.failed, 0, "{summary:?}");
    let v = export_json(&p);

    let posts = posts_of(&v, "post");
    let pages = posts_of(&v, "page");
    assert_eq!(posts.len(), 3, "posts");
    assert_eq!(pages.len(), 2, "pages");

    let parent = pages.iter().find(|p| p["title"] == "Parent Page").unwrap();
    let child = pages.iter().find(|p| p["title"] == "Child Page").unwrap();
    assert_eq!(child["parent"], parent["id"]);
    assert!(
        child["link"]
            .as_str()
            .unwrap()
            .ends_with("/parent-page/child-page/")
    );

    let first = posts.iter().find(|p| p["title"] == "First Post").unwrap();
    assert_eq!(
        first["acf"]["subtitle"], "A subtitle",
        "ACF value under its name"
    );
    assert_eq!(first["meta"]["_yoast_wpseo_title"], "First Post SEO title");
    let groups = v["acf_field_groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["fields"][0]["name"], "subtitle");

    let private = posts.iter().find(|p| p["status"] == "private").unwrap();
    let link = private["link"].as_str().unwrap();
    assert!(
        link.ends_with("/private-post/") && !link.contains('?'),
        "private permalink: {link}"
    );

    let menus = v["menus"].as_array().unwrap();
    assert_eq!(menus.len(), 1);
    assert_eq!(menus[0]["items"].as_array().unwrap().len(), 2);
    assert_eq!(v["site"]["permalink_structure"], "/%postname%/");

    // Redirection rows via SQL, as NDJSON, each with a RawItem row and a status.
    let redirects = std::fs::read_to_string(p.raw_dir("wp-db").join("redirection.ndjson")).unwrap();
    let rows: Vec<Value> = redirects
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    let rid = item(ItemKind::Redirect, &format!("redirect:{}", rows[0]["id"]));
    assert_eq!(
        p.store().status(Stage::Extract, rid).unwrap(),
        Some(ItemStatus::Done)
    );

    // Every status has a RawItem row at the same id.
    let id = item(ItemKind::Document, &format!("post:{}", first["id"]));
    let row: RawItem = p.store().get_item(id).unwrap().unwrap();
    assert_eq!(row.title.as_deref(), Some("First Post"));
    assert_eq!(
        p.store().status(Stage::Extract, id).unwrap(),
        Some(ItemStatus::Done)
    );
    for (id, _) in p.store().statuses(Stage::Extract).unwrap() {
        assert!(p.store().get_item::<RawItem>(id).unwrap().is_some());
    }
    let settings = item(ItemKind::Settings, "settings");
    assert!(p.store().get_item::<RawItem>(settings).unwrap().is_some());

    // No access gaps when WordPress itself ran.
    let gaps = p.store().issues(&IssueFilter::default()).unwrap();
    assert!(
        gaps.iter().all(|i| i.kind != IssueKind::AccessGap),
        "{gaps:?}"
    );
}

#[tokio::test]
async fn wpdb_fallback_reports_gaps() {
    if !docker_tests() {
        return;
    }
    let (_d, p, r) = extract("wpdb fallback", &fixture_dir().join("dump.sql.gz"), None).await;
    r.unwrap();
    let msgs: Vec<String> = p
        .store()
        .issues(&IssueFilter::default())
        .unwrap()
        .into_iter()
        .filter(|i| i.kind == IssueKind::AccessGap)
        .map(|i| i.message)
        .collect();
    for want in [
        "ACF field names unavailable (keys only)",
        "permalinks reconstructed from permalink_structure",
        "form definitions unavailable",
    ] {
        assert!(
            msgs.iter().any(|m| m == want),
            "missing {want:?} in {msgs:?}"
        );
    }
    let v = export_json(&p);
    assert_eq!(posts_of(&v, "post").len(), 3);
    let pages = posts_of(&v, "page");
    let child = pages.iter().find(|p| p["title"] == "Child Page").unwrap();
    assert!(
        child["link"]
            .as_str()
            .unwrap()
            .ends_with("/parent-page/child-page/")
    );
    let first = posts_of(&v, "post")
        .into_iter()
        .find(|p| p["title"] == "First Post")
        .unwrap();
    assert_eq!(first["meta"]["_yoast_wpseo_title"], "First Post SEO title");
    assert!(p.raw_dir("wp-db").join("redirection.ndjson").exists());
}

#[tokio::test]
async fn wpdb_container_removed_on_error() {
    if !docker_tests() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dump = tmp.path().join("broken.sql");
    std::fs::write(&dump, "CREATE TABLE ((( this is not SQL;\n").unwrap();
    let (_d, _p, r) = extract("wpdb corrupt", &dump, None).await;
    let err = format!("{:#}", r.expect_err("a corrupt dump must fail the stage"));
    assert!(err.contains("importing the dump failed"), "{err}");
    for what in ["ps", "network"] {
        let mut cmd = Command::new("docker");
        if what == "ps" {
            cmd.args(["ps", "-a", "--format", "{{.Names}}"]);
        } else {
            cmd.args(["network", "ls", "--format", "{{.Name}}"]);
        }
        let out = String::from_utf8(cmd.output().unwrap().stdout).unwrap();
        let left: Vec<&str> = out
            .lines()
            .filter(|n| n.starts_with("porter-wpdb-corrupt-"))
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }
}

#[tokio::test]
async fn wpdb_cancel_cleans_up() {
    if !docker_tests() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Project::init(dir.path(), "wpdb cancel", "wordpress").unwrap());
    let conn = Arc::new(WpDbConnector::new(&ConnectorConfig::WpDb {
        dump: fixture_dir().join("dump.sql.gz"),
        wp_root: None,
        table_prefix: "wp_".into(),
        site_url: None,
    }));
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        c2.cancel();
    });
    let s = run_stage(
        p.clone(),
        Arc::new(p.config().clone()),
        EventSink::new(4096),
        cancel,
        Stage::Extract,
        |ctx| async move { conn.extract(&ctx).await },
    )
    .await
    .unwrap();
    assert!(s.interrupted);
    let out = Command::new("docker")
        .args(["ps", "-a", "--format", "{{.Names}}"])
        .output()
        .unwrap();
    let names = String::from_utf8(out.stdout).unwrap();
    assert!(!names.contains("porter-wpdb-cancel-"), "{names}");
}

#[test]
fn export_php_is_generic() {
    assert!(!EXPORT_PHP.contains("http"), "export.php names a URL");
    assert!(
        !EXPORT_PHP.contains('@'),
        "export.php has an @ (email or suppression)"
    );
}
