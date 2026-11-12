use super::{
    Ctx, Hub, SyncState, download, is_hash, local_changes, media_path, progress, storage_client,
};
use crate::output::print_json;
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Component, Path};

/// Caps: the compressed snapshot, its unpacked size, and one media object.
const MAX_SNAPSHOT: u64 = 20 << 30;
const MAX_UNPACKED: u64 = 50 << 30;
const MAX_MEDIA: u64 = 5 << 30;
const PARALLEL: usize = 8;

pub enum Pulled {
    Done,
    /// Local work would be lost; the reason says how to proceed.
    Refused(String),
}

pub async fn pull(ctx: &Ctx, hub: &Hub, version: Option<u64>, force: bool) -> Result<Pulled> {
    let dir = &ctx.dir;
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
    let work = tempfile::Builder::new()
        .prefix(".porter-hub-pull-")
        .tempdir_in(dir)
        .context("creating a temp dir in the project")?;
    let archive = work.path().join("snapshot.tar.zst");
    let s3 = storage_client()?;
    if !ctx.json {
        eprintln!("downloading v{n}");
    }
    let got = download(&s3, url, &archive, MAX_SNAPSHOT).await?;
    if got != want {
        bail!("the snapshot for v{n} does not match its sha256 (got {got}, want {want})");
    }
    let unpacked = work.path().join("new");
    extract(&archive, &unpacked)?;
    swap(dir, &unpacked, &work.path().join("old"))?;

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
    SyncState::write(dir, hub, n)?;
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

/// Unpacks regular files and folders only, with relative, `..`-free paths, never a
/// top-level `media/` or dot-entry, and at most `MAX_UNPACKED` bytes.
pub fn extract(archive: &Path, into: &Path) -> Result<()> {
    let f = std::fs::File::open(archive)?;
    let mut ar = tar::Archive::new(zstd::Decoder::new(f)?);
    std::fs::create_dir_all(into)?;
    let mut total = 0u64;
    for entry in ar.entries().context("reading the snapshot")? {
        let mut e = entry.context("reading the snapshot")?;
        let path = e.path()?.into_owned();
        let shown = path.display().to_string();
        let mut comps = path.components().peekable();
        let Some(Component::Normal(first)) = comps.peek().copied() else {
            bail!("unsafe path in snapshot: {shown}");
        };
        let first = first.to_string_lossy();
        if first.starts_with('.') || first == "media" {
            bail!("unexpected entry in snapshot: {shown}");
        }
        if !path.components().all(|c| matches!(c, Component::Normal(_))) {
            bail!("unsafe path in snapshot: {shown}");
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

/// Moves each unpacked top-level entry into `dir`, the old one aside into `old`. Local
/// entries the snapshot lacks are kept, except SQLite sidecars of a replaced project.db.
fn swap(dir: &Path, new: &Path, old: &Path) -> Result<()> {
    std::fs::create_dir_all(old)?;
    let mut names: Vec<_> = std::fs::read_dir(new)?
        .flatten()
        .map(|e| e.file_name())
        .collect();
    if names.iter().any(|n| n == "project.db") {
        names.extend(["project.db-wal", "project.db-shm", "project.db-journal"].map(Into::into));
    }
    for name in names {
        let (src, dst) = (new.join(&name), dir.join(&name));
        if std::fs::symlink_metadata(&dst).is_ok() {
            std::fs::rename(&dst, old.join(&name))
                .with_context(|| format!("moving {} aside", dst.display()))?;
        }
        if src.exists() {
            std::fs::rename(&src, &dst).with_context(|| format!("writing {}", dst.display()))?;
        }
    }
    Ok(())
}
