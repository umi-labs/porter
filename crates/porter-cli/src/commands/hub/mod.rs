//! `porter hub …`: sync a project with a Porter hub and run stages there.
//!
//! The token only ever goes to the hub origin: `Hub::call` builds every authed URL from
//! that origin, and the authed client never follows redirects. Presigned storage URLs are
//! fetched with a separate client that has no auth header.

pub mod link;
pub mod login;
pub mod pull;
pub mod push;
pub mod run;
pub mod uploads;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::{Client, Method, StatusCode, redirect};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

pub const KEYRING_SERVICE: &str = "porter-hub";
pub const TOKEN_ENV: &str = "PORTER_HUB_TOKEN";
/// The origin `PORTER_HUB_TOKEN` belongs to; the env token is used only for it.
pub const URL_ENV: &str = "PORTER_HUB_URL";
pub const SYNC_FILE: &str = ".porter-hub.json";
const USER_AGENT: &str = concat!("porter/", env!("CARGO_PKG_VERSION"));

/// Auth: `porter hub login` stores a token in the system keyring per hub origin. In CI,
/// set PORTER_HUB_TOKEN together with PORTER_HUB_URL (the hub it belongs to); the env
/// token is refused for any other hub origin.
#[derive(Args)]
#[command(
    after_help = "Environment: PORTER_HUB_TOKEN (a hub session token) is used only when PORTER_HUB_URL is set to the same origin as the target hub; otherwise the command fails. Without them, the keyring entry stored by `porter hub login` for that exact origin is used."
)]
pub struct HubArgs {
    /// Hub URL (https; http only for localhost). Defaults to `[hub] url` in porter.toml
    #[arg(long, global = true)]
    pub url: Option<String>,
    /// Hub project slug. Defaults to `[hub] project` in porter.toml
    #[arg(long, global = true)]
    pub slug: Option<String>,
    #[command(subcommand)]
    pub cmd: HubCmd,
}

#[derive(Subcommand)]
pub enum HubCmd {
    /// Sign in with the device flow; the token goes in the system keyring
    Login,
    /// Remove the stored token
    Logout,
    /// Show the signed-in user
    Whoami,
    /// Write `[hub] url, project` into porter.toml (no secrets)
    Link {
        #[arg(value_name = "SLUG")]
        project_slug: String,
    },
    /// Upload the project (without media) and its missing media as a new version
    Push {
        #[arg(long, short)]
        message: Option<String>,
        /// Push even if the hub has a newer version
        #[arg(long)]
        force: bool,
    },
    /// Download a version and its missing media into the project
    Pull {
        /// Version number (default: the latest)
        #[arg(long = "version")]
        version: Option<u64>,
        /// Overwrite local changes
        #[arg(long)]
        force: bool,
    },
    /// Run stages on the hub, stream the events, then pull the result
    Run {
        #[arg(required = true, value_parser = ["all", "analyse", "extract", "normalise", "schema-check", "load", "verify"])]
        stages: Vec<String>,
        /// Really write to the destination (needs --confirm-host)
        #[arg(long)]
        apply: bool,
        /// The destination host, typed out to confirm --apply
        #[arg(long)]
        confirm_host: Option<String>,
    },
    /// List client uploads, or `get <id>` to download one
    Uploads {
        #[command(subcommand)]
        cmd: Option<UploadsCmd>,
    },
}

#[derive(Subcommand)]
pub enum UploadsCmd {
    /// Download an upload (verified by sha256); never overwrites
    Get {
        id: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

pub struct Ctx {
    pub dir: PathBuf,
    pub json: bool,
    pub interactive: bool,
}

pub async fn dispatch(ctx: Ctx, args: HubArgs) -> Result<i32> {
    let HubArgs { url, slug, cmd } = args;
    let conf = HubConf::read(&ctx.dir);
    let origin = || -> Result<Url> {
        let u = url
            .clone()
            .or_else(|| conf.url.clone())
            .or_else(|| conf.sync.as_ref().map(|s| s.0.clone()))
            .ok_or_else(|| {
                anyhow!("no hub URL: pass --url or run `porter hub link <slug> --url <hub>`")
            })?;
        origin(&u)
    };
    let slug = || -> Result<String> {
        // The last sync's project only counts for the hub it was synced with.
        let o = origin().ok().map(|o| o.origin().ascii_serialization());
        let s = slug
            .clone()
            .or_else(|| conf.project.clone())
            .or_else(|| {
                conf.sync
                    .as_ref()
                    .filter(|s| Some(&s.0) == o.as_ref())
                    .map(|s| s.1.clone())
            })
            .ok_or_else(|| {
                anyhow!("no hub project: pass --slug or run `porter hub link <slug>`")
            })?;
        check_slug(&s)?;
        Ok(s)
    };
    match cmd {
        HubCmd::Login => login::login(&ctx, origin()?).await,
        HubCmd::Logout => login::logout(&ctx, origin()?).await,
        HubCmd::Whoami => login::whoami(&ctx, &Hub::new(origin()?, String::new())?).await,
        HubCmd::Link { project_slug } => link::run(&ctx, &project_slug, url.or(conf.url)),
        HubCmd::Push { message, force } => {
            push::run(&ctx, &Hub::new(origin()?, slug()?)?, message, force).await
        }
        HubCmd::Pull { version, force } => {
            let hub = Hub::new(origin()?, slug()?)?;
            // Ctrl-C exits 3. The swap has no await inside, so it is never cut off halfway.
            let pulled = tokio::select! {
                r = pull::pull(&ctx, &hub, version, force) => r?,
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("interrupted; run `porter hub pull` again to finish");
                    return Ok(3);
                }
            };
            match pulled {
                pull::Pulled::Done => Ok(0),
                pull::Pulled::Interrupted => {
                    eprintln!("interrupted before anything changed; run `porter hub pull` again");
                    Ok(3)
                }
                pull::Pulled::Refused(why) => bail!("{why}"),
            }
        }
        HubCmd::Run {
            stages,
            apply,
            confirm_host,
        } => {
            run::run(
                &ctx,
                &Hub::new(origin()?, slug()?)?,
                stages,
                apply,
                confirm_host,
            )
            .await
        }
        HubCmd::Uploads { cmd: None } => uploads::list(&ctx, &Hub::new(origin()?, slug()?)?).await,
        HubCmd::Uploads {
            cmd: Some(UploadsCmd::Get { id, out }),
        } => uploads::get(&ctx, &Hub::new(origin()?, slug()?)?, &id, out).await,
    }
}

/// `[hub]` from porter.toml, read loosely so a config error elsewhere doesn't block sync,
/// and the last sync's (hub, project) as a fallback (a pulled porter.toml may lack `[hub]`).
#[derive(Default)]
struct HubConf {
    url: Option<String>,
    project: Option<String>,
    sync: Option<(String, String)>,
}

impl HubConf {
    fn read(dir: &Path) -> HubConf {
        let text = std::fs::read_to_string(dir.join("porter.toml")).unwrap_or_default();
        let t: toml::Table = toml::from_str(&text).unwrap_or_default();
        let get = |k: &str| t.get("hub")?.get(k)?.as_str().map(str::to_string);
        let sync: Option<SyncState> = std::fs::read_to_string(dir.join(SYNC_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        HubConf {
            url: get("url"),
            project: get("project"),
            sync: sync.map(|s| (s.hub, s.project)),
        }
    }
}

/// The hub origin: https, or http only for localhost / 127.0.0.1 (dev). No credentials.
pub fn origin(raw: &str) -> Result<Url> {
    let u = Url::parse(raw.trim()).with_context(|| format!("hub URL {raw:?} is not a URL"))?;
    if !u.username().is_empty() || u.password().is_some() {
        bail!("the hub URL must not contain credentials");
    }
    let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match u.scheme() {
        "https" => {}
        "http" if local => {}
        _ => bail!("the hub URL must be https (http only for localhost): {raw}"),
    }
    Url::parse(&u.origin().ascii_serialization()).context("hub origin")
}

fn check_slug(s: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 100
        || !s
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        bail!("invalid hub project slug {s:?}");
    }
    Ok(())
}

/// The token for `origin`: `PORTER_HUB_TOKEN`, else the keyring. Never logged.
///
/// The env token is only sent to the origin `PORTER_HUB_URL` names, so a porter.toml
/// (say, one from a pulled project) can't point a CI token at another host. Keyring
/// tokens are stored per origin, so a URL only ever gets its own.
pub fn token(origin: &Url) -> Result<String> {
    let account = origin.origin().ascii_serialization();
    if let Ok(t) = std::env::var(TOKEN_ENV)
        && !t.trim().is_empty()
    {
        let bound = std::env::var(URL_ENV)
            .ok()
            .and_then(|u| self::origin(&u).ok())
            .map(|u| u.origin().ascii_serialization());
        if bound.as_deref() != Some(account.as_str()) {
            bail!(
                "{TOKEN_ENV} is set but {URL_ENV} is {}, not the target hub {account}; set {URL_ENV}={account} to use the token there",
                bound.map_or("unset or invalid".to_string(), |b| format!("{b:?}"))
            );
        }
        return Ok(t.trim().to_string());
    }
    match keyring::Entry::new(KEYRING_SERVICE, &account).and_then(|e| e.get_password()) {
        Ok(t) => Ok(t),
        Err(keyring::Error::NoEntry) => bail!(
            "not signed in to {account}: run `porter hub login` or set {TOKEN_ENV} and {URL_ENV}"
        ),
        Err(e) => {
            bail!("the system keyring is unavailable ({e}); set {TOKEN_ENV} and {URL_ENV} instead")
        }
    }
}

/// An authed hub client for one origin (and project, when `slug` is set).
pub struct Hub {
    pub origin: Url,
    pub slug: String,
    http: Client,
    auth: Option<HeaderValue>,
}

impl Hub {
    /// Reads the token now, so a command fails fast when signed out.
    pub fn new(origin: Url, slug: String) -> Result<Hub> {
        let t = token(&origin)?;
        Hub::with_token(origin, slug, &t)
    }

    pub fn with_token(origin: Url, slug: String, t: &str) -> Result<Hub> {
        let mut auth = HeaderValue::from_str(&format!("Bearer {t}"))
            .map_err(|_| anyhow!("the hub token has characters that can't go in a header"))?;
        auth.set_sensitive(true);
        Ok(Hub {
            origin,
            slug,
            http: authed_client()?,
            auth: Some(auth),
        })
    }

    /// For the device flow, before there is a token.
    pub fn anonymous(origin: Url) -> Result<Hub> {
        Ok(Hub {
            origin,
            slug: String::new(),
            http: authed_client()?,
            auth: None,
        })
    }

    pub fn origin_str(&self) -> String {
        self.origin.origin().ascii_serialization()
    }

    /// `origin` + percent-encoded segments. The only way an authed URL is built.
    pub fn url(&self, segs: &[&str]) -> Url {
        let mut u = self.origin.clone();
        if let Ok(mut p) = u.path_segments_mut() {
            p.clear().extend(segs);
        }
        u
    }

    /// Sends to `origin/segs` with the token; returns the status and the JSON body (or Null).
    pub async fn call(
        &self,
        method: Method,
        segs: &[&str],
        body: Option<&Value>,
    ) -> Result<(StatusCode, Value)> {
        let resp = self
            .request(method, segs, body)
            .send()
            .await
            .map_err(|e| anyhow!("hub request failed: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    pub fn request(
        &self,
        method: Method,
        segs: &[&str],
        body: Option<&Value>,
    ) -> reqwest::RequestBuilder {
        let mut rb = self.http.request(method, self.url(segs));
        if let Some(a) = &self.auth {
            rb = rb.header(AUTHORIZATION, a.clone());
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        rb
    }

    /// `/api/projects/<slug>/<rest…>`.
    pub async fn project(
        &self,
        method: Method,
        rest: &[&str],
        body: Option<&Value>,
    ) -> Result<(StatusCode, Value)> {
        let mut segs = vec!["api", "projects", self.slug.as_str()];
        segs.extend_from_slice(rest);
        self.call(method, &segs, body).await
    }

    /// Like `project`, but anything other than `ok` is an error carrying the hub's message.
    pub async fn project_ok(
        &self,
        method: Method,
        rest: &[&str],
        body: Option<&Value>,
        ok: StatusCode,
    ) -> Result<Value> {
        let (st, v) = self.project(method, rest, body).await?;
        if st != ok {
            bail!("{}", hub_error(st, &v));
        }
        Ok(v)
    }
}

/// "hub returned 403 Forbidden: <error>".
pub fn hub_error(st: StatusCode, v: &Value) -> String {
    let hint = match st {
        StatusCode::UNAUTHORIZED => " (sign in with `porter hub login` or check PORTER_HUB_TOKEN)",
        StatusCode::NOT_FOUND => " (unknown project, or you are not a member)",
        _ => "",
    };
    match v.get("error").and_then(Value::as_str) {
        Some(e) => format!("hub returned {st}: {}{hint}", clean(e)),
        None => format!("hub returned {st}{hint}"),
    }
}

fn authed_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        // A redirect could carry the Authorization header elsewhere.
        .redirect(redirect::Policy::none())
        .connect_timeout(Duration::from_secs(30))
        // SSE sends a ping every 15 s, so a minute of silence is a dead connection.
        .read_timeout(Duration::from_secs(60))
        .build()
        .context("building the HTTP client")
}

/// For presigned storage URLs: no auth header, no redirects.
pub fn storage_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .redirect(redirect::Policy::none())
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(120))
        .build()
        .context("building the HTTP client")
}

/// `.porter-hub.json`: what the project was last synced to.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SyncState {
    pub hub: String,
    pub project: String,
    pub version: u64,
    pub project_db_sha256: Option<String>,
    pub porter_toml_sha256: Option<String>,
}

impl SyncState {
    /// The sync file, only if it is for this hub and project.
    pub fn read(dir: &Path, hub: &Hub) -> Option<SyncState> {
        let text = std::fs::read_to_string(dir.join(SYNC_FILE)).ok()?;
        let s: SyncState = serde_json::from_str(&text).ok()?;
        (s.hub == hub.origin_str() && s.project == hub.slug).then_some(s)
    }

    /// `db` is `db_hash` of the synced database; `toml` the sha256 of the synced porter.toml.
    pub fn write(
        dir: &Path,
        hub: &Hub,
        version: u64,
        db: Option<String>,
        toml: Option<String>,
    ) -> Result<()> {
        let s = SyncState {
            hub: hub.origin_str(),
            project: hub.slug.clone(),
            version,
            project_db_sha256: db,
            porter_toml_sha256: toml,
        };
        let text = serde_json::to_string_pretty(&s)? + "\n";
        porter_engine::mapping::edit::write_atomic(&dir.join(SYNC_FILE), &text)
    }
}

pub const DB_FILE: &str = "project.db";
/// Staging for a pull (safe to delete while `PULL_BAK` is absent).
pub const PULL_TMP: &str = ".porter-hub-pull.tmp";
/// The entries a pull replaced, kept until the swap is complete.
pub const PULL_BAK: &str = ".porter-hub-pull.bak";

/// A consistent copy of project.db at `out` (`VACUUM INTO` reads one transaction, so
/// commits still in the WAL are included and a concurrent writer can't tear it), and its
/// `db_hash`. `None` when there is no project.db.
pub fn copy_db(dir: &Path, out: &Path) -> Result<Option<String>> {
    let src = dir.join(DB_FILE);
    if !src.exists() {
        return Ok(None);
    }
    let conn =
        rusqlite::Connection::open(&src).with_context(|| format!("opening {}", src.display()))?;
    conn.busy_timeout(Duration::from_secs(10))?;
    let target = out
        .to_str()
        .ok_or_else(|| anyhow!("temp path {} is not UTF-8", out.display()))?;
    conn.execute("VACUUM INTO ?1", [target])
        .with_context(|| format!("copying {}", src.display()))?;
    drop(conn);
    db_hash(out).map(Some)
}

/// sha256 of a database file with the header fields that change without any change to the
/// data zeroed: write/read versions (WAL flag), change counter, schema cookie,
/// version-valid-for and the SQLite version. Same data, same hash, after any reopen.
pub fn db_hash(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut head = [0u8; 100];
    f.read_exact(&mut head)
        .with_context(|| format!("{} is not a SQLite database", path.display()))?;
    for r in [18..20, 24..28, 40..44, 92..100] {
        head[r].fill(0);
    }
    let mut h = Sha256::new();
    h.update(head);
    std::io::copy(&mut f, &mut h)?;
    Ok(hex(&h.finalize()))
}

// ponytail: heuristic in-use check (WAL left behind after our close). Known gaps: the
// window between pull's second check and the end of the swap, and openers that don't use
// WAL (a `sqlite3` shell in rollback mode) aren't detected. Upgrade path: `Store::open`
// holds a shared advisory lock on a project lock file (e.g. flock on `.porter.lock`), and
// pull takes it exclusively, non-blocking, from the check through the swap.
/// Why project.db can't be replaced now: another process (a porter run, the TUI or the
/// desktop app) has it open. The WAL is checkpointed into the db first; when our
/// connection closes SQLite deletes the WAL, unless another connection still holds it.
pub fn db_in_use(dir: &Path) -> Result<Option<String>> {
    let src = dir.join(DB_FILE);
    if !src.exists() {
        return Ok(None);
    }
    let busy: i64 = {
        let conn = rusqlite::Connection::open(&src)
            .with_context(|| format!("opening {}", src.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
            .unwrap_or(1)
    };
    let wal = dir.join("project.db-wal").exists();
    Ok((busy != 0 || wal).then(|| {
        "project.db is open in another porter process (a run, the TUI or the desktop app); close it and try again".to_string()
    }))
}

/// A crashed pull leaves `PULL_BAK` (refuse, with recovery steps) or only `PULL_TMP`
/// (staging, removed).
pub fn check_leftovers(dir: &Path) -> Result<()> {
    let bak = dir.join(PULL_BAK);
    if bak.exists() {
        bail!(
            "an earlier pull was interrupted while swapping files in; the entries it replaced are in {bak}.\n\
             To undo it: move every entry in {bak} back into {dir} (replacing what is there), then delete {bak} and {tmp}.\n\
             To keep the pulled files instead: delete {bak} and {tmp}, then run `porter hub pull --force` to finish.",
            bak = bak.display(),
            dir = dir.display(),
            tmp = dir.join(PULL_TMP).display(),
        );
    }
    let tmp = dir.join(PULL_TMP);
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).with_context(|| format!("removing {}", tmp.display()))?;
    }
    Ok(())
}

/// Why pulling over the project would lose local work, if it would.
pub fn local_changes(dir: &Path, sync: Option<&SyncState>) -> Result<Option<String>> {
    let Some(s) = sync else {
        let non_empty = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .flatten()
                .any(|e| !e.file_name().to_string_lossy().starts_with('.')),
            Err(_) => false,
        };
        return Ok(non_empty.then(|| {
            format!(
                "{} has files but no {SYNC_FILE} for this hub project; use --force to overwrite",
                dir.display()
            )
        }));
    };
    let scratch = tempfile::tempdir().context("creating a temp dir")?;
    let db = copy_db(dir, &scratch.path().join(DB_FILE))?;
    let toml = sha256_file(&dir.join("porter.toml"))?;
    for (file, got, want) in [
        (DB_FILE, db, &s.project_db_sha256),
        ("porter.toml", toml, &s.porter_toml_sha256),
    ] {
        if &got != want {
            return Ok(Some(format!(
                "{file} changed since the last sync (v{}); push it first or use --force to overwrite",
                s.version
            )));
        }
    }
    Ok(None)
}

/// Replaces control characters (other than newline and tab) in server text shown on a
/// terminal, so it can't carry escape sequences.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '?'
            } else {
                c
            }
        })
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn is_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The `x-amz-checksum-sha256` header value: base64 of the raw digest.
pub fn checksum_b64(hex_hash: &str) -> Result<String> {
    use base64::Engine;
    if !is_hash(hex_hash) {
        bail!("invalid sha256 {hex_hash:?}");
    }
    let raw: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&hex_hash[i * 2..i * 2 + 2], 16))
        .collect::<Result<_, _>>()?;
    Ok(base64::engine::general_purpose::STANDARD.encode(raw))
}

/// sha256 of a file, `None` if it doesn't exist.
pub fn sha256_file(path: &Path) -> Result<Option<String>> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(hex(&h.finalize())))
}

/// `media/<h0h1>/<h2h3>/<sha256>`, the engine store's layout.
pub fn media_path(dir: &Path, hash: &str) -> PathBuf {
    dir.join("media")
        .join(&hash[0..2])
        .join(&hash[2..4])
        .join(hash)
}

/// Streams a presigned GET to `dest` (created, must not exist), up to `max` bytes, and
/// returns its sha256. The URL is dropped from errors (it carries a signature).
pub async fn download(s3: &Client, url: &str, dest: &Path, max: u64) -> Result<String> {
    use std::io::Write;
    let mut resp = s3
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("download failed: {}", e.without_url()))?;
    if !resp.status().is_success() {
        bail!("download failed: storage returned {}", resp.status());
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut h = Sha256::new();
    let mut n = 0u64;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| anyhow!("download failed: {}", e.without_url()))?
    {
        n += chunk.len() as u64;
        if n > max {
            bail!("download is larger than {max} bytes");
        }
        h.update(&chunk);
        f.write_all(&chunk)?;
    }
    f.sync_all()?;
    Ok(hex(&h.finalize()))
}

/// PUTs a file to a presigned URL with the signed checksum and `if-none-match: *`.
/// 412 means the object is already there (content is pinned by the checksum).
pub async fn upload(s3: &Client, url: &str, path: &Path, hash: &str) -> Result<()> {
    let len = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    let resp = s3
        .put(url)
        .header("x-amz-checksum-sha256", checksum_b64(hash)?)
        .header("if-none-match", "*")
        .header(reqwest::header::CONTENT_LENGTH, len)
        .body(reqwest::Body::from(file))
        .send()
        .await
        .map_err(|e| anyhow!("upload failed: {}", e.without_url()))?;
    let st = resp.status();
    if st.is_success() || st == StatusCode::PRECONDITION_FAILED {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    let code = body
        .split("<Code>")
        .nth(1)
        .and_then(|s| s.split('<').next())
        .unwrap_or("");
    bail!(
        "upload of {} failed: storage returned {st} {code}",
        path.display()
    )
}

/// Progress on stderr, or a `hub-progress` event with `--json`.
pub fn progress(json: bool, phase: &str, done: u64, total: u64) {
    if json {
        crate::output::print_json(&serde_json::json!(
            {"v": 1, "event": "hub-progress", "phase": phase, "done": done, "total": total}
        ));
    } else {
        eprintln!("{phase}: {done}/{total}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_rules() {
        assert_eq!(
            origin("https://hub.example.com/x/y").unwrap().as_str(),
            "https://hub.example.com/"
        );
        assert!(origin("http://localhost:3100").is_ok());
        assert!(origin("http://127.0.0.1:9").is_ok());
        assert!(origin("http://hub.example.com").is_err());
        assert!(origin("https://u:p@hub.example.com").is_err());
        assert!(origin("ftp://localhost").is_err());
    }

    #[test]
    fn checksum_is_base64_of_raw_digest() {
        // sha256("") = e3b0…b855
        let h = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(
            checksum_b64(h).unwrap(),
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
    }
}
