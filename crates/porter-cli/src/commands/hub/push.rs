use super::{
    Ctx, DB_FILE, Hub, SyncState, check_leftovers, copy_db, hex, is_hash, media_path, progress,
    sha256_file, storage_client, upload,
};
use crate::output::print_json;
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use reqwest::{Method, StatusCode};
use serde_json::{Map, Value, json};
use sha2::Digest;
use std::collections::HashMap;
use std::path::Path;

/// Concurrent media PUTs.
const PARALLEL: usize = 8;

pub async fn run(ctx: &Ctx, hub: &Hub, message: Option<String>, force: bool) -> Result<i32> {
    let dir = &ctx.dir;
    if !dir.join("porter.toml").exists() {
        bail!("no porter.toml in {}", dir.display());
    }
    check_leftovers(dir)?;
    let summary = summary(dir);
    let Snapshot {
        file: snap,
        db_hash,
        toml_hash,
    } = snapshot(dir)?;
    let snap_hash = sha256_file(snap.path())?.unwrap_or_default();
    let media = media_hashes(dir)?;
    let ours: std::collections::HashSet<&str> = media.iter().map(String::as_str).collect();
    let s3 = storage_client()?;
    let mut base = SyncState::read(dir, hub).map(|s| s.version);
    for attempt in 0..2 {
        let retry = force && attempt == 0;
        let forced = force && attempt == 1;
        let (st, v) = hub
            .project(
                Method::POST,
                &["versions", "init"],
                Some(&json!({"base_version": base, "snapshot_sha256": snap_hash, "media_hashes": media})),
            )
            .await?;
        if st == StatusCode::CONFLICT {
            base = stale(&v, retry, forced)?;
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
                    .filter_map(Value::as_str)
                    // Only hashes we offered: never a path built from a value we didn't send.
                    .filter(|h| is_hash(h) && ours.contains(h))
                    .map(str::to_string)
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
            base = stale(&v, retry, forced)?;
            continue;
        }
        if st != StatusCode::CREATED {
            bail!("{}", super::hub_error(st, &v));
        }
        let number = v
            .get("number")
            .and_then(Value::as_u64)
            .context("the hub's commit response has no version number")?;
        SyncState::write(dir, hub, number, db_hash.clone(), toml_hash.clone())?;
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

/// A 409. Only "stale base" carries `latest` (null before the first version): with
/// `--force` (first try) retry on top of it, else refuse. Any other 409 (the project is
/// archived, a run is going) is the hub's own words; `--force` can't help with those.
fn stale(v: &Value, retry: bool, forced: bool) -> Result<Option<u64>> {
    let Some(latest) = v.get("latest") else {
        bail!("{}", super::hub_error(StatusCode::CONFLICT, v));
    };
    let latest = latest.as_u64();
    if retry {
        return Ok(latest);
    }
    if forced {
        bail!(
            "the hub moved on again while pushing (it now has version {}); retry",
            latest.unwrap_or(0)
        );
    }
    match latest {
        Some(n) => bail!("hub has version {n}; pull first or --force"),
        None => bail!("hub has no versions yet; push with --force"),
    }
}

/// What `porter --json status` reports (`status::report`), as the hub's worker stores it.
fn summary(dir: &Path) -> Value {
    if !dir.join(DB_FILE).exists() {
        return json!({});
    }
    super::super::open(dir)
        .and_then(|p| super::super::status::report(&p))
        .unwrap_or_else(|_| json!({}))
}

struct Snapshot {
    file: tempfile::NamedTempFile,
    /// `db_hash` and sha256 of exactly what was packed, for the sync file.
    db_hash: Option<String>,
    toml_hash: Option<String>,
}

/// Top-level entries the snapshot leaves out: dot-entries (`.porter-hub.json`, pull
/// staging), `media/` (uploaded by hash), `uploads/` (client dumps) and SQLite sidecars
/// (the db is packed as a consistent copy instead).
pub fn excluded(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    name.starts_with('.')
        || l == "media"
        || l == "uploads"
        || l.ends_with("-wal")
        || l.ends_with("-shm")
        || l.ends_with("-journal")
}

/// tar.zst of the project: project.db as a `VACUUM INTO` copy, porter.toml as the bytes
/// hashed, everything else walked; a symlink or special file at any depth is refused.
fn snapshot(dir: &Path) -> Result<Snapshot> {
    let file = tempfile::NamedTempFile::new().context("creating a temp file")?;
    let scratch = tempfile::tempdir().context("creating a temp dir")?;
    let mut tar = tar::Builder::new(zstd::Encoder::new(file.reopen()?, 3)?);
    tar.mode(tar::HeaderMode::Deterministic);
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !excluded(n))
        .collect();
    names.sort();
    let (mut db_hash, mut toml_hash) = (None, None);
    for name in names {
        let path = dir.join(&name);
        if name == DB_FILE {
            let copy = scratch.path().join(DB_FILE);
            db_hash = copy_db(dir, &copy)?;
            tar.append_path_with_name(&copy, DB_FILE)
                .context("packing project.db")?;
        } else if name == "porter.toml" {
            let bytes = std::fs::read(&path).context("reading porter.toml")?;
            toml_hash = Some(hex(&sha2::Sha256::digest(&bytes)));
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            tar.append_data(&mut h, "porter.toml", &bytes[..])
                .context("packing porter.toml")?;
        } else {
            add(&mut tar, &path, Path::new(&name))?;
        }
    }
    tar.into_inner()?.finish()?.sync_all()?;
    Ok(Snapshot {
        file,
        db_hash,
        toml_hash,
    })
}

/// Appends a regular file or a folder (recursively, sorted); refuses anything else.
fn add<W: std::io::Write>(tar: &mut tar::Builder<W>, path: &Path, name: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_file() {
        return tar
            .append_path_with_name(path, name)
            .with_context(|| format!("packing {}", path.display()));
    }
    if !meta.is_dir() {
        bail!(
            "{} is a symlink or special file; porter hub only pushes files and folders (move it out of the project)",
            path.display()
        );
    }
    tar.append_dir(name, path)
        .with_context(|| format!("packing {}", path.display()))?;
    let mut kids: Vec<_> = std::fs::read_dir(path)?
        .flatten()
        .map(|e| e.file_name())
        .collect();
    kids.sort();
    for k in kids {
        add(tar, &path.join(&k), &name.join(&k))?;
    }
    Ok(())
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
