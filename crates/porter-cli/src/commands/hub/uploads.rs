use super::{Ctx, Hub, clean, download, is_hash, storage_client};
use crate::output::print_json;
use anyhow::{Context, Result, bail};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::path::PathBuf;

const MAX_UPLOAD: u64 = 100 << 30;

pub async fn list(ctx: &Ctx, hub: &Hub) -> Result<i32> {
    let v = hub
        .project_ok(Method::GET, &["uploads"], None, StatusCode::OK)
        .await?;
    let rows = v.as_array().cloned().unwrap_or_default();
    if ctx.json {
        print_json(&json!({"v": 1, "event": "hub-uploads", "uploads": rows}));
        return Ok(0);
    }
    if rows.is_empty() {
        eprintln!("no uploads");
    }
    for r in &rows {
        let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("-");
        println!(
            "{}  {:<10} {:<13} {:>12}  {}",
            s("id"),
            s("state"),
            s("purpose"),
            r.get("size").and_then(Value::as_u64).unwrap_or(0),
            clean(s("filename"))
        );
    }
    Ok(0)
}

/// Downloads into `out` (default `<project>/uploads/<filename>`), verified, never overwriting.
pub async fn get(ctx: &Ctx, hub: &Hub, id: &str, out: Option<PathBuf>) -> Result<i32> {
    let v = hub
        .project_ok(
            Method::GET,
            &["uploads", id, "download"],
            None,
            StatusCode::OK,
        )
        .await?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
    let want = s("sha256");
    if !is_hash(want) {
        bail!("the hub sent no sha256 for upload {id}");
    }
    let out = out.unwrap_or_else(|| ctx.dir.join("uploads").join(sanitize(s("filename"), id)));
    if std::fs::symlink_metadata(&out).is_ok() {
        bail!(
            "{} already exists; pass --out to choose another path",
            out.display()
        );
    }
    let parent = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent).with_context(|| format!("creating {}", parent.display()))?;
    let tmp = tempfile::Builder::new()
        .prefix(".porter-dl-")
        .tempfile_in(&parent)?
        .into_temp_path();
    std::fs::remove_file(&tmp)?;
    if !ctx.json {
        eprintln!(
            "downloading {} ({} bytes)",
            clean(s("filename")),
            v.get("size").and_then(Value::as_u64).unwrap_or(0)
        );
    }
    let got = download(&storage_client()?, s("url"), &tmp, MAX_UPLOAD).await?;
    if got != want {
        bail!("upload {id} downloaded with the wrong sha256 (got {got}, want {want})");
    }
    // Hard link, then drop the temp name: fails rather than overwrite a file that appeared meanwhile.
    // No hard links on this file system (exFAT, some mounts): rename if still absent.
    if let Err(e) = std::fs::hard_link(&tmp, &out) {
        if std::fs::symlink_metadata(&out).is_ok() {
            return Err(e).with_context(|| format!("writing {}", out.display()));
        }
        std::fs::rename(&tmp, &out).with_context(|| format!("writing {}", out.display()))?;
    }
    drop(tmp);
    if ctx.json {
        print_json(
            &json!({"v": 1, "event": "hub-upload-saved", "id": id, "path": out, "sha256": got}),
        );
    } else {
        eprintln!("saved {}", out.display());
    }
    Ok(0)
}

/// A safe file name: no separators, control characters or leading dots; `id` if nothing is left.
fn sanitize(name: &str, id: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() {
        id.to_string()
    } else {
        s.chars().take(200).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize("../../etc/passwd", "u1"), "_.._etc_passwd");
        assert_eq!(sanitize("..", "u1"), "u1");
        assert_eq!(sanitize("dump.sql.gz", "u1"), "dump.sql.gz");
        assert_eq!(sanitize("a\nb", "u1"), "a_b");
    }
}
