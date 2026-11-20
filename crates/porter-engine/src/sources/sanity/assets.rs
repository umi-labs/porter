//! Resumable, verified download of a Sanity export's assets into the media store.

use super::export::{AssetDoc, Export};
use super::shape::asset_ref;
use crate::config::SanityAssets;
use crate::connectors::fetch::{DEFAULT_MEDIA_CAP, Download, download_capped, media_client};
use crate::stage::StageCtx;
use anyhow::Result;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, Media, Severity, SourceRef};
use reqwest::Client;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const CONCURRENCY: usize = 4;
const INTERRUPTED: &str = "interrupted before download";

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FetchSummary {
    pub fetched: u64,
    pub skipped: u64,
    pub failed: u64,
    /// Assets no document references (whether or not they were fetched).
    pub unreferenced: u64,
    pub unreferenced_bytes: u64,
}

enum Outcome {
    Fetched,
    Skipped,
    Failed,
    Interrupted,
}

fn collect_refs(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if let Some(r) = asset_ref(v) {
                out.insert(r.to_string());
            }
            m.values().for_each(|c| collect_refs(c, out));
        }
        Value::Array(a) => a.iter().for_each(|c| collect_refs(c, out)),
        _ => {}
    }
}

/// Asset `_id`s used anywhere in a non-asset document (drafts included).
pub(crate) fn referenced(export: &Export) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    export.docs.iter().for_each(|d| collect_refs(d, &mut out));
    out
}

/// The asset's CDN URL without its query string.
pub(crate) fn cdn_url(asset: &AssetDoc) -> String {
    asset.url.split(['?', '#']).next().unwrap_or("").to_string()
}

/// `originalFilename`, else `<sha1>.<ext>` (Payload guesses the mime type from the extension).
fn file_name(a: &AssetDoc, url: &str) -> String {
    if let Some(n) = a.filename.as_ref().filter(|n| !n.is_empty()) {
        return n.clone();
    }
    let last = url.rsplit('/').next().unwrap_or("");
    let ext = match last.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && e.len() <= 5 => e.to_string(),
        _ => a.mime.rsplit('/').next().unwrap_or("bin").to_string(),
    };
    let stem = if a.sha1.is_empty() { &a.id } else { &a.sha1 };
    format!("{stem}.{ext}")
}

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) async fn fetch(
    ctx: &StageCtx,
    export: &Export,
    which: SanityAssets,
) -> Result<FetchSummary> {
    let used = referenced(export);
    let mut sum = FetchSummary::default();
    let mut todo: Vec<&AssetDoc> = vec![];
    let mut all: Vec<&AssetDoc> = export.assets.values().collect();
    all.sort_by(|a, b| a.id.cmp(&b.id));
    for a in all {
        if used.contains(&a.id) {
            todo.push(a);
        } else {
            sum.unreferenced += 1;
            sum.unreferenced_bytes += a.size;
            if which == SanityAssets::All {
                todo.push(a);
            }
        }
    }
    let total = todo.len() as u64;
    let client = media_client("porter")?;
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set = JoinSet::new();
    for a in todo {
        let (ctx, client, sem, a) = (ctx.clone(), client.clone(), sem.clone(), a.clone());
        set.spawn(async move {
            let _permit = sem.acquire_owned().await?;
            one(&ctx, &client, &a).await
        });
    }
    let mut done = 0;
    while let Some(r) = set.join_next().await {
        match r?? {
            Outcome::Fetched => sum.fetched += 1,
            Outcome::Skipped => sum.skipped += 1,
            Outcome::Failed => sum.failed += 1,
            Outcome::Interrupted => {}
        }
        done += 1;
        ctx.progress(done, Some(total), "assets");
    }
    Ok(sum)
}

async fn one(ctx: &StageCtx, client: &Client, a: &AssetDoc) -> Result<Outcome> {
    let store = ctx.project.store();
    let src = SourceRef {
        connector: "sanity-assets".into(),
        source_id: a.id.clone(),
        url: None,
    };
    let item = ItemId::new(ItemKind::Media, &src);
    let status = |s| ctx.item_status(item, ItemKind::Media, s);
    let fail = |sev: Severity, msg: String, why: &str| -> Result<Outcome> {
        ctx.issue(
            IssueKind::MissingMedia,
            sev,
            Some(item),
            msg,
            json!({"asset": a.id}),
        )?;
        ctx.item_status(
            item,
            ItemKind::Media,
            ItemStatus::Failed { error: why.into() },
        )?;
        Ok(Outcome::Failed)
    };
    let interrupted = || -> Result<Outcome> {
        ctx.item_status(
            item,
            ItemKind::Media,
            ItemStatus::Partial {
                reason: INTERRUPTED.into(),
            },
        )?;
        Ok(Outcome::Interrupted)
    };
    if ctx.cancel.is_cancelled() {
        return interrupted();
    }
    let url = cdn_url(a);
    if !ctx.refresh && store.media_by_url(&url)?.is_some() {
        status(ItemStatus::Skipped {
            reason: "already downloaded".into(),
        })?;
        return Ok(Outcome::Skipped);
    }
    let bytes = match download_capped(client, &url, DEFAULT_MEDIA_CAP, &ctx.cancel).await {
        Ok(Download::Ok(b)) => b,
        Ok(Download::Cancelled) => return interrupted(),
        Ok(Download::OverCap(n)) => {
            return fail(
                Severity::Warning,
                format!("{}: file is over the size cap ({n} bytes)", a.id),
                "over the size cap",
            );
        }
        Ok(Download::Status(s)) => {
            return fail(
                Severity::Warning,
                format!("{}: download failed (HTTP {s})", a.id),
                &format!("HTTP {s}"),
            );
        }
        Err(e) => {
            return fail(
                Severity::Warning,
                format!("{}: download failed: {e}", a.id),
                "download failed",
            );
        }
    };
    if !a.sha1.is_empty() && sha1_hex(&bytes) != a.sha1 {
        return fail(
            Severity::Error,
            format!("{}: downloaded file doesn't match Sanity's SHA-1", a.id),
            "SHA-1 mismatch",
        );
    }
    let name = file_name(a, &url);
    let h = store.put_media_bytes(&bytes, &url, &name)?;
    if let Some(mut m) = store.get_item::<Media>(h.id)? {
        let before = m.clone();
        m.filename = name;
        if !a.mime.is_empty() {
            m.mime = a.mime.clone();
        }
        m.width = a.width.or(m.width);
        m.height = a.height.or(m.height);
        m.alt = a.alt.clone().or(m.alt);
        if m != before {
            let src = SourceRef {
                connector: "media".into(),
                source_id: m.hash.clone(),
                url: None,
            };
            store.put_item(ItemKind::Media, h.id, &src, &m)?;
        }
    }
    status(ItemStatus::Done)?;
    Ok(Outcome::Fetched)
}
