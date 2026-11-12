use super::{
    Ctx, DB_FILE, Hub, PULL_BAK, PULL_TMP, SyncState, check_leftovers, copy_db, db_in_use,
    download, is_hash, local_changes, media_path, progress, sha256_file, storage_client,
};
use crate::output::print_json;
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Caps: the compressed snapshot, its unpacked size, and one media object.
const MAX_SNAPSHOT: u64 = 20 << 30;
const MAX_UNPACKED: u64 = 50 << 30;
const MAX_MEDIA: u64 = 5 << 30;
const PARALLEL: usize = 8;
const MAX_ENTRIES: u64 = 1_000_000;

pub enum Pulled {
    Done,
    /// Ctrl-C before the swap: nothing changed.
    Interrupted,
    /// Local work would be lost; the reason says how to proceed.
    Refused(String),
}

pub async fn pull(ctx: &Ctx, hub: &Hub, version: Option<u64>, force: bool) -> Result<Pulled> {
    let dir = &ctx.dir;
    check_leftovers(dir)?;
    // Extract and the db checks don't yield, so the caller's Ctrl-C `select!` can't see a
    // signal until after the swap; this flag is checked right before it instead.
    let interrupted = Arc::new(AtomicBool::new(false));
    let _listener = AbortOnDrop(tokio::spawn({
        let flag = interrupted.clone();
        async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }));
    // Even with --force: replacing a db another process has open loses its writes.
    if let Some(why) = db_in_use(dir)? {
        bail!("{why}");
    }
    if !force && let Some(why) = local_changes(dir, SyncState::read(dir, hub).as_ref())? {
        return Ok(Pulled::Refused(why));
    }
    let n = match version {
        Some(n) => n,
        None => {
            let list = hub
                .project_ok(Method::GET, &["versions"], None, StatusCode::OK)
                .await?;
            match list
                .get(0)
                .and_then(|v| v.get("number"))
                .and_then(Value::as_u64)
            {
                Some(n) => n,
                None => bail!("the hub project has no versions yet"),
            }
        }
    };
    let v = hub
        .project_ok(
            Method::GET,
            &["versions", &n.to_string()],
            None,
            StatusCode::OK,
        )
        .await?;
    let want = v
        .get("snapshot_sha256")
        .and_then(Value::as_str)
        .filter(|h| is_hash(h))
        .context("this version has no snapshot checksum, so it can't be verified")?;
    let url = v
        .get("snapshot_get_url")
        .and_then(Value::as_str)
        .context("the hub sent no snapshot URL")?;

    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let work = Staging(dir.join(PULL_TMP));
    std::fs::create_dir(&work.0).with_context(|| format!("creating {}", work.0.display()))?;
    let archive = work.0.join("snapshot.tar.zst");
    let s3 = storage_client()?;
    if !ctx.json {
        eprintln!("downloading v{n}");
    }
    let got = download(&s3, url, &archive, MAX_SNAPSHOT).await?;
    if got != want {
        bail!("the snapshot for v{n} does not match its sha256 (got {got}, want {want})");
    }
    let unpacked = work.0.join("new");
    extract(&archive, &unpacked)?;
    if interrupted.load(Ordering::SeqCst) {
        return Ok(Pulled::Interrupted);
    }
    // Last check right before the swap: an app may have opened the project meanwhile.
    if let Some(why) = db_in_use(dir)? {
        bail!("{why}");
    }
    swap(dir, &unpacked)?;
    // Recorded now, so a failed media download below doesn't leave a stale sync file.
    let db = copy_db(dir, &work.0.join(DB_FILE))?;
    SyncState::write(dir, hub, n, db, sha256_file(&dir.join("porter.toml"))?)?;

    let hashes: Vec<String> = v
        .get("media_hashes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|h| h.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let need: Vec<&String> = hashes
        .iter()
        .filter(|h| is_hash(h) && !media_path(dir, h).exists())
        .collect();
    let total = need.len() as u64;
    let mut done = 0u64;
    for batch in need.chunks(1000) {
        let r = hub
            .project_ok(
                Method::POST,
                &["media", "urls"],
                Some(&json!({ "hashes": batch })),
                StatusCode::OK,
            )
            .await?;
        if let Some(m) = r
            .get("missing")
            .and_then(Value::as_array)
            .filter(|m| !m.is_empty())
        {
            eprintln!(
                "warning: the hub is missing {} media objects of v{n}",
                m.len()
            );
        }
        let urls: HashMap<String, String> =
            serde_json::from_value(r.get("urls").cloned().unwrap_or_default())
                .context("the hub's media urls response")?;
        let mut gets = futures::stream::iter(batch.iter().filter_map(|h| {
            let url = urls.get(h.as_str())?.clone();
            let s3 = &s3;
            Some(async move { fetch_media(s3, &url, dir, h).await })
        }))
        .buffer_unordered(PARALLEL);
        while let Some(r) = gets.next().await {
            r?;
            done += 1;
            progress(ctx.json, "media", done, total);
        }
    }
    if ctx.json {
        print_json(&json!({"v": 1, "event": "hub-pulled", "version": n, "media_downloaded": done}));
    } else {
        eprintln!("pulled v{n} ({done} media downloaded)");
    }
    Ok(Pulled::Done)
}

/// Downloads one media object to its hash path, verified, via a temp file.
async fn fetch_media(s3: &reqwest::Client, url: &str, dir: &Path, hash: &str) -> Result<()> {
    let dest = media_path(dir, hash);
    let parent = dest.parent().context("media path")?;
    std::fs::create_dir_all(parent)?;
    let tmp = tempfile::Builder::new()
        .prefix(".dl-")
        .tempfile_in(parent)?
        .into_temp_path();
    std::fs::remove_file(&tmp)?;
    let got = download(s3, url, &tmp, MAX_MEDIA).await?;
    if got != hash {
        bail!("media {hash} downloaded with the wrong sha256 ({got})");
    }
    std::fs::rename(&tmp, &dest).with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

/// Moves `.bak` into the staging dir (next to `new`) in one rename.
fn retire(bak: &Path, new: &Path) -> Result<()> {
    let to = new.with_file_name("old");
    std::fs::rename(bak, &to).with_context(|| format!("moving {} into staging", bak.display()))
}

/// Removes the pull staging dir however the pull ends (a crash leaves it for
/// `check_leftovers`, which deletes it).
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Unpacks regular files and folders only, with relative, `..`-free paths, never a
/// top-level `media/`, `uploads/` or dot-entry, no two top-level names that differ only by
/// case, at most `MAX_ENTRIES` entries and `MAX_UNPACKED` bytes.
pub fn extract(archive: &Path, into: &Path) -> Result<()> {
    let f = std::fs::File::open(archive)?;
    let mut ar = tar::Archive::new(zstd::Decoder::new(f)?);
    std::fs::create_dir_all(into)?;
    let (mut total, mut count) = (0u64, 0u64);
    let mut top: HashMap<String, String> = HashMap::new();
    for entry in ar.entries().context("reading the snapshot")? {
        let mut e = entry.context("reading the snapshot")?;
        count += 1;
        if count > MAX_ENTRIES {
            bail!("snapshot has more than {MAX_ENTRIES} entries");
        }
        let path = e.path()?.into_owned();
        let shown = path.display().to_string();
        if !path.components().all(|c| matches!(c, Component::Normal(_))) {
            bail!("unsafe path in snapshot: {shown}");
        }
        let Some(Component::Normal(first)) = path.components().next() else {
            bail!("unsafe path in snapshot: {shown}");
        };
        let first = first.to_string_lossy().into_owned();
        let lower = first.to_ascii_lowercase();
        if first.starts_with('.') || lower == "media" || lower == "uploads" {
            bail!("unexpected entry in snapshot: {shown}");
        }
        if let Some(prev) = top.insert(lower, first.clone())
            && prev != first
        {
            bail!("snapshot has entries that differ only by case: {prev} and {first}");
        }
        let dest = into.join(&path);
        match e.header().entry_type() {
            tar::EntryType::Directory => std::fs::create_dir_all(&dest)?,
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                total += e.size();
                if total > MAX_UNPACKED {
                    bail!("snapshot unpacks to more than {MAX_UNPACKED} bytes");
                }
                if let Some(p) = dest.parent() {
                    std::fs::create_dir_all(p)?;
                }
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&dest)
                    .with_context(|| format!("unpacking {shown}"))?;
                std::io::copy(&mut e, &mut out).with_context(|| format!("unpacking {shown}"))?;
            }
            t => bail!(
                "unsafe entry in snapshot: {shown} ({t:?}); only files and folders are allowed"
            ),
        }
    }
    Ok(())
}

/// Moves the unpacked top-level entries into `dir`. Each replaced entry goes to
/// `PULL_BAK` first, so a crash leaves every original there (see `check_leftovers`). On an
/// error the completed moves are undone in reverse; `PULL_BAK` is deleted only once the
/// project is whole again (new or old). Local entries the snapshot lacks are kept, except
/// SQLite sidecars of a replaced project.db.
fn swap(dir: &Path, new: &Path) -> Result<()> {
    let bak = dir.join(PULL_BAK);
    std::fs::create_dir(&bak).with_context(|| format!("creating {}", bak.display()))?;
    let mut names: BTreeSet<_> = std::fs::read_dir(new)?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if names.iter().any(|n| n.eq_ignore_ascii_case(DB_FILE)) {
        names.extend(["project.db-wal", "project.db-shm", "project.db-journal"].map(String::from));
    }
    // (name, an original was moved to bak, the new one was moved in)
    let mut done: Vec<(String, bool, bool)> = Vec::new();
    let step = |name: &str, done: &mut Vec<(String, bool, bool)>| -> Result<()> {
        let (src, dst) = (new.join(name), dir.join(name));
        let had = std::fs::symlink_metadata(&dst).is_ok();
        if had {
            std::fs::rename(&dst, bak.join(name))
                .with_context(|| format!("moving {} aside", dst.display()))?;
        }
        done.push((name.to_string(), had, false));
        if src.exists() {
            std::fs::rename(&src, &dst).with_context(|| format!("writing {}", dst.display()))?;
            if let Some(last) = done.last_mut() {
                last.2 = true;
            }
        }
        Ok(())
    };
    let mut failed = None;
    for name in &names {
        if let Err(e) = step(name, &mut done) {
            failed = Some(e);
            break;
        }
    }
    let Some(err) = failed else {
        // One atomic rename retires the originals into staging, which `Staging` deletes.
        // A crash from here on leaves only `.tmp` (cleaned up next run), never a half-deleted
        // `.bak` that recovery would restore over the complete new files.
        retire(&bak, new)?;
        return Ok(());
    };
    let rollback = || -> std::io::Result<()> {
        for (name, had, moved_in) in done.iter().rev() {
            if *moved_in {
                std::fs::rename(dir.join(name), new.join(name))?;
            }
            if *had {
                std::fs::rename(bak.join(name), dir.join(name))?;
            }
        }
        Ok(())
    };
    match rollback() {
        Ok(()) => {
            let _ = retire(&bak, new);
            Err(err.context("pull failed; the project was left as it was"))
        }
        Err(re) => Err(err.context(format!(
            "pull failed and undoing it failed too ({re}); the original entries are in {}",
            bak.display()
        ))),
    }
}
