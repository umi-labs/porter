use super::{
    Ctx, Hub, SyncState, is_hash, media_path, progress, sha256_file, storage_client, upload,
};
use crate::output::print_json;
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use reqwest::{Method, StatusCode};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::Path;

/// Concurrent media PUTs.
const PARALLEL: usize = 8;

pub async fn run(ctx: &Ctx, hub: &Hub, message: Option<String>, force: bool) -> Result<i32> {
    let dir = &ctx.dir;
    if !dir.join("porter.toml").exists() {
        bail!("no porter.toml in {}", dir.display());
    }
    let summary = summary(dir);
    let snap = snapshot(dir)?;
    let snap_hash = sha256_file(snap.path())?.unwrap_or_default();
    let media = media_hashes(dir)?;
    let s3 = storage_client()?;
    let mut base = SyncState::read(dir, hub).map(|s| s.version);
    for attempt in 0..2 {
        let retry = force && attempt == 0;
        let (st, v) = hub
            .project(
                Method::POST,
                &["versions", "init"],
                Some(&json!({"base_version": base, "snapshot_sha256": snap_hash, "media_hashes": media})),
            )
            .await?;
        if st == StatusCode::CONFLICT {
            base = stale(&v, retry)?;
            continue;
        }
        if st != StatusCode::OK {
            bail!("{}", super::hub_error(st, &v));
        }
        let field = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let upload_id = field("upload_id");
        let missing: Vec<String> = v
            .get("missing")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|h| h.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        if !ctx.json {
            let kb = std::fs::metadata(snap.path())
                .map(|m| m.len() / 1024)
                .unwrap_or(0);
            eprintln!("uploading snapshot ({kb} KB)");
        }
        upload(&s3, &field("snapshot_put_url"), snap.path(), &snap_hash).await?;
        progress(ctx.json, "snapshot", 1, 1);

        let total = missing.len() as u64;
        let mut done = 0u64;
        for batch in missing.chunks(1000) {
            let v = hub
                .project_ok(
                    Method::POST,
                    &["versions", "uploads", &upload_id, "media-urls"],
                    Some(&json!({ "hashes": batch })),
                    StatusCode::OK,
                )
                .await?;
            let urls: HashMap<String, String> =
                serde_json::from_value(v.get("urls").cloned().unwrap_or_default())
                    .context("the hub's media-urls response")?;
            let mut puts = futures::stream::iter(batch.iter().map(|h| {
                let (s3, url, path) = (&s3, urls.get(h).cloned(), media_path(dir, h));
                async move {
                    match url {
                        Some(u) => upload(s3, &u, &path, h).await,
                        None => bail!("the hub sent no upload URL for media {h}"),
                    }
                }
            }))
            .buffer_unordered(PARALLEL);
            while let Some(r) = puts.next().await {
                r?;
                done += 1;
                progress(ctx.json, "media", done, total);
            }
        }

        let mut body = Map::new();
        body.insert("upload_id".into(), json!(upload_id));
        body.insert("base_version".into(), json!(base));
        if let Some(m) = &message {
            body.insert("message".into(), json!(m));
        }
        body.insert("porter_version".into(), json!(env!("CARGO_PKG_VERSION")));
        body.insert("summary".into(), summary.clone());
        let (st, v) = hub
            .project(
                Method::POST,
                &["versions", "commit"],
                Some(&Value::Object(body)),
            )
            .await?;
        if st == StatusCode::CONFLICT {
            base = stale(&v, retry)?;
            continue;
        }
        if st != StatusCode::CREATED {
            bail!("{}", super::hub_error(st, &v));
        }
        let number = v
            .get("number")
            .and_then(Value::as_u64)
            .context("the hub's commit response has no version number")?;
        SyncState::write(dir, hub, number)?;
        if ctx.json {
            print_json(&json!({
                "v": 1, "event": "hub-pushed", "version": number, "media_uploaded": total,
            }));
        } else {
            eprintln!("pushed v{number} ({total} new media)");
        }
        return Ok(0);
    }
    bail!("the hub kept moving on; pull and push again")
}

/// A 409: with `--force` (first try) retry on top of the hub's latest, else refuse.
fn stale(v: &Value, retry: bool) -> Result<Option<u64>> {
    let latest = v.get("latest").and_then(Value::as_u64);
    if retry {
        return Ok(latest);
    }
    match latest {
        Some(n) => bail!("hub has version {n}; pull first or --force"),
        None => bail!("hub has no versions yet; push with --force"),
    }
}

/// The same shape as `porter --json status` (what the hub's worker stores).
fn summary(dir: &Path) -> Value {
    if !dir.join("project.db").exists() {
        return json!({});
    }
    match super::super::open(dir).and_then(|p| p.store().overview()) {
        Ok(o) => json!({
            "stages": o["stages"], "open_issues": o["open_issues"], "last_runs": o["last_runs"],
        }),
        Err(_) => json!({}),
    }
}

/// Every top-level entry except `media/`, `.porter-hub.json` and dot-entries, as tar.zst.
fn snapshot(dir: &Path) -> Result<tempfile::NamedTempFile> {
    let tmp = tempfile::NamedTempFile::new().context("creating a temp file")?;
    let enc = zstd::Encoder::new(tmp.reopen()?, 3)?;
    let mut tar = tar::Builder::new(enc);
    tar.follow_symlinks(false);
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.file_name())
        .filter(|n| {
            let n = n.to_string_lossy();
            !n.starts_with('.') && n != "media"
        })
        .collect();
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            tar.append_dir_all(&name, &path)
        } else if meta.is_file() {
            tar.append_path_with_name(&path, &name)
        } else {
            eprintln!(
                "warning: skipping {} (not a file or folder)",
                path.display()
            );
            continue;
        }
        .with_context(|| format!("packing {}", path.display()))?;
    }
    tar.into_inner()?.finish()?.sync_all()?;
    Ok(tmp)
}

/// Hashes of `media/<h0h1>/<h2h3>/<sha256>` files.
fn media_hashes(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let root = dir.join("media");
    let Ok(level1) = std::fs::read_dir(&root) else {
        return Ok(out);
    };
    for a in level1.flatten() {
        let Ok(level2) = std::fs::read_dir(a.path()) else {
            continue;
        };
        for b in level2.flatten() {
            let Ok(files) = std::fs::read_dir(b.path()) else {
                continue;
            };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                if is_hash(&name)
                    && f.file_type().is_ok_and(|t| t.is_file())
                    && media_path(dir, &name) == f.path()
                {
                    out.push(name);
                }
            }
        }
    }
    out.sort();
    Ok(out)
}
