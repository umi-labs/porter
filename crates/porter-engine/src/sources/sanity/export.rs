//! Reads a Sanity export into `raw/sanity/data.ndjson` (from the HTTP export API, a
//! `sanity dataset export` tar.gz, or a folder) and parses it.

use crate::http::{client, send_with_retry};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub(crate) const DATA_FILE: &str = "data.ndjson";

/// The parts of a `sanity-api` connector the fetch needs.
pub(crate) struct SanityApiCfg {
    pub project_id: String,
    pub dataset: String,
    pub api_version: String,
    /// Name of the env var holding the token (for error messages only).
    pub token_env: String,
}

pub(crate) struct Export {
    /// Every non-asset document, drafts included.
    pub docs: Vec<Value>,
    /// Asset documents keyed by `_id`.
    pub assets: HashMap<String, AssetDoc>,
}

#[derive(Clone, Debug)]
pub(crate) struct AssetDoc {
    pub id: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub filename: Option<String>,
    pub alt: Option<String>,
}

pub(crate) fn is_asset_type(t: &str) -> bool {
    t == "sanity.imageAsset" || t == "sanity.fileAsset"
}

/// Streams the export to `out/data.ndjson` via a temp file, so an interrupted stream leaves
/// no half file. `base` is `https://{project_id}.api.sanity.io` unless overridden.
pub(crate) async fn fetch_api(
    cfg: &SanityApiCfg,
    base: &str,
    token: &str,
    out: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    let url = format!(
        "{}/{}/data/export/{}",
        base.trim_end_matches('/'),
        cfg.api_version,
        cfg.dataset
    );
    let req = client("porter").get(&url).bearer_auth(token);
    let mut res = send_with_retry(req, 3).await?;
    let status = res.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!(
            "Sanity refused the token (HTTP {}); check that {} holds a read token (Viewer role) for project {}",
            status.as_u16(),
            cfg.token_env,
            cfg.project_id
        );
    }
    if !status.is_success() {
        bail!("Sanity export failed: HTTP {}", status.as_u16());
    }
    std::fs::create_dir_all(out)?;
    let tmp = out.join(format!("{DATA_FILE}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    while let Some(chunk) = res.chunk().await? {
        if cancel.is_cancelled() {
            drop(f);
            let _ = std::fs::remove_file(&tmp);
            bail!("cancelled while streaming the Sanity export");
        }
        f.write_all(&chunk)?;
    }
    f.flush()?;
    drop(f);
    std::fs::rename(&tmp, out.join(DATA_FILE))?;
    Ok(())
}

/// A tar.gz (first `*/data.ndjson`, or a top-level one) or a folder holding `data.ndjson`
/// becomes `out/data.ndjson`.
pub(crate) fn read_archive(path: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let tmp = out.join(format!("{DATA_FILE}.tmp"));
    let dest = out.join(DATA_FILE);
    if path.is_dir() {
        let direct = path.join(DATA_FILE);
        let src = if direct.is_file() {
            direct
        } else {
            std::fs::read_dir(path)?
                .filter_map(|e| e.ok())
                .map(|e| e.path().join(DATA_FILE))
                .find(|p| p.is_file())
                .ok_or_else(|| anyhow!("no {DATA_FILE} in {}", path.display()))?
        };
        std::fs::copy(&src, &tmp)?;
    } else {
        let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
        let mut found = false;
        for e in ar.entries().context("reading the export archive")? {
            let mut e = e?;
            let p = e.path()?.into_owned();
            if p.file_name().is_some_and(|n| n == DATA_FILE) && p.components().count() <= 2 {
                let mut w = std::fs::File::create(&tmp)?;
                std::io::copy(&mut e, &mut w)?;
                found = true;
                break;
            }
        }
        if !found {
            bail!("no {DATA_FILE} in {}", path.display());
        }
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Parses `out_dir/data.ndjson`.
pub(crate) fn load(out_dir: &Path) -> Result<Export> {
    let p = out_dir.join(DATA_FILE);
    let f = std::fs::File::open(&p).with_context(|| format!("opening {}", p.display()))?;
    let mut ex = Export {
        docs: vec![],
        assets: HashMap::new(),
    };
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line)
            .with_context(|| format!("{} line {}: not JSON", p.display(), i + 1))?;
        let ty = v.get("_type").and_then(Value::as_str).unwrap_or("");
        if is_asset_type(ty) {
            if let Some(a) = asset_doc(&v) {
                ex.assets.insert(a.id.clone(), a);
            }
        } else {
            ex.docs.push(v);
        }
    }
    Ok(ex)
}

fn asset_doc(v: &Value) -> Option<AssetDoc> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let dim = |k: &str| {
        v.pointer(&format!("/metadata/dimensions/{k}"))
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    Some(AssetDoc {
        id: s("_id")?,
        url: s("url")?,
        sha1: s("sha1hash").unwrap_or_default(),
        size: v.get("size").and_then(Value::as_u64).unwrap_or(0),
        mime: s("mimeType").unwrap_or_default(),
        width: dim("width"),
        height: dim("height"),
        filename: s("originalFilename"),
        alt: s("altText"),
    })
}
