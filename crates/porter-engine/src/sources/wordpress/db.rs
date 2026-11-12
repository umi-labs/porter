//! WordPress DB-dump connector: the fullest-fidelity way in. It loads the dump into a throwaway
//! MariaDB container and, given `wp_root`, runs the generic `export.php` inside WordPress
//! (`wordpress:cli`), so ACF fields come out under their names, permalinks resolve and form and
//! menu definitions are captured. Submissions and Redirection rows are paged out with SQL.
//!
//! Safety: plugins and themes must load (ACF, Gravity Forms, Ninja Forms and themes register
//! types and fields), so the site's own PHP runs, but only inside a disposable container on a
//! `docker network create --internal` network: no published ports and no route out. `wp_root`
//! and the dump are mounted read-only; only a fresh temp dir is writable. The MariaDB root
//! password is random per run and reaches containers only through the docker CLI's environment
//! (`-e NAME` with no value), so it never appears in argv, logs, events, issues or raw files.
//! Every container runs with no-new-privileges, a pids limit and 4 GB of memory; the exporter
//! drops all capabilities, MariaDB keeps only SETUID/SETGID/DAC_READ_SEARCH, and the dump is
//! imported with the client's `--sandbox`. Files read back from the exporter's output dir must
//! be regular files (never symlinks). Resources carry `porter.managed`/`porter.run` labels, and
//! a run first removes managed leftovers older than three hours.
//! Containers and the network are removed on every exit: explicitly on success, error and
//! cancel, and by a `Drop` guard if the future is dropped.
use crate::config::ConnectorConfig;
use crate::connectors::RawItem;
use crate::stage::{Capabilities, Connector, Feature, StageCtx};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, Severity, SourceRef};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const NAME: &str = "wp-db";
const DB_IMAGE: &str = "mariadb:11";
const CLI_IMAGE: &str = "wordpress:cli";
const DB_NAME: &str = "wp";
const PAGE: u64 = 1000;
const READY_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const IMPORT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const SQL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const EXPORT_PHP: &str = include_str!("export.php");
/// `wp --require`d before the site's wp-config.php, so these constants win over the site's own.
const BOOT_PHP: &str = r#"<?php
define('DB_NAME', (string) getenv('PORTER_DB_NAME'));
define('DB_USER', 'root');
define('DB_PASSWORD', (string) getenv('PORTER_DB_PASSWORD'));
define('DB_HOST', (string) getenv('PORTER_DB_HOST'));
define('WP_HTTP_BLOCK_EXTERNAL', true);
define('DISABLE_WP_CRON', true);
if (getenv('PORTER_SITE_URL')) define('WP_HOME', getenv('PORTER_SITE_URL'));
"#;
/// Used only when `wp_root` has no wp-config.php (WordPress looks one directory up).
const WP_CONFIG: &str = r#"<?php
$table_prefix = getenv('PORTER_TABLE_PREFIX') ?: 'wp_';
require_once ABSPATH . 'wp-settings.php';
"#;
/// Post types never exported as documents (same list as export.php).
/// Resources labelled `porter.managed=1` older than this are leftovers of a crashed run (no run
/// lives this long: import and export each time out at 30 minutes).
const STALE_AFTER_SECS: u64 = 3 * 60 * 60;
/// Applied to every container: untrusted dumps and site PHP run inside.
const LIMITS: [&str; 3] = [
    "--security-opt=no-new-privileges",
    "--pids-limit=512",
    "--memory=4g",
];
const SKIP_TYPES: &str = "'revision','nav_menu_item','nf_sub'";

pub struct WpDbConnector {
    dump: PathBuf,
    wp_root: Option<PathBuf>,
    prefix: String,
    site_url: Option<String>,
}

impl WpDbConnector {
    pub fn new(cfg: &ConnectorConfig) -> Self {
        match cfg {
            ConnectorConfig::WpDb {
                dump,
                wp_root,
                table_prefix,
                site_url,
            } => WpDbConnector {
                dump: dump.clone(),
                wp_root: wp_root.clone(),
                prefix: table_prefix.clone(),
                site_url: site_url
                    .as_ref()
                    .map(|u| u.trim_end_matches('/').to_string()),
            },
            _ => WpDbConnector {
                dump: PathBuf::new(),
                wp_root: None,
                prefix: "wp_".into(),
                site_url: None,
            },
        }
    }

    fn paths(&self, ctx: &StageCtx) -> Result<(PathBuf, Option<PathBuf>)> {
        let abs = |p: &Path, what: &str| {
            let p = ctx.project.dir().join(p);
            p.canonicalize()
                .with_context(|| format!("{what} {} not found", p.display()))
        };
        let dump = abs(&self.dump, "dump")?;
        let root = self
            .wp_root
            .as_deref()
            .map(|r| abs(r, "wp_root"))
            .transpose()?;
        for p in std::iter::once(&dump).chain(root.as_ref()) {
            // `--mount` is comma-separated; a comma in a path would split it.
            anyhow::ensure!(
                !p.to_string_lossy().contains(','),
                "paths with commas are not supported: {}",
                p.display()
            );
        }
        Ok((dump, root))
    }

    fn check_prefix(&self) -> Result<()> {
        let ok = !self.prefix.is_empty()
            && self
                .prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        anyhow::ensure!(ok, "table_prefix must be letters, digits and _ only");
        Ok(())
    }
}

// ---------- docker plumbing ----------

/// Runs a docker command with a timeout; `None` means cancelled (the child is killed).
async fn run(
    cmd: &mut Command,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<Option<Output>> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("Docker is required for the wp-db connector (docker not found on PATH)")
        }
        Err(e) => return Err(e.into()),
    };
    tokio::select! {
        _ = cancel.cancelled() => Ok(None),
        _ = tokio::time::sleep(timeout) => bail!("timed out after {} minutes", timeout.as_secs() / 60),
        out = child.wait_with_output() => Ok(Some(out?)),
    }
}

/// Last 50 lines of stderr with the secret scrubbed.
fn tail(out: &Output, secret: &str) -> String {
    let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
    if !secret.is_empty() {
        text = text.replace(secret, "[redacted]");
    }
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(50)..].join("\n")
}

fn docker() -> Command {
    Command::new("docker")
}

async fn require_docker(cancel: &CancellationToken) -> Result<()> {
    let out = run(
        docker().args(["version", "--format", "{{.Server.Version}}"]),
        Duration::from_secs(60),
        cancel,
    )
    .await?;
    match out {
        Some(o) if !o.status.success() => bail!(
            "Docker is required for the wp-db connector (daemon unavailable): {}",
            tail(&o, "")
        ),
        _ => Ok(()),
    }
}

fn sanitise(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s: String = s.trim_end_matches('-').chars().take(30).collect();
    let s = s.trim_end_matches('-');
    if s.is_empty() {
        "project".into()
    } else {
        s.into()
    }
}

/// The run's containers and network. Dropping it without `cleanup` removes them synchronously.
struct Sandbox {
    /// Name of both the network and the database container.
    db: String,
    /// `--label` args for every container and the network.
    labels: Vec<String>,
    cli: String,
    password: String,
    clean: bool,
}

impl Sandbox {
    fn new(project: &str) -> Sandbox {
        let short = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let base = format!("porter-{}-{short}", sanitise(project));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let labels = [
            "porter.managed=1".to_string(),
            format!("porter.run={base}"),
            format!("porter.started={now}"),
        ]
        .into_iter()
        .flat_map(|l| ["--label".to_string(), l])
        .collect();
        Sandbox {
            labels,
            db: format!("{base}-db"),
            cli: format!("{base}-wp"),
            password: uuid::Uuid::new_v4().simple().to_string(),
            clean: false,
        }
    }

    fn removals(&self) -> [Vec<String>; 2] {
        [
            vec![
                "rm".into(),
                "-f".into(),
                "-v".into(),
                self.cli.clone(),
                self.db.clone(),
            ],
            vec!["network".into(), "rm".into(), self.db.clone()],
        ]
    }

    /// Removes containers and networks of earlier runs that never cleaned up (crash, kill -9).
    async fn sweep_stale(cancel: &CancellationToken) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let fmt = "{{.Names}}\t{{.Label \"porter.started\"}}";
        let nfmt = "{{.Name}}\t{{.Label \"porter.started\"}}";
        let filter = "label=porter.managed=1";
        let list = [
            (
                vec!["ps", "-a", "--filter", filter, "--format", fmt],
                vec!["rm", "-f", "-v"],
            ),
            (
                vec!["network", "ls", "--filter", filter, "--format", nfmt],
                vec!["network", "rm"],
            ),
        ];
        for (ls, rm) in list {
            let Some(out) = run(docker().args(&ls), Duration::from_secs(60), cancel).await? else {
                return Ok(());
            };
            let stale = stale_names(&String::from_utf8_lossy(&out.stdout), now);
            if !stale.is_empty() {
                run(
                    docker().args(&rm).args(&stale),
                    Duration::from_secs(120),
                    cancel,
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn cleanup(&mut self) {
        for args in self.removals() {
            let _ = docker()
                .args(&args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
        self.clean = true;
    }

    /// A `docker exec` in the db container with the root password in `MYSQL_PWD`. `env` is
    /// passed by name, so its value stays out of argv.
    fn exec(&self, script: &str, env: Option<(&str, &str)>) -> Command {
        let mut c = docker();
        c.arg("exec");
        if let Some((k, v)) = env {
            c.args(["-e", k]).env(k, v);
        }
        c.args([&self.db, "bash", "-o", "pipefail", "-c"])
            .arg(format!(
                "export MYSQL_PWD=\"$MARIADB_ROOT_PASSWORD\"; {script}"
            ));
        c
    }

    async fn start(&self, dump: &Path, gz: bool, cancel: &CancellationToken) -> Result<bool> {
        let out = run(
            docker()
                .args(["network", "create", "--internal"])
                .args(&self.labels)
                .arg(&self.db),
            Duration::from_secs(60),
            cancel,
        )
        .await?;
        let Some(out) = out else { return Ok(false) };
        anyhow::ensure!(
            out.status.success(),
            "docker network create failed: {}",
            tail(&out, &self.password)
        );
        let target = if gz {
            "/dump/dump.sql.gz"
        } else {
            "/dump/dump.sql"
        };
        let mut cmd = docker();
        // The entrypoint switches to the mysql user (SETUID/SETGID); `docker exec` reads the
        // mounted dump as root, which may need DAC_READ_SEARCH for a 0600 file.
        cmd.args(["run", "-d", "--name", &self.db, "--network", &self.db])
            .args(&self.labels)
            .args(LIMITS)
            .args([
                "--cap-drop=ALL",
                "--cap-add=SETUID",
                "--cap-add=SETGID",
                "--cap-add=DAC_READ_SEARCH",
            ])
            .args(["-e", "MARIADB_ROOT_PASSWORD", "-e"])
            .arg(format!("MARIADB_DATABASE={DB_NAME}"))
            .arg("--mount")
            .arg(format!(
                "type=bind,src={},dst={target},readonly",
                dump.display()
            ))
            .args([DB_IMAGE, "--max-allowed-packet=1G"])
            .env("MARIADB_ROOT_PASSWORD", &self.password);
        let Some(out) = run(&mut cmd, Duration::from_secs(10 * 60), cancel).await? else {
            return Ok(false);
        };
        anyhow::ensure!(
            out.status.success(),
            "starting {DB_IMAGE} failed: {}",
            tail(&out, &self.password)
        );
        // The image's first start runs a socket-only init server; TCP answers once it is done.
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            let probe = "mariadb -h127.0.0.1 -uroot -e 'select 1'";
            let Some(out) =
                run(&mut self.exec(probe, None), Duration::from_secs(30), cancel).await?
            else {
                return Ok(false);
            };
            if out.status.success() {
                return Ok(true);
            }
            if String::from_utf8_lossy(&out.stderr).contains("is not running") {
                bail!("database container stopped: {}", tail(&out, &self.password));
            }
            if tokio::time::Instant::now() > deadline {
                bail!(
                    "database did not become ready: {}",
                    tail(&out, &self.password)
                );
            }
            tokio::select! {
                _ = cancel.cancelled() => return Ok(false),
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }

    async fn import(&self, gz: bool, cancel: &CancellationToken) -> Result<bool> {
        let load = format!(
            "mariadb -uroot --sandbox --max-allowed-packet=1G --default-character-set=utf8mb4 {DB_NAME}"
        );
        let script = if gz {
            format!("gzip -dc /dump/dump.sql.gz | {load}")
        } else {
            format!("{load} < /dump/dump.sql")
        };
        let Some(out) = run(&mut self.exec(&script, None), IMPORT_TIMEOUT, cancel)
            .await
            .context("importing the dump")?
        else {
            return Ok(false);
        };
        anyhow::ensure!(
            out.status.success(),
            "importing the dump failed:\n{}",
            tail(&out, &self.password)
        );
        Ok(true)
    }

    /// One JSON value per output row; `sql` must select a single JSON column.
    async fn rows(&self, sql: &str, cancel: &CancellationToken) -> Result<Option<Vec<Value>>> {
        let mut c = self.exec(
            &format!(
                "mariadb -uroot -N -B -r --default-character-set=utf8mb4 {DB_NAME} -e \"$PORTER_Q\""
            ),
            Some(("PORTER_Q", sql)),
        );
        let Some(out) = run(&mut c, SQL_TIMEOUT, cancel).await? else {
            return Ok(None);
        };
        anyhow::ensure!(
            out.status.success(),
            "query failed:\n{}",
            tail(&out, &self.password)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        let mut v = vec![];
        for line in text.lines().filter(|l| !l.is_empty() && *l != "NULL") {
            v.push(
                serde_json::from_str(line)
                    .with_context(|| format!("bad JSON row from MariaDB: {line:.200}"))?,
            );
        }
        Ok(Some(v))
    }

    async fn one(&self, sql: &str, cancel: &CancellationToken) -> Result<Option<Value>> {
        Ok(self
            .rows(sql, cancel)
            .await?
            .map(|mut r| r.pop().unwrap_or(Value::Null)))
    }

    async fn has_table(&self, table: &str, cancel: &CancellationToken) -> Result<Option<bool>> {
        let sql = format!(
            "SELECT JSON_OBJECT('n', COUNT(*)) FROM information_schema.tables \
             WHERE table_schema = '{DB_NAME}' AND table_name = '{table}'"
        );
        Ok(self
            .one(&sql, cancel)
            .await?
            .map(|v| v["n"].as_u64().unwrap_or(0) > 0))
    }

    /// Pages `select ... from ...` (one JSON column) by `order`, 1000 rows a time.
    /// Returns false if cancelled.
    async fn paged(
        &self,
        select: &str,
        order: &str,
        cancel: &CancellationToken,
        mut each: impl FnMut(Vec<Value>) -> Result<()>,
    ) -> Result<bool> {
        let mut offset = 0;
        loop {
            let sql = format!("{select} ORDER BY {order} LIMIT {PAGE} OFFSET {offset}");
            let Some(rows) = self.rows(&sql, cancel).await? else {
                return Ok(false);
            };
            let n = rows.len() as u64;
            each(rows)?;
            if n < PAGE {
                return Ok(true);
            }
            offset += PAGE;
        }
    }

    /// Runs export.php in wordpress:cli; returns the path of the written JSON.
    async fn export(
        &self,
        root: &Path,
        prefix: &str,
        site_url: Option<&str>,
        work: &Path,
        cancel: &CancellationToken,
    ) -> Result<Option<PathBuf>> {
        let php = work.join("php");
        let out = work.join("out");
        std::fs::create_dir_all(&php)?;
        std::fs::create_dir_all(&out)?;
        std::fs::write(php.join("export.php"), EXPORT_PHP)?;
        std::fs::write(php.join("boot.php"), BOOT_PHP)?;
        std::fs::write(work.join("wp-config.php"), WP_CONFIG)?;
        // The container's user (www-data) is not ours.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o777))?;
        }
        let bind = |src: &Path, dst: &str, ro: bool| {
            format!(
                "type=bind,src={},dst={dst}{}",
                src.display(),
                if ro { ",readonly" } else { "" }
            )
        };
        let mut cmd = docker();
        cmd.args(["run", "--rm", "--name", &self.cli, "--network", &self.db])
            .args(&self.labels)
            .args(LIMITS)
            .arg("--cap-drop=ALL")
            .args(["-e", "PORTER_DB_PASSWORD", "-e", "HOME=/tmp"])
            .arg("-e")
            .arg(format!("PORTER_DB_HOST={}", self.db))
            .arg("-e")
            .arg(format!("PORTER_DB_NAME={DB_NAME}"))
            .arg("-e")
            .arg(format!("PORTER_TABLE_PREFIX={prefix}"))
            .args(["-e", "PORTER_OUT=/out/export.json"])
            .arg("--mount")
            .arg(bind(root, "/var/www/html", true))
            .arg("--mount")
            .arg(bind(&php, "/porter", true))
            .arg("--mount")
            .arg(bind(
                &work.join("wp-config.php"),
                "/var/www/wp-config.php",
                true,
            ))
            .arg("--mount")
            .arg(bind(&out, "/out", false))
            .env("PORTER_DB_PASSWORD", &self.password);
        if let Some(u) = site_url {
            cmd.arg("-e").arg(format!("PORTER_SITE_URL={u}"));
        }
        cmd.args([
            CLI_IMAGE,
            "php",
            "-d",
            "memory_limit=2G",
            "/usr/local/bin/wp",
        ])
        .args(["eval-file", "/porter/export.php", "--path=/var/www/html"])
        .arg("--require=/porter/boot.php");
        let Some(o) = run(&mut cmd, EXPORT_TIMEOUT, cancel)
            .await
            .context("running export.php")?
        else {
            return Ok(None);
        };
        anyhow::ensure!(
            o.status.success(),
            "export.php failed (exit {}):\n{}",
            o.status.code().unwrap_or(-1),
            tail(&o, &self.password)
        );
        let file = out.join("export.json");
        anyhow::ensure!(
            file.symlink_metadata().is_ok(),
            "export.php wrote no export.json:\n{}",
            tail(&o, &self.password)
        );
        regular_file(&file)?;
        Ok(Some(file))
    }
}

/// Files the exporter leaves in `/out` are untrusted: a symlink there could point at any host
/// file. Only a regular file (checked without following links) is read back.
fn regular_file(p: &Path) -> Result<()> {
    let ok = std::fs::symlink_metadata(p)
        .with_context(|| format!("reading {}", p.display()))?
        .file_type()
        .is_file();
    anyhow::ensure!(ok, "exporter produced a non-regular file: {}", p.display());
    Ok(())
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.clean {
            return;
        }
        for args in self.removals() {
            let _ = std::process::Command::new("docker")
                .args(&args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Names from `name\tstarted` lines whose start is over `STALE_AFTER_SECS` ago.
fn stale_names(listing: &str, now: u64) -> Vec<String> {
    listing
        .lines()
        .filter_map(|l| {
            let (name, started) = l.split_once('\t')?;
            let started: u64 = started.trim().parse().ok()?;
            (now.saturating_sub(started) > STALE_AFTER_SECS).then(|| name.to_string())
        })
        .collect()
}

// ---------- records ----------

fn s(v: &Value, k: &str) -> Option<String> {
    match &v[k] {
        Value::String(x) => Some(x.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Writes the RawItem row and status for one item.
fn record(
    ctx: &StageCtx,
    kind: ItemKind,
    source_id: String,
    row: RawItem,
    status: ItemStatus,
) -> Result<()> {
    let src = SourceRef {
        connector: NAME.into(),
        source_id,
        url: row.url.clone(),
    };
    let id = ItemId::new(kind, &src);
    ctx.project.store().put_item(kind, id, &src, &row)?;
    ctx.item_status(id, kind, status)
}

/// Records a list of objects. `sid` builds the source id from the object's id; an object with
/// no id fails (never dropped).
fn record_list(
    ctx: &StageCtx,
    kind: ItemKind,
    list: &Value,
    raw: &str,
    label: impl Fn(&Value) -> String,
    sid: impl Fn(&Value, &str) -> String,
) -> Result<()> {
    for (n, v) in list.as_array().into_iter().flatten().enumerate() {
        let (id, status) = match s(v, "id") {
            Some(id) => (id, ItemStatus::Done),
            None => (
                format!("unknown-{}", n + 1),
                ItemStatus::Failed {
                    error: "record has no id".into(),
                },
            ),
        };
        let row = RawItem {
            label: label(v),
            title: s(v, "title")
                .or_else(|| s(v, "name"))
                .or_else(|| s(v, "display_name")),
            status: s(v, "status"),
            url: s(v, "link"),
            raw_path: Some(raw.into()),
        };
        record(ctx, kind, sid(v, &id), row, status)?;
    }
    Ok(())
}

fn record_export(ctx: &StageCtx, v: &Value) -> Result<()> {
    let raw = "export.json";
    record_list(
        ctx,
        ItemKind::Document,
        &v["posts"],
        raw,
        |p| s(p, "type").unwrap_or_default(),
        |p, id| {
            let kind = if p["type"] == "attachment" {
                "attachment"
            } else {
                "post"
            };
            format!("{kind}:{id}")
        },
    )?;
    record_list(
        ctx,
        ItemKind::Term,
        &v["terms"],
        raw,
        |t| s(t, "taxonomy").unwrap_or_default(),
        |t, id| format!("term:{}:{id}", s(t, "taxonomy").unwrap_or_default()),
    )?;
    record_list(
        ctx,
        ItemKind::Author,
        &v["users"],
        raw,
        |_| "user".into(),
        |_, id| format!("user:{id}"),
    )?;
    record_list(
        ctx,
        ItemKind::Menu,
        &v["menus"],
        raw,
        |_| "menu".into(),
        |_, id| format!("menu:{id}"),
    )?;
    for f in ["gf", "nf"] {
        record_list(
            ctx,
            ItemKind::Form,
            &v["forms"][f],
            raw,
            |_| format!("form-{f}"),
            |_, id| format!("form:{f}:{id}"),
        )?;
    }
    let row = RawItem {
        label: "settings".into(),
        title: s(&v["site"], "blogname"),
        status: None,
        url: s(&v["site"], "home"),
        raw_path: Some(raw.into()),
    };
    record(
        ctx,
        ItemKind::Settings,
        "settings".into(),
        row,
        ItemStatus::Done,
    )
}

fn gap(ctx: &StageCtx, msg: &str) -> Result<()> {
    ctx.issue(
        IssueKind::AccessGap,
        Severity::Warning,
        None,
        msg,
        json!({ "connector": NAME, "hint": "set wp_root to run WordPress itself" }),
    )
}

// ---------- SQL fallback ----------

/// Rebuilds a post's link from `permalink_structure` the way WordPress's defaults would.
/// `pages` maps hierarchical ids to `(slug, parent)`; `category` is the post's first category.
pub fn permalink(
    home: &str,
    structure: &str,
    p: &Value,
    pages: &HashMap<u64, (String, u64)>,
    category: Option<&str>,
    author: Option<&str>,
) -> Option<String> {
    let id = p["id"].as_u64()?;
    let ty = p["type"].as_str().unwrap_or("post");
    let slug = p["slug"].as_str().unwrap_or("");
    if ty == "attachment" {
        let file = p["meta"]["_wp_attached_file"].as_str()?;
        return Some(format!("{home}/wp-content/uploads/{file}"));
    }
    if structure.is_empty() || slug.is_empty() {
        return Some(match ty {
            "post" => format!("{home}/?p={id}"),
            "page" => format!("{home}/?page_id={id}"),
            t => format!("{home}/?post_type={t}&p={id}"),
        });
    }
    if pages.contains_key(&id) {
        let mut parts = vec![];
        let mut cur = Some(id);
        while let Some(c) = cur.filter(|_| parts.len() < 50) {
            let Some((sl, parent)) = pages.get(&c) else {
                break;
            };
            parts.push(sl.as_str());
            cur = (*parent != 0).then_some(*parent);
        }
        parts.reverse();
        let prefix = if ty == "page" {
            String::new()
        } else {
            format!("{ty}/")
        };
        return Some(format!("{home}/{prefix}{}/", parts.join("/")));
    }
    if ty != "post" {
        return Some(format!("{home}/{ty}/{slug}/"));
    }
    let date = p["date"].as_str().unwrap_or("");
    let part = |r: std::ops::Range<usize>| date.get(r).unwrap_or("").to_string();
    let path = structure
        .replace("%postname%", slug)
        .replace("%post_id%", &id.to_string())
        .replace("%year%", &part(0..4))
        .replace("%monthnum%", &part(5..7))
        .replace("%day%", &part(8..10))
        .replace("%hour%", &part(11..13))
        .replace("%minute%", &part(14..16))
        .replace("%second%", &part(17..19))
        .replace("%category%", category.unwrap_or("uncategorized"))
        .replace("%author%", author.unwrap_or(""));
    Some(format!("{home}{path}"))
}

impl WpDbConnector {
    /// The same shape export.php writes, from tables alone. Sets `objects` if a serialized
    /// PHP object was left undecoded.
    async fn sql_export(
        &self,
        sb: &Sandbox,
        cancel: &CancellationToken,
        objects: &mut bool,
    ) -> Result<Option<Value>> {
        let p = &self.prefix;
        let Some(opts) = sb
            .one(
                &format!(
                    "SELECT JSON_ARRAYAGG(JSON_ARRAY(option_name, option_value)) FROM {p}options WHERE option_name IN \
                     ('home','siteurl','blogname','blogdescription','show_on_front','page_on_front',\
                     'page_for_posts','permalink_structure','wpseo_titles','active_plugins')"
                ),
                cancel,
            )
            .await?
        else {
            return Ok(None);
        };
        let opts = fold_meta(&opts, objects);
        let opt = |k: &str| opts.get(k).cloned().unwrap_or(Value::Null);
        let id_or_null = |v: Value| match v.as_str().and_then(|x| x.parse::<u64>().ok()) {
            Some(n) if n > 0 => json!(n),
            _ => Value::Null,
        };
        let site = json!({
            "home": opt("home"),
            "siteurl": opt("siteurl"),
            "blogname": opt("blogname"),
            "blogdescription": opt("blogdescription"),
            "show_on_front": opt("show_on_front"),
            "page_on_front": id_or_null(opt("page_on_front")),
            "page_for_posts": id_or_null(opt("page_for_posts")),
            "permalink_structure": opt("permalink_structure"),
            "wpseo_titles": match opt("wpseo_titles") { Value::String(x) if x.is_empty() => Value::Null, v => v },
            "active_plugins": match opt("active_plugins") {
                Value::Object(o) => Value::Array(o.into_iter().map(|(_, v)| v).collect()),
                v @ Value::Array(_) => v,
                _ => json!([]),
            },
            "acf_active": null,
            "acf_options": null,
        });

        let mut posts = vec![];
        let select = format!(
            "SELECT JSON_OBJECT('id', p.ID, 'type', p.post_type, 'status', p.post_status, \
             'slug', p.post_name, 'title', p.post_title, 'parent', NULLIF(p.post_parent, 0), \
             'menu_order', p.menu_order, 'author', NULLIF(p.post_author, 0), 'date', p.post_date, \
             'date_gmt', p.post_date_gmt, 'modified_gmt', p.post_modified_gmt, \
             'mime', NULLIF(p.post_mime_type, ''), 'content', p.post_content, 'excerpt', p.post_excerpt, \
             'meta', (SELECT JSON_ARRAYAGG(JSON_ARRAY(m.meta_key, m.meta_value) ORDER BY m.meta_id) \
                      FROM {p}postmeta m WHERE m.post_id = p.ID AND m.meta_key IS NOT NULL), \
             'term_ids', (SELECT JSON_ARRAYAGG(tt.term_id) FROM {p}term_relationships tr \
                          JOIN {p}term_taxonomy tt ON tt.term_taxonomy_id = tr.term_taxonomy_id \
                          WHERE tr.object_id = p.ID)) \
             FROM {p}posts p WHERE p.post_status <> 'auto-draft' AND p.post_type NOT IN ({SKIP_TYPES})"
        );
        if !sb
            .paged(&select, "p.ID", cancel, |rows| {
                posts.extend(rows);
                Ok(())
            })
            .await?
        {
            return Ok(None);
        }
        let mut terms = vec![];
        let select = format!(
            "SELECT JSON_OBJECT('id', t.term_id, 'taxonomy', tt.taxonomy, 'slug', t.slug, 'name', t.name, \
             'description', tt.description, 'parent', NULLIF(tt.parent, 0), 'count', tt.count, \
             'meta', (SELECT JSON_ARRAYAGG(JSON_ARRAY(m.meta_key, m.meta_value) ORDER BY m.meta_id) \
                      FROM {p}termmeta m WHERE m.term_id = t.term_id AND m.meta_key IS NOT NULL)) \
             FROM {p}terms t JOIN {p}term_taxonomy tt ON tt.term_id = t.term_id WHERE tt.taxonomy <> 'nav_menu'"
        );
        if !sb
            .paged(&select, "t.term_id", cancel, |rows| {
                terms.extend(rows);
                Ok(())
            })
            .await?
        {
            return Ok(None);
        }
        let mut users = vec![];
        let select = format!(
            "SELECT JSON_OBJECT('id', u.ID, 'login', u.user_login, 'nicename', u.user_nicename, \
             'display_name', u.display_name, 'email', u.user_email, \
             'caps', (SELECT meta_value FROM {p}usermeta WHERE user_id = u.ID AND meta_key = '{p}capabilities' LIMIT 1)) \
             FROM {p}users u"
        );
        if !sb
            .paged(&select, "u.ID", cancel, |rows| {
                users.extend(rows);
                Ok(())
            })
            .await?
        {
            return Ok(None);
        }

        let home = self
            .site_url
            .clone()
            .or_else(|| s(&site, "home"))
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let structure = s(&site, "permalink_structure").unwrap_or_default();
        let term_slug: HashMap<u64, (String, String)> = terms
            .iter()
            .filter_map(|t| Some((t["id"].as_u64()?, (s(t, "taxonomy")?, s(t, "slug")?))))
            .collect();
        let nicename: HashMap<u64, String> = users
            .iter()
            .filter_map(|u| Some((u["id"].as_u64()?, s(u, "nicename")?)))
            .collect();
        // Hierarchical types: pages, plus any type with a post whose parent is set.
        let mut hier: Vec<String> = vec!["page".into()];
        for p in &posts {
            if p["parent"].as_u64().is_some() && p["type"] != "attachment" {
                hier.extend(s(p, "type"));
            }
        }
        let pages: HashMap<u64, (String, u64)> = posts
            .iter()
            .filter(|p| {
                p["type"]
                    .as_str()
                    .is_some_and(|t| hier.iter().any(|h| h == t))
            })
            .filter_map(|p| {
                Some((
                    p["id"].as_u64()?,
                    (s(p, "slug")?, p["parent"].as_u64().unwrap_or(0)),
                ))
            })
            .collect();
        let mut types: Map<String, Value> = Map::new();
        for p in &mut posts {
            let Some(o) = p.as_object_mut() else { continue };
            let meta = fold_meta(&o.remove("meta").unwrap_or(Value::Null), objects);
            // {taxonomy: [term ids]}, as export.php writes it.
            let mut by_tax: Map<String, Value> = Map::new();
            let mut category = None;
            for t in o
                .remove("term_ids")
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
            {
                let Some((tax, slug)) = t.as_u64().and_then(|t| term_slug.get(&t)) else {
                    continue;
                };
                if tax == "category" && category.is_none() {
                    category = Some(slug.clone());
                }
                if let Value::Array(a) = by_tax.entry(tax.clone()).or_insert_with(|| json!([])) {
                    a.push(t.clone());
                }
            }
            let ty = o
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            types.insert(ty.clone(), Value::Null);
            o.insert(
                "template".into(),
                match meta.get("_wp_page_template") {
                    Some(Value::String(t)) if !t.is_empty() && t != "default" => json!(t),
                    _ => Value::Null,
                },
            );
            o.insert(
                "featured_image".into(),
                meta.get("_thumbnail_id")
                    .and_then(Value::as_str)
                    .and_then(|x| x.parse::<u64>().ok())
                    .filter(|n| *n > 0)
                    .map_or(Value::Null, |n| json!(n)),
            );
            if ty == "attachment" {
                o.insert("attachment".into(), json!({
                    "file": meta.get("_wp_attached_file").cloned().unwrap_or(Value::Null),
                    "metadata": meta.get("_wp_attachment_metadata").cloned().unwrap_or(Value::Null),
                }));
            }
            o.insert("meta".into(), Value::Object(meta));
            o.insert("acf".into(), Value::Null);
            o.insert("terms".into(), Value::Object(by_tax));
            let author = o
                .get("author")
                .and_then(Value::as_u64)
                .and_then(|a| nicename.get(&a))
                .cloned();
            let link = permalink(
                &home,
                &structure,
                p,
                &pages,
                category.as_deref(),
                author.as_deref(),
            );
            if let Some(o) = p.as_object_mut() {
                o.insert("link".into(), link.map_or(Value::Null, Value::String));
            }
        }
        let mut taxonomies: Map<String, Value> = Map::new();
        for t in &mut terms {
            let Some(o) = t.as_object_mut() else { continue };
            if let Some(tax) = o.get("taxonomy").and_then(Value::as_str) {
                taxonomies.insert(tax.to_string(), Value::Null);
            }
            let meta = fold_meta(&o.remove("meta").unwrap_or(Value::Null), objects);
            o.insert("meta".into(), Value::Object(meta));
            o.insert("link".into(), Value::Null);
            o.insert("acf".into(), Value::Null);
        }
        for u in &mut users {
            let Some(o) = u.as_object_mut() else { continue };
            let caps = match o.remove("caps").and_then(|c| c.as_str().map(php_decode)) {
                Some(Php::Value(Value::Object(m))) => m
                    .into_iter()
                    .filter(|(_, v)| v != &json!(false))
                    .map(|(k, _)| json!(k))
                    .collect(),
                Some(Php::Object) => {
                    *objects = true;
                    vec![]
                }
                _ => vec![],
            };
            o.insert("roles".into(), Value::Array(caps));
        }
        // Registration details only exist inside WordPress: null, like export.php's
        // entry for a type whose plugin is gone.
        Ok(Some(json!({
            "site": site,
            "post_types": types,
            "posts": posts,
            "taxonomies": taxonomies,
            "terms": terms,
            "users": users,
            "menus": [],
            "menu_locations": {},
            "acf_field_groups": [],
            "forms": { "gf": [], "nf": [] },
            "translations": null,
        })))
    }

    /// Pages `select` into `raw/wp-db/<file>`, one JSON row per line, recording each row.
    #[allow(clippy::too_many_arguments)]
    async fn ndjson(
        &self,
        ctx: &StageCtx,
        sb: &Sandbox,
        out: &mut BufWriter<std::fs::File>,
        file: &str,
        select: &str,
        order: &str,
        kind: ItemKind,
        sid: impl Fn(&Value, &str) -> String,
        objects: &mut bool,
    ) -> Result<bool> {
        let mut seen = 0u64;
        sb.paged(select, order, &ctx.cancel, |rows| {
            for mut v in rows {
                // Submission answers arrive as [key, value] pairs; fold like post meta.
                if let Some(f) = v.get_mut("fields").filter(|f| f.is_array()) {
                    *f = Value::Object(fold_meta(f, objects));
                }
                serde_json::to_writer(&mut *out, &v)?;
                out.write_all(b"\n")?;
                seen += 1;
                let (id, status) = match s(&v, "id") {
                    Some(id) => (id, ItemStatus::Done),
                    None => (
                        format!("unknown-{seen}"),
                        ItemStatus::Failed {
                            error: "row has no id".into(),
                        },
                    ),
                };
                let row = RawItem {
                    label: kind.as_str().into(),
                    title: s(&v, "url").or_else(|| s(&v, "title")),
                    status: s(&v, "status"),
                    url: None,
                    raw_path: Some(file.into()),
                };
                record(ctx, kind, sid(&v, &id), row, status)?;
            }
            ctx.progress(seen, None, file);
            Ok(())
        })
        .await
    }

    async fn sql_extras(&self, ctx: &StageCtx, sb: &Sandbox, objects: &mut bool) -> Result<bool> {
        let p = &self.prefix;
        let dir = ctx.project.raw_dir(NAME);
        let open = |f: &str| -> Result<BufWriter<std::fs::File>> {
            Ok(BufWriter::new(std::fs::File::create(dir.join(f))?))
        };
        let cancel = &ctx.cancel;

        let mut out = open("redirection.ndjson")?;
        let table = format!("{p}redirection_items");
        let Some(has) = sb.has_table(&table, cancel).await? else {
            return Ok(false);
        };
        if has {
            let select = format!(
                "SELECT JSON_OBJECT('id', id, 'url', url, 'match_url', match_url, 'match_type', match_type, \
                 'match_data', match_data, 'regex', regex, 'action_type', action_type, \
                 'action_code', action_code, 'action_data', action_data, 'status', status, \
                 'group_id', group_id, 'position', position, 'title', title, 'last_count', last_count) \
                 FROM {table}"
            );
            let sid = |_: &Value, id: &str| format!("redirect:{id}");
            if !self
                .ndjson(
                    ctx,
                    sb,
                    &mut out,
                    "redirection.ndjson",
                    &select,
                    "id",
                    ItemKind::Redirect,
                    sid,
                    objects,
                )
                .await?
            {
                return Ok(false);
            }
        }
        out.flush()?;

        let mut out = open("submissions.ndjson")?;
        let Some(has_gf) = sb.has_table(&format!("{p}gf_entry"), cancel).await? else {
            return Ok(false);
        };
        let Some(has_gf_meta) = sb.has_table(&format!("{p}gf_entry_meta"), cancel).await? else {
            return Ok(false);
        };
        if has_gf && has_gf_meta {
            let select = format!(
                "SELECT JSON_OBJECT('source', 'gf', 'id', e.id, 'form_id', e.form_id, \
                 'date', e.date_created, 'status', e.status, 'source_url', e.source_url, \
                 'fields', (SELECT JSON_ARRAYAGG(JSON_ARRAY(m.meta_key, m.meta_value) ORDER BY m.id) \
                            FROM {p}gf_entry_meta m WHERE m.entry_id = e.id AND m.meta_key IS NOT NULL)) \
                 FROM {p}gf_entry e"
            );
            let sid = |_: &Value, id: &str| format!("submission:gf:{id}");
            if !self
                .ndjson(
                    ctx,
                    sb,
                    &mut out,
                    "submissions.ndjson",
                    &select,
                    "e.id",
                    ItemKind::Submission,
                    sid,
                    objects,
                )
                .await?
            {
                return Ok(false);
            }
        }
        // Ninja Forms keeps submissions as `nf_sub` posts with `_form_id` and `_field_<n>` meta.
        let select = format!(
            "SELECT JSON_OBJECT('source', 'nf', 'id', p.ID, 'date', p.post_date_gmt, 'status', p.post_status, \
             'form_id', (SELECT meta_value FROM {p}postmeta WHERE post_id = p.ID AND meta_key = '_form_id' LIMIT 1), \
             'fields', (SELECT JSON_ARRAYAGG(JSON_ARRAY(m.meta_key, m.meta_value) ORDER BY m.meta_id) \
                        FROM {p}postmeta m WHERE m.post_id = p.ID AND m.meta_key LIKE '\\\\_field\\\\_%')) \
             FROM {p}posts p WHERE p.post_type = 'nf_sub'"
        );
        let sid = |_: &Value, id: &str| format!("submission:nf:{id}");
        if !self
            .ndjson(
                ctx,
                sb,
                &mut out,
                "submissions.ndjson",
                &select,
                "p.ID",
                ItemKind::Submission,
                sid,
                objects,
            )
            .await?
        {
            return Ok(false);
        }
        out.flush()?;
        Ok(true)
    }

    /// Everything between start and cleanup. `Ok` also on cancel (resumable, nothing half-recorded).
    async fn extract_in(&self, ctx: &StageCtx, sb: &Sandbox) -> Result<()> {
        let (dump, root) = self.paths(ctx)?;
        let gz = is_gzip(&dump)?;
        let cancel = &ctx.cancel;
        let dir = ctx.project.raw_dir(NAME);
        std::fs::create_dir_all(&dir)?;
        ctx.progress(0, None, "starting database");
        if !sb.start(&dump, gz, cancel).await? {
            return Ok(());
        }
        ctx.progress(0, None, "importing dump");
        if !sb.import(gz, cancel).await? {
            return Ok(());
        }
        let mut objects = false;
        let export = match &root {
            Some(root) => {
                ctx.progress(0, None, "running export.php");
                let work = tempfile::tempdir()?;
                let Some(file) = sb
                    .export(
                        root,
                        &self.prefix,
                        self.site_url.as_deref(),
                        work.path(),
                        cancel,
                    )
                    .await?
                else {
                    return Ok(());
                };
                regular_file(&file)?;
                std::fs::copy(&file, dir.join("export.json"))?;
                let f = std::fs::File::open(dir.join("export.json"))?;
                serde_json::from_reader(std::io::BufReader::new(f))
                    .context("export.json is not valid JSON")?
            }
            None => {
                ctx.progress(0, None, "reading tables");
                let Some(v) = self.sql_export(sb, cancel, &mut objects).await? else {
                    return Ok(());
                };
                std::fs::write(dir.join("export.json"), serde_json::to_vec(&v)?)?;
                for msg in [
                    "ACF field names unavailable (keys only)",
                    "permalinks reconstructed from permalink_structure",
                    "form definitions unavailable",
                ] {
                    gap(ctx, msg)?;
                }
                v
            }
        };
        record_export(ctx, &export)?;
        drop(export);
        let finished = self.sql_extras(ctx, sb, &mut objects).await?;
        if objects && finished {
            ctx.issue(
                IssueKind::AccessGap,
                Severity::Info,
                None,
                "serialized PHP objects left undecoded",
                json!({ "connector": NAME, "hint": "values starting O: or C: are kept as raw strings" }),
            )?;
        }
        Ok(())
    }
}

/// A PHP `serialize()` string decoded the way `maybe_unserialize` would.
#[derive(Debug, PartialEq)]
pub enum Php {
    Value(Value),
    /// Holds an object (`O:`, `C:`) or reference; kept as the raw string.
    Object,
    /// Not serialized data.
    Plain,
}

/// Decodes `s`, `i`, `d`, `b`, `N` and `a` (lists become arrays, other arrays objects).
pub fn php_decode(raw: &str) -> Php {
    let b = raw.trim_end().as_bytes();
    let mut pos = 0;
    match php_value(b, &mut pos) {
        Ok(v) if pos == b.len() => Php::Value(v),
        Err(true) => Php::Object,
        _ => Php::Plain,
    }
}

/// `Err(true)`: an object or reference was found; `Err(false)`: not serialized.
fn php_value(b: &[u8], pos: &mut usize) -> std::result::Result<Value, bool> {
    let tag = *b.get(*pos).ok_or(false)?;
    if matches!(tag, b'O' | b'C' | b'r' | b'R' | b'E') && b.get(*pos + 1) == Some(&b':') {
        return Err(true);
    }
    if tag == b'N' {
        expect(b, pos, b"N;")?;
        return Ok(Value::Null);
    }
    expect(b, pos, &[tag, b':'])?;
    match tag {
        b'b' => match until(b, pos, b';')? {
            "0" => Ok(json!(false)),
            "1" => Ok(json!(true)),
            _ => Err(false),
        },
        b'i' => until(b, pos, b';')?
            .parse::<i64>()
            .map(|n| json!(n))
            .map_err(|_| false),
        b'd' => {
            let t = until(b, pos, b';')?;
            let f: f64 = t.parse().map_err(|_| false)?;
            Ok(serde_json::Number::from_f64(f).map_or_else(|| json!(t), Value::Number))
        }
        b's' => {
            let len: usize = until(b, pos, b':')?.parse().map_err(|_| false)?;
            expect(b, pos, b"\"")?;
            let bytes = b.get(*pos..*pos + len).ok_or(false)?;
            *pos += len;
            expect(b, pos, b"\";")?;
            Ok(Value::String(String::from_utf8_lossy(bytes).into_owned()))
        }
        b'a' => {
            let n: usize = until(b, pos, b':')?.parse().map_err(|_| false)?;
            expect(b, pos, b"{")?;
            let mut pairs = Vec::with_capacity(n.min(1024));
            for _ in 0..n {
                let k = match php_value(b, pos)? {
                    Value::String(s) => s,
                    Value::Number(n) => n.to_string(),
                    _ => return Err(false),
                };
                pairs.push((k, php_value(b, pos)?));
            }
            expect(b, pos, b"}")?;
            let is_list = pairs
                .iter()
                .enumerate()
                .all(|(i, (k, _))| *k == i.to_string());
            Ok(if is_list {
                Value::Array(pairs.into_iter().map(|(_, v)| v).collect())
            } else {
                Value::Object(pairs.into_iter().collect())
            })
        }
        _ => Err(false),
    }
}

fn expect(b: &[u8], pos: &mut usize, lit: &[u8]) -> std::result::Result<(), bool> {
    if b.get(*pos..*pos + lit.len()) == Some(lit) {
        *pos += lit.len();
        Ok(())
    } else {
        Err(false)
    }
}

fn until<'a>(b: &'a [u8], pos: &mut usize, end: u8) -> std::result::Result<&'a str, bool> {
    let rest = b.get(*pos..).ok_or(false)?;
    let n = rest.iter().position(|c| *c == end).ok_or(false)?;
    *pos += n + 1;
    std::str::from_utf8(&rest[..n]).map_err(|_| false)
}

/// `[[key, value], …]` rows folded into `{key: value | [value, …]}` with PHP-serialized values
/// decoded, exactly like export.php's `$meta_of`. Undecodable objects stay raw and set `objects`.
fn fold_meta(pairs: &Value, objects: &mut bool) -> Map<String, Value> {
    let mut grouped: Map<String, Value> = Map::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for pair in pairs.as_array().into_iter().flatten() {
        let Some(k) = pair.get(0).and_then(Value::as_str) else {
            continue;
        };
        let v = match pair.get(1) {
            Some(Value::String(raw)) => match php_decode(raw) {
                Php::Value(v) => v,
                Php::Object => {
                    *objects = true;
                    json!(raw)
                }
                Php::Plain => json!(raw),
            },
            Some(v) => v.clone(),
            None => Value::Null,
        };
        let n = counts.entry(k.to_string()).or_default();
        *n += 1;
        match (*n, grouped.get_mut(k)) {
            (1, _) | (_, None) => {
                grouped.insert(k.to_string(), v);
            }
            (2, Some(first)) => *first = json!([first.take(), v]),
            (_, Some(Value::Array(a))) => a.push(v),
            (_, Some(_)) => {}
        }
    }
    grouped
}

fn is_gzip(p: &Path) -> Result<bool> {
    let mut magic = [0u8; 2];
    let n = std::fs::File::open(p)
        .with_context(|| format!("opening {}", p.display()))?
        .read(&mut magic)?;
    Ok(n == 2 && magic == [0x1f, 0x8b])
}

#[async_trait]
impl Connector for WpDbConnector {
    fn name(&self) -> &str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            reads: [
                Feature::Drafts,
                Feature::Forms,
                Feature::Locales,
                Feature::Media,
                Feature::Menus,
                Feature::Redirects,
                Feature::Seo,
                Feature::Submissions,
                Feature::Taxonomies,
            ]
            .into_iter()
            .collect(),
            writes: Default::default(),
        }
    }

    /// Cheap checks only (Docker, the dump, the prefix); loading the dump is extract's job.
    async fn analyse(&self, ctx: &StageCtx) -> Result<()> {
        self.check_prefix()?;
        require_docker(&ctx.cancel).await?;
        let (dump, root) = self.paths(ctx)?;
        let dir = ctx.project.raw_dir(NAME);
        std::fs::create_dir_all(&dir)?;
        let analysis = json!({
            "dump_bytes": std::fs::metadata(&dump)?.len(),
            "gzip": is_gzip(&dump)?,
            "wp_root": root.is_some(),
        });
        Ok(std::fs::write(
            dir.join("analysis.json"),
            serde_json::to_vec_pretty(&analysis)?,
        )?)
    }

    async fn extract(&self, ctx: &StageCtx) -> Result<()> {
        self.check_prefix()?;
        require_docker(&ctx.cancel).await?;
        Sandbox::sweep_stale(&ctx.cancel).await?;
        let mut sb = Sandbox::new(&ctx.project.config().project.name);
        let r = self.extract_in(ctx, &sb).await;
        sb.cleanup().await;
        r.map_err(|e| anyhow!("{}", format!("{e:#}").replace(&sb.password, "[redacted]")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn tail_without_secret_is_untouched() {
        use std::os::unix::process::ExitStatusExt;
        let out = Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: vec![],
            stderr: b"daemon not running\nsecret-pw here".to_vec(),
        };
        assert_eq!(tail(&out, ""), "daemon not running\nsecret-pw here");
        assert_eq!(
            tail(&out, "secret-pw"),
            "daemon not running\n[redacted] here"
        );
    }

    #[cfg(unix)]
    #[test]
    fn regular_file_rejects_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("credentials");
        std::fs::write(&target, "secret").unwrap();
        let link = d.path().join("export.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = regular_file(&link).unwrap_err().to_string();
        assert!(
            err.contains("exporter produced a non-regular file"),
            "{err}"
        );
        std::fs::create_dir(d.path().join("dir")).unwrap();
        assert!(regular_file(&d.path().join("dir")).is_err());
        assert!(regular_file(&target).is_ok());
    }

    #[test]
    fn php_decoder() {
        let ok = |s: &str| match php_decode(s) {
            Php::Value(v) => v,
            other => panic!("{s}: {other:?}"),
        };
        assert_eq!(ok("i:42;"), json!(42));
        assert_eq!(ok("b:1;"), json!(true));
        assert_eq!(ok("N;"), Value::Null);
        assert_eq!(ok("d:1.5;"), json!(1.5));
        // Lengths are bytes: é is two.
        assert_eq!(ok("s:6:\"caf\u{e9};\";"), json!("caf\u{e9};"));
        assert_eq!(
            ok("a:2:{i:0;s:30:\"advanced-custom-fields/acf.php\";i:1;s:3:\"x;y\";}"),
            json!(["advanced-custom-fields/acf.php", "x;y"])
        );
        assert_eq!(
            ok(
                "a:2:{s:5:\"width\";i:800;s:5:\"sizes\";a:1:{s:5:\"thumb\";a:1:{s:4:\"file\";s:5:\"a.jpg\";}}}"
            ),
            json!({"width": 800, "sizes": {"thumb": {"file": "a.jpg"}}})
        );
        assert_eq!(php_decode("O:8:\"stdClass\":0:{}"), Php::Object);
        assert_eq!(php_decode("a:1:{i:0;O:8:\"stdClass\":0:{}}"), Php::Object);
        assert_eq!(php_decode("hello"), Php::Plain);
        assert_eq!(php_decode("s:5:\"abc\";"), Php::Plain);
        assert_eq!(php_decode("i:1;junk"), Php::Plain);
    }

    #[test]
    fn fold_keeps_repeated_keys() {
        let mut objects = false;
        let pairs = json!([
            ["_wp_old_slug", "first"],
            ["_wp_old_slug", "second"],
            ["_wp_old_slug", "third"],
            ["_edit_lock", "1"],
            ["sizes", "a:1:{i:0;i:7;}"],
            ["obj", "O:8:\"stdClass\":0:{}"],
        ]);
        let m = fold_meta(&pairs, &mut objects);
        assert_eq!(m["_wp_old_slug"], json!(["first", "second", "third"]));
        assert_eq!(m["_edit_lock"], json!("1"));
        assert_eq!(m["sizes"], json!([7]));
        assert_eq!(m["obj"], json!("O:8:\"stdClass\":0:{}"));
        assert!(objects);
    }

    #[test]
    fn stale_after_three_hours() {
        let now = 100_000;
        let listing = format!(
            "old-db\t{}\nnew-db\t{}\nno-label\t\n",
            now - STALE_AFTER_SECS - 1,
            now - 60
        );
        assert_eq!(stale_names(&listing, now), vec!["old-db".to_string()]);
    }

    #[test]
    fn sanitise_names() {
        assert_eq!(sanitise("My Site!! 2"), "my-site-2");
        assert_eq!(sanitise("--"), "project");
        assert_eq!(sanitise(&"a".repeat(80)).len(), 30);
    }

    #[test]
    fn permalinks_from_structure() {
        let pages: HashMap<u64, (String, u64)> =
            [(1, ("parent".into(), 0)), (2, ("child".into(), 1))].into();
        let home = "https://example.com";
        let page = json!({"id": 2, "type": "page", "slug": "child"});
        assert_eq!(
            permalink(home, "/%postname%/", &page, &pages, None, None).as_deref(),
            Some("https://example.com/parent/child/")
        );
        let post = json!({"id": 9, "type": "post", "slug": "hi", "date": "2024-03-05 10:00:00"});
        assert_eq!(
            permalink(
                home,
                "/%year%/%monthnum%/%category%/%postname%/",
                &post,
                &pages,
                Some("news"),
                None
            )
            .as_deref(),
            Some("https://example.com/2024/03/news/hi/")
        );
        assert_eq!(
            permalink(home, "", &post, &pages, None, None).as_deref(),
            Some("https://example.com/?p=9")
        );
        let cpt = json!({"id": 4, "type": "event", "slug": "gig"});
        assert_eq!(
            permalink(home, "/%postname%/", &cpt, &pages, None, None).as_deref(),
            Some("https://example.com/event/gig/")
        );
        let att =
            json!({"id": 5, "type": "attachment", "meta": {"_wp_attached_file": "2024/03/a.jpg"}});
        assert_eq!(
            permalink(home, "/%postname%/", &att, &pages, None, None).as_deref(),
            Some("https://example.com/wp-content/uploads/2024/03/a.jpg")
        );
    }
}
