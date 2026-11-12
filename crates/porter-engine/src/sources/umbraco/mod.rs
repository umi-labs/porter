//! Umbraco JSON export source. The connector copies the export files into `raw/umbraco/` and
//! writes a `RawItem` row per node (connector `umbraco-json`); normalise turns each node into
//! a `Document` (connector `umbraco`, id `node:<id>`), each `_variants`/`variants` entry into a
//! translation, and downloads/resolves media referenced by pickers and rich text.

pub mod normalise;

use crate::config::ConnectorConfig;
use crate::connectors::RawItem;
use crate::connectors::fetch::{DEFAULT_MEDIA_CAP, Download, download_capped, media_client};
use crate::stage::{Capabilities, Connector, Feature, Source, StageCtx};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, Severity, SourceRef};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet};
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::LazyLock;

/// Connector name of the raw rows.
const NAME: &str = "umbraco-json";
/// Connector name of normalised items.
pub(crate) const CANON: &str = "umbraco";
/// Largest export file read.
const MAX_FILE: u64 = 1 << 30;
const USER_AGENT: &str = "porter/2.0 (+https://github.com/umi-labs/porter)";
/// Values nested deeper than this stay as JSON text.
pub(crate) const MAX_DEPTH: usize = 64;

pub struct UmbracoSource;

impl Source for UmbracoSource {
    fn connectors(&self, cfg: &crate::config::ProjectConfig) -> Result<Vec<Box<dyn Connector>>> {
        let mut paths = vec![];
        for c in &cfg.connectors {
            match c {
                ConnectorConfig::UmbracoJson { paths: p } => paths.extend(p.iter().cloned()),
                _ => bail!("only \"umbraco-json\" connectors apply to an umbraco source"),
            }
        }
        if paths.is_empty() {
            bail!(
                "an umbraco source needs a [[connectors]] entry: kind = \"umbraco-json\", paths = [...]"
            );
        }
        Ok(vec![Box::new(UmbracoJsonConnector { paths })])
    }

    fn normalise(&self, ctx: &StageCtx) -> Result<()> {
        normalise::run(ctx)
    }
}

// ---- reading -------------------------------------------------------------------------

/// One node as exported, in file order.
pub(crate) struct Entry {
    /// Source id: the node's `id`, else `key`, else `udi`, else its position.
    pub sid: String,
    /// Which export file it came from.
    pub file: usize,
    pub node: Result<Map<String, Value>, String>,
    /// The id was seen before; this entry's `sid` carries a `~n` suffix.
    pub dup: bool,
}

/// The nodes in an export: the items of any well-known content array, else the root array,
/// else the root object itself.
fn nodes_of(root: Value) -> Vec<Value> {
    const ARRAYS: [&str; 6] = [
        "accommodationPages",
        "contentPages",
        "content",
        "items",
        "nodes",
        "pages",
    ];
    match root {
        Value::Array(a) => a,
        Value::Object(mut m) => {
            let found: Vec<Value> = ARRAYS
                .iter()
                .filter_map(|k| match m.remove(*k) {
                    Some(Value::Array(a)) => Some(a),
                    _ => None,
                })
                .flatten()
                .collect();
            if found.is_empty() {
                vec![Value::Object(m)]
            } else {
                found
            }
        }
        other => vec![other],
    }
}

pub(crate) fn read_file(path: &std::path::Path) -> Result<Vec<Value>> {
    let len = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    if len > MAX_FILE {
        bail!(
            "{} is {len} bytes, over the 1 GB limit per file; split the export",
            path.display()
        );
    }
    let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    // ponytail: parses a file whole (<= 1 GB); a streaming array reader if exports outgrow that.
    let root: Value = serde_json::from_reader(BufReader::with_capacity(256 * 1024, f))
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    Ok(nodes_of(root))
}

pub(crate) fn entries(files: Vec<Vec<Value>>) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for (file, nodes) in files.into_iter().enumerate() {
        for v in nodes {
            let ordinal = out.len() + 1;
            let (sid, node) = match v {
                Value::Object(m) => (
                    sid_of(&m).unwrap_or_else(|| format!("item-{ordinal}")),
                    Ok(m),
                ),
                _ => (
                    format!("item-{ordinal}"),
                    Err(format!("item {ordinal} is not an object")),
                ),
            };
            let dup = !seen.insert(sid.clone());
            let sid = if dup { format!("{sid}~{ordinal}") } else { sid };
            out.push(Entry {
                sid,
                file,
                node,
                dup,
            });
        }
    }
    out
}

fn id_str(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn sid_of(m: &Map<String, Value>) -> Option<String> {
    ["id", "key", "udi"].iter().find_map(|k| id_str(m.get(*k)))
}

/// First non-empty string among `keys`.
pub(crate) fn text<'a>(m: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|k| m.get(*k)?.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
}

pub(crate) fn alias_of(m: &Map<String, Value>) -> String {
    text(m, &["contentTypeAlias", "contentType"])
        .or_else(|| text(m.get("contentType")?.as_object()?, &["alias"]))
        .unwrap_or("unknown")
        .to_string()
}

pub(crate) fn url_of(m: &Map<String, Value>) -> Option<String> {
    text(m, &["url"])
        .or_else(|| text(m.get("route")?.as_object()?, &["path", "url"]))
        .map(str::to_string)
}

fn raw_ref(sid: &str, url: Option<String>) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: NAME.into(),
        source_id: sid.into(),
        url,
    };
    (ItemId::new(ItemKind::Document, &src), src)
}

/// `https://<host>/path` for a URL that has a host, or a relative one when `hosts` has one.
pub(crate) fn absolute(hosts: &[String], url: &str) -> Option<String> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if let Ok(u) = url::Url::parse(url) {
        return matches!(u.scheme(), "http" | "https").then(|| u.to_string());
    }
    let base = url::Url::parse(&format!("https://{}/", hosts.first()?)).ok()?;
    Some(base.join(url).ok()?.to_string())
}

/// Hosts of the absolute node URLs (first is the main one).
pub(crate) fn hosts_of(entries: &[Entry]) -> Vec<String> {
    let mut hosts: Vec<String> = vec![];
    for e in entries {
        let Ok(m) = &e.node else { continue };
        for u in std::iter::once(m).chain(variants(m)).filter_map(url_of) {
            if let Some(h) = url::Url::parse(&u)
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https"))
                .and_then(|u| u.host_str().map(str::to_string))
                && !hosts.contains(&h)
            {
                hosts.push(h);
            }
        }
    }
    hosts
}

const CULTURE: [&str; 4] = ["culture", "language", "cultureCode", "locale"];

/// A node's variant objects (`_variants`, else `variants`), the legacy
/// `{language, properties: {...}}` wrapper unwrapped.
pub(crate) fn variants(m: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    ["_variants", "variants"]
        .iter()
        .filter_map(|k| m.get(*k)?.as_array())
        .find(|a| !a.is_empty())
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|v| match v.get("properties").and_then(Value::as_object) {
            Some(inner) if CULTURE.iter().any(|k| inner.contains_key(*k)) => inner,
            _ => v,
        })
        .collect()
}

pub(crate) fn culture_of(v: &Map<String, Value>) -> Option<String> {
    text(v, &CULTURE).map(str::to_string)
}

// ---- media references ----------------------------------------------------------------

pub(crate) struct MediaRef {
    pub url: String,
    pub name: Option<String>,
    pub alt: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl MediaRef {
    fn bare(url: &str) -> MediaRef {
        MediaRef {
            url: url.trim().to_string(),
            name: None,
            alt: None,
            width: None,
            height: None,
        }
    }
}

/// A media-picker object: a URL plus one of the keys only media carries.
pub(crate) fn media_ref(m: &Map<String, Value>) -> Option<MediaRef> {
    let url = text(m, &["url", "mediaUrl", "src"])?;
    let typed = [
        "mediaType",
        "mediaUrl",
        "mediaId",
        "extension",
        "focalPoint",
        "crops",
    ]
    .iter()
    .any(|k| m.contains_key(*k))
        || text(m, &["contentType"]).is_some_and(|t| {
            ["image", "file", "media", "folder"].contains(&t.to_lowercase().as_str())
        });
    if !typed {
        return None;
    }
    let dim = |k: &str| m.get(k)?.as_u64().and_then(|n| u32::try_from(n).ok());
    Some(MediaRef {
        url: url.to_string(),
        name: text(m, &["name", "mediaName"]).map(str::to_string),
        alt: text(m, &["alt", "altText"]).map(str::to_string),
        width: dim("width"),
        height: dim("height"),
    })
}

/// A bare string that is a media file: under `/media/`, or an http(s) URL to a known file type.
pub(crate) fn looks_media(s: &str) -> bool {
    const EXT: [&str; 17] = [
        "jpg", "jpeg", "png", "gif", "webp", "svg", "avif", "pdf", "mp4", "mp3", "webm", "doc",
        "docx", "xls", "xlsx", "zip", "ico",
    ];
    let s = s.trim();
    if s.is_empty() || s.contains(char::is_whitespace) || s.contains('<') {
        return false;
    }
    if s.starts_with("/media/") {
        return true;
    }
    let Ok(u) = url::Url::parse(s) else {
        return false;
    };
    matches!(u.scheme(), "http" | "https")
        && u.path()
            .rsplit_once('.')
            .is_some_and(|(_, e)| EXT.contains(&e.to_lowercase().as_str()))
}

/// Rich text: has a `<` and a closing tag.
pub(crate) fn looks_html(s: &str) -> bool {
    static CLOSE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"</[A-Za-z][^>]*>").expect("valid regex"));
    s.contains('<') && CLOSE.is_match(s)
}

/// Every media reference in `v`: picker objects, bare media URLs, and `src`/`href` values in
/// rich text.
pub(crate) fn walk_media(v: &Value, out: &mut Vec<MediaRef>, depth: usize) {
    static ATTR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:src|href)\s*=\s*["']([^"']+)["']"#).expect("valid regex")
    });
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Value::String(s) if looks_media(s) => out.push(MediaRef::bare(s)),
        Value::String(s) if looks_html(s) => out.extend(
            ATTR.captures_iter(s)
                .filter(|c| looks_media(&c[1]))
                .map(|c| MediaRef::bare(&c[1])),
        ),
        Value::Array(a) => a.iter().for_each(|x| walk_media(x, out, depth + 1)),
        Value::Object(m) => match media_ref(m) {
            Some(r) => out.push(r),
            None => m.values().for_each(|x| walk_media(x, out, depth + 1)),
        },
        _ => {}
    }
}

// ---- connector -----------------------------------------------------------------------

pub struct UmbracoJsonConnector {
    paths: Vec<PathBuf>,
}

impl UmbracoJsonConnector {
    fn load(&self, ctx: &StageCtx) -> Result<Vec<Entry>> {
        let files = self
            .paths
            .iter()
            .map(|p| read_file(&ctx.project.dir().join(p)))
            .collect::<Result<Vec<_>>>()?;
        Ok(entries(files))
    }
}

fn row(e: &Entry, raw_path: Option<String>) -> RawItem {
    match &e.node {
        Ok(m) => RawItem {
            label: alias_of(m),
            title: text(m, &["name"]).map(str::to_string),
            status: text(m, &["status", "state"]).map(str::to_string),
            url: url_of(m),
            raw_path,
        },
        Err(_) => RawItem {
            label: "unparsed".into(),
            title: None,
            status: None,
            url: None,
            raw_path: None,
        },
    }
}

/// Writes the RawItem row; returns the item and the status for a node that needs none beyond
/// parse success (duplicates are partial, non-objects failed).
fn record(ctx: &StageCtx, e: &Entry, raw_path: Option<String>) -> Result<(ItemId, ItemStatus)> {
    let url = e.node.as_ref().ok().and_then(url_of);
    let (id, src) = raw_ref(&e.sid, url);
    ctx.project
        .store()
        .put_item(ItemKind::Document, id, &src, &row(e, raw_path))?;
    let status = match &e.node {
        Err(msg) => {
            ctx.issue(
                IssueKind::UnmappedField,
                Severity::Error,
                Some(id),
                format!("{msg}; skipped"),
                json!({ "item": e.sid }),
            )?;
            ItemStatus::Failed { error: msg.clone() }
        }
        Ok(_) if e.dup => {
            ctx.issue(
                IssueKind::UnmappedField,
                Severity::Warning,
                Some(id),
                format!(
                    "node id appears more than once; this copy is kept as {}",
                    e.sid
                ),
                json!({ "item": e.sid }),
            )?;
            ItemStatus::Partial {
                reason: "duplicate node id".into(),
            }
        }
        Ok(_) => ItemStatus::Done,
    };
    Ok((id, status))
}

#[async_trait]
impl Connector for UmbracoJsonConnector {
    fn name(&self) -> &str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            reads: [Feature::Media, Feature::Locales, Feature::Drafts]
                .into_iter()
                .collect(),
            writes: Default::default(),
        }
    }

    async fn analyse(&self, ctx: &StageCtx) -> Result<()> {
        let entries = self.load(ctx)?;
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for e in &entries {
            if let Ok(m) = &e.node {
                *counts.entry(alias_of(m)).or_default() += 1;
            }
            let (id, status) = record(ctx, e, None)?;
            ctx.item_status(id, ItemKind::Document, status)?;
        }
        let dir = ctx.project.raw_dir(CANON);
        std::fs::create_dir_all(&dir)?;
        let analysis = json!({ "items_by_type": counts, "files": self.paths.len() });
        std::fs::write(
            dir.join("analysis.json"),
            serde_json::to_vec_pretty(&analysis)?,
        )?;
        Ok(())
    }

    async fn extract(&self, ctx: &StageCtx) -> Result<()> {
        let dir = ctx.project.raw_dir(CANON);
        std::fs::create_dir_all(&dir)?;
        // Stale copies from an earlier configuration would be read by normalise.
        for f in exports(&dir)? {
            std::fs::remove_file(f)?;
        }
        let entries = self.load(ctx)?;
        for (i, p) in self.paths.iter().enumerate() {
            let from = ctx.project.dir().join(p);
            std::fs::copy(&from, dir.join(format!("export-{i}.json")))
                .with_context(|| format!("copying {}", from.display()))?;
        }
        let hosts = hosts_of(&entries);
        let client = media_client(USER_AGENT)?;
        let total = entries.len() as u64;
        let mut iter = entries.iter().enumerate();
        while let Some((n, e)) = iter.next() {
            if ctx.cancel.is_cancelled() {
                // Resumable: this node and the rest stay unfinished.
                for (_, e) in std::iter::once((n, e)).chain(iter) {
                    let (id, _) = raw_ref(&e.sid, None);
                    let reason = "interrupted before download".to_string();
                    ctx.item_status(id, ItemKind::Document, ItemStatus::Partial { reason })?;
                }
                return Ok(());
            }
            if n % 200 == 0 {
                ctx.progress(n as u64, Some(total), "nodes");
            }
            let (id, mut status) = record(ctx, e, Some(format!("export-{}.json", e.file)))?;
            if let Ok(m) = &e.node {
                let mut refs = vec![];
                m.values().for_each(|v| walk_media(v, &mut refs, 0));
                let mut done = HashSet::new();
                for r in refs {
                    let Some(url) = absolute(&hosts, &r.url).filter(|u| done.insert(u.clone()))
                    else {
                        continue;
                    };
                    if let Some(why) = fetch(ctx, &client, &url).await? {
                        ctx.issue(
                            IssueKind::MissingMedia,
                            Severity::Warning,
                            Some(id),
                            format!("could not download {url}: {why}"),
                            json!({ "url": url }),
                        )?;
                        if status == ItemStatus::Done {
                            status = ItemStatus::Partial {
                                reason: format!("media download failed: {why}"),
                            };
                        }
                    }
                }
            }
            ctx.item_status(id, ItemKind::Document, status)?;
        }
        Ok(())
    }
}

/// Downloads `url` into the media store unless it is already there. `Some(reason)` on failure.
// ponytail: sequential downloads; a bounded JoinSet if big libraries make this slow.
async fn fetch(ctx: &StageCtx, client: &reqwest::Client, url: &str) -> Result<Option<String>> {
    let store = ctx.project.store();
    if !ctx.refresh && store.media_by_url(url)?.is_some() {
        return Ok(None);
    }
    Ok(
        match download_capped(client, url, DEFAULT_MEDIA_CAP, &ctx.cancel).await {
            Ok(Download::Ok(bytes)) => {
                let name = url
                    .split(['?', '#'])
                    .next()
                    .and_then(|u| u.rsplit('/').next())
                    .filter(|n| !n.is_empty())
                    .unwrap_or("file");
                store.put_media_bytes(&bytes, url, name)?;
                None
            }
            Ok(Download::OverCap(n)) => Some(format!("file over size cap ({n} bytes)")),
            Ok(Download::Status(c)) => Some(format!("HTTP {c}")),
            Ok(Download::Cancelled) => Some("interrupted".into()),
            Err(e) => Some(format!("{e:#}")),
        },
    )
}

/// `raw/umbraco/export-N.json`, in N order.
pub(crate) fn exports(dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut v: Vec<(usize, PathBuf)> = vec![];
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        let n = p.file_name().and_then(|n| n.to_str()).and_then(|n| {
            n.strip_prefix("export-")?
                .strip_suffix(".json")?
                .parse()
                .ok()
        });
        if let Some(n) = n {
            v.push((n, p));
        }
    }
    v.sort();
    Ok(v.into_iter().map(|(_, p)| p).collect())
}
