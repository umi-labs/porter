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
    let text = String::from_utf8_lossy(&out.stderr).replace(secret, "[redacted]");
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
    cli: String,
    password: String,
    clean: bool,
}

impl Sandbox {
    fn new(project: &str) -> Sandbox {
        let short = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let base = format!("porter-{}-{short}", sanitise(project));
        Sandbox {
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
            docker().args(["network", "create", "--internal", &self.db]),
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
        cmd.args(["run", "-d", "--name", &self.db, "--network", &self.db])
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
            "mariadb -uroot --max-allowed-packet=1G --default-character-set=utf8mb4 {DB_NAME}"
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
            file.exists(),
            "export.php wrote no export.json:\n{}",
            tail(&o, &self.password)
        );
        Ok(Some(file))
    }
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
    async fn sql_export(&self, sb: &Sandbox, cancel: &CancellationToken) -> Result<Option<Value>> {
        let p = &self.prefix;
        let Some(site) = sb
            .one(
                &format!(
                    "SELECT JSON_OBJECTAGG(option_name, option_value) FROM {p}options WHERE option_name IN \
                     ('home','siteurl','blogname','blogdescription','show_on_front','page_on_front',\
                     'page_for_posts','permalink_structure','wpseo_titles','active_plugins')"
                ),
                cancel,
            )
            .await?
        else {
            return Ok(None);
        };
        let mut posts = vec![];
        let select = format!(
            "SELECT JSON_OBJECT('id', p.ID, 'type', p.post_type, 'status', p.post_status, \
             'slug', p.post_name, 'title', p.post_title, 'parent', NULLIF(p.post_parent, 0), \
             'menu_order', p.menu_order, 'author', NULLIF(p.post_author, 0), 'date', p.post_date, \
             'date_gmt', p.post_date_gmt, 'modified_gmt', p.post_modified_gmt, \
             'mime', NULLIF(p.post_mime_type, ''), 'content', p.post_content, 'excerpt', p.post_excerpt, \
             'meta', (SELECT JSON_OBJECTAGG(m.meta_key, m.meta_value) FROM {p}postmeta m \
                      WHERE m.post_id = p.ID AND m.meta_key IS NOT NULL), \
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
             'description', tt.description, 'parent', NULLIF(tt.parent, 0), 'count', tt.count) \
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
            "SELECT JSON_OBJECT('id', ID, 'login', user_login, 'nicename', user_nicename, \
             'display_name', display_name, 'email', user_email) FROM {p}users"
        );
        if !sb
            .paged(&select, "ID", cancel, |rows| {
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
        for p in &mut posts {
            let category = p["term_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| term_slug.get(&t.as_u64()?))
                .find(|(tax, _)| tax == "category")
                .map(|(_, slug)| slug.clone());
            let author = p["author"].as_u64().and_then(|a| nicename.get(&a)).cloned();
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
                o.insert("acf".into(), Value::Null);
            }
        }
        let mut site = match site {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        site.insert("acf_active".into(), Value::Null);
        Ok(Some(json!({
            "site": site,
            "posts": posts,
            "terms": terms,
            "users": users,
            "menus": [],
            "acf_field_groups": [],
            "forms": { "gf": [], "nf": [] },
            "translations": null,
            "source": "sql-fallback",
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
    ) -> Result<bool> {
        let mut seen = 0u64;
        sb.paged(select, order, &ctx.cancel, |rows| {
            for v in rows {
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

    async fn sql_extras(&self, ctx: &StageCtx, sb: &Sandbox) -> Result<bool> {
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
                 'fields', (SELECT JSON_OBJECTAGG(m.meta_key, m.meta_value) FROM {p}gf_entry_meta m \
                            WHERE m.entry_id = e.id AND m.meta_key IS NOT NULL)) \
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
             'fields', (SELECT JSON_OBJECTAGG(m.meta_key, m.meta_value) FROM {p}postmeta m \
                        WHERE m.post_id = p.ID AND m.meta_key LIKE '\\\\_field\\\\_%')) \
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
                std::fs::copy(&file, dir.join("export.json"))?;
                let f = std::fs::File::open(dir.join("export.json"))?;
                serde_json::from_reader(std::io::BufReader::new(f))
                    .context("export.json is not valid JSON")?
            }
            None => {
                ctx.progress(0, None, "reading tables");
                let Some(v) = self.sql_export(sb, cancel).await? else {
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
        self.sql_extras(ctx, sb).await?;
        Ok(())
    }
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
        let mut sb = Sandbox::new(&ctx.project.config().project.name);
        let r = self.extract_in(ctx, &sb).await;
        sb.cleanup().await;
        r.map_err(|e| anyhow!("{}", format!("{e:#}").replace(&sb.password, "[redacted]")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
