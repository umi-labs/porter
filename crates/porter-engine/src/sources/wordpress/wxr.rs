//! Streaming WordPress export (WXR) reader. Never holds more than one item in memory.
use crate::config::ConnectorConfig;
use crate::connectors::RawItem;
use crate::connectors::fetch::{DEFAULT_MEDIA_CAP, Download, download_capped, media_client};
use crate::stage::{Capabilities, Connector, Feature, StageCtx};
use anyhow::{Context, Result};
use async_trait::async_trait;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, Severity, SourceRef};
use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const NAME: &str = "wxr";
/// Largest item (text and attribute bytes) kept in memory; bigger ones fail.
const MAX_ITEM_BYTES: usize = 64 * 1024 * 1024;
const USER_AGENT: &str = "porter/2.0 (+https://github.com/umi-labs/porter)";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct WxrItem {
    pub post_id: u64,
    pub post_type: String,
    pub status: String,
    pub title: String,
    pub link: String,
    pub slug: String,
    pub parent: u64,
    pub menu_order: i64,
    pub date_gmt: String,
    pub modified_gmt: String,
    pub author: String,
    pub content: String,
    pub excerpt: String,
    /// `(domain, nicename, label)`
    pub categories: Vec<(String, String, String)>,
    pub meta: Vec<(String, String)>,
    pub attachment_url: Option<String>,
    /// Invalid UTF-8 was replaced while decoding this item.
    pub invalid_utf8: bool,
}

#[derive(Debug)]
pub struct WxrItemError {
    /// Known when the id parsed before the failure.
    pub post_id: Option<u64>,
    /// 1-based position of the item in the file.
    pub ordinal: usize,
    pub message: String,
}

impl std::fmt::Display for WxrItemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "item {}: {}", self.ordinal, self.message)
    }
}
impl std::error::Error for WxrItemError {}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct WxrAuthor {
    pub login: String,
    pub email: String,
    pub display_name: String,
    pub first_name: String,
    pub last_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct WxrTerm {
    /// Taxonomy: `category`, `post_tag` or a custom one.
    pub domain: String,
    pub nicename: String,
    pub name: String,
    pub parent: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct WxrHeader {
    pub title: String,
    pub link: String,
    pub base_site_url: String,
    pub authors: Vec<WxrAuthor>,
    pub terms: Vec<WxrTerm>,
}

/// One element with its subtree; only ever built for a single item or header block.
#[derive(Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn text_of(&self, name: &str) -> String {
        self.children
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.text.clone())
            .unwrap_or_default()
    }
    /// Moves the text out (no copy) for the fields that can be huge.
    fn take_text(&mut self, name: &str) -> String {
        self.children
            .iter_mut()
            .find(|c| c.name == name)
            .map(|c| std::mem::take(&mut c.text))
            .unwrap_or_default()
    }
    fn attr(&self, name: &str) -> String {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

/// Element name with the `wp:`/`dc:` prefix dropped; `content:`/`excerpt:` stay so the two
/// `encoded` fields stay apart.
fn local(name: &[u8], lossy: &mut bool) -> String {
    let s = decode(name, lossy);
    match s.strip_prefix("wp:").or_else(|| s.strip_prefix("dc:")) {
        Some(l) => l.to_string(),
        None => s,
    }
}

fn decode(b: &[u8], lossy: &mut bool) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => {
            *lossy = true;
            String::from_utf8_lossy(b).into_owned()
        }
    }
}

fn unescape(raw: String) -> String {
    // Unknown entities (&nbsp; in sloppy exports) are kept verbatim.
    quick_xml::escape::unescape(&raw)
        .map(|c| c.into_owned())
        .unwrap_or(raw)
}

fn node_of(e: &quick_xml::events::BytesStart<'_>, lossy: &mut bool) -> Node {
    let attrs = e
        .attributes()
        .with_checks(false)
        .flatten()
        .map(|a| {
            let v = unescape(decode(&a.value, lossy));
            (local(a.key.as_ref(), lossy), v)
        })
        .collect();
    Node {
        name: local(e.name().as_ref(), lossy),
        attrs,
        ..Node::default()
    }
}

/// Bytes a node's name and attributes hold, counted against the item cap.
fn node_bytes(n: &Node) -> usize {
    // 16 covers the Node/Vec bookkeeping, so floods of empty elements count too.
    16 + n.name.len()
        + n.attrs
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
}

/// Reads from just after `start` to its matching end tag.
fn read_node<R: BufRead>(
    rd: &mut Reader<R>,
    start: &quick_xml::events::BytesStart<'_>,
    lossy: &mut bool,
    cap: usize,
) -> Result<Node, (String, bool)> {
    let mut stack = vec![node_of(start, lossy)];
    let mut buf = Vec::new();
    let (mut size, mut over) = (0usize, false);
    loop {
        buf.clear();
        // Err's bool: the element was consumed to its end tag (no skipping needed).
        match rd
            .read_event_into(&mut buf)
            .map_err(|e| (e.to_string(), false))?
        {
            // Once over the cap only nesting depth is kept, so memory stays bounded.
            Event::Start(_) if over => stack.push(Node::default()),
            Event::Empty(_) if over => {}
            Event::Start(e) => {
                let n = node_of(&e, lossy);
                size += node_bytes(&n);
                over = size > cap;
                stack.push(if over { Node::default() } else { n });
            }
            Event::Empty(e) => {
                let n = node_of(&e, lossy);
                size += node_bytes(&n);
                over = size > cap;
                if let (false, Some(top)) = (over, stack.last_mut()) {
                    top.children.push(n);
                }
            }
            Event::Text(t) if over => drop(t),
            Event::CData(t) if over => drop(t),
            Event::Text(t) => {
                let s = unescape(decode(&t, lossy));
                size += s.len();
                over = size > cap;
                if over {
                    continue;
                }
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&s);
                }
            }
            // WordPress splits a literal `]]>` across two CDATA sections; appending rejoins them.
            Event::CData(t) => {
                let s = decode(&t, lossy);
                size += s.len();
                over = size > cap;
                if over {
                    continue;
                }
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&s);
                }
            }
            Event::End(e) => {
                let name = local(e.name().as_ref(), lossy);
                let node = stack
                    .pop()
                    .ok_or(("unbalanced end tag".to_string(), false))?;
                if !over && node.name != name {
                    let msg = format!("mismatched end tag </{name}> for <{}>", node.name);
                    return Err((msg, false));
                }
                match stack.last_mut() {
                    Some(_) if over => {}
                    Some(parent) => parent.children.push(node),
                    None if over => return Err((too_big(cap), true)),
                    None => return Ok(node),
                }
            }
            Event::Eof => return Err(("unexpected end of file inside element".into(), false)),
            _ => {}
        }
    }
}

fn too_big(cap: usize) -> String {
    const MB: usize = 1024 * 1024;
    if cap.is_multiple_of(MB) {
        format!("item larger than {} MB", cap / MB)
    } else {
        format!("item larger than {cap} bytes")
    }
}

fn build_item(mut n: Node, ordinal: usize, lossy: bool) -> Result<WxrItem, WxrItemError> {
    let err = |post_id, message: String| WxrItemError {
        post_id,
        ordinal,
        message,
    };
    let (title, content, excerpt) = (
        n.take_text("title"),
        n.take_text("content:encoded"),
        n.take_text("excerpt:encoded"),
    );
    let categories = n
        .children
        .iter_mut()
        .filter(|c| c.name == "category")
        .map(|c| {
            (
                c.attr("domain"),
                c.attr("nicename"),
                std::mem::take(&mut c.text),
            )
        })
        .collect();
    let meta = n
        .children
        .iter_mut()
        .filter(|c| c.name == "postmeta")
        .map(|m| (m.take_text("meta_key"), m.take_text("meta_value")))
        .collect();
    let id_text = n.text_of("post_id");
    let post_id: u64 = id_text.trim().parse().map_err(|_| {
        err(
            None,
            format!("post_id is not a number: {:?}", id_text.trim()),
        )
    })?;
    let num = |name: &str| -> Result<i64, WxrItemError> {
        let t = n.text_of(name);
        let t = t.trim();
        if t.is_empty() {
            return Ok(0);
        }
        t.parse()
            .map_err(|_| err(Some(post_id), format!("{name} is not a number: {t:?}")))
    };
    let trimmed = |name: &str| n.text_of(name).trim().to_string();
    Ok(WxrItem {
        post_id,
        post_type: trimmed("post_type"),
        status: trimmed("status"),
        title,
        link: trimmed("link"),
        slug: trimmed("post_name"),
        parent: u64::try_from(num("post_parent")?).unwrap_or(0),
        menu_order: num("menu_order")?,
        date_gmt: trimmed("post_date_gmt"),
        modified_gmt: trimmed("post_modified_gmt"),
        author: trimmed("creator"),
        content,
        excerpt,
        categories,
        meta,
        attachment_url: Some(trimmed("attachment_url")).filter(|u| !u.is_empty()),
        invalid_utf8: lossy,
    })
}

fn author_of(n: &Node) -> WxrAuthor {
    WxrAuthor {
        login: n.text_of("author_login").trim().to_string(),
        email: n.text_of("author_email").trim().to_string(),
        display_name: n.text_of("author_display_name"),
        first_name: n.text_of("author_first_name"),
        last_name: n.text_of("author_last_name"),
    }
}

fn term_of(n: &Node) -> WxrTerm {
    let t = |k: &str| n.text_of(k).trim().to_string();
    let (domain, nicename, name, parent) = match n.name.as_str() {
        "category" => (
            "category".into(),
            t("category_nicename"),
            n.text_of("cat_name"),
            t("category_parent"),
        ),
        "tag" => (
            "post_tag".into(),
            t("tag_slug"),
            n.text_of("tag_name"),
            String::new(),
        ),
        _ => (
            t("term_taxonomy"),
            t("term_slug"),
            n.text_of("term_name"),
            t("term_parent"),
        ),
    };
    WxrTerm {
        domain,
        nicename,
        name,
        parent,
    }
}

/// Streams the export. A malformed item reaches `on_item` as `Err` and parsing goes on.
pub fn parse_wxr<R: BufRead>(
    r: R,
    on_item: impl FnMut(Result<WxrItem, WxrItemError>),
) -> Result<WxrHeader> {
    parse_wxr_with_cap(r, MAX_ITEM_BYTES, on_item)
}

/// `parse_wxr` with an explicit per-item byte cap.
pub fn parse_wxr_with_cap<R: BufRead>(
    r: R,
    cap: usize,
    mut on_item: impl FnMut(Result<WxrItem, WxrItemError>),
) -> Result<WxrHeader> {
    parse_until(r, cap, |i| {
        on_item(i);
        true
    })
}

/// Like `parse_wxr`, but stops early once `on_item` returns false.
fn parse_until<R: BufRead>(
    r: R,
    cap: usize,
    mut on_item: impl FnMut(Result<WxrItem, WxrItemError>) -> bool,
) -> Result<WxrHeader> {
    let mut rd = Reader::from_reader(r);
    rd.config_mut().check_end_names = false;
    let mut header = WxrHeader::default();
    let mut buf = Vec::new();
    let (mut depth, mut ordinal) = (0usize, 0usize);
    loop {
        buf.clear();
        let ev = rd
            .read_event_into(&mut buf)
            .with_context(|| format!("XML error at byte {}", rd.buffer_position()))?;
        match ev {
            Event::Start(e) => {
                let mut lossy = false;
                let name = local(e.name().as_ref(), &mut lossy);
                // depth 2 = directly inside <rss><channel>
                let consume = match name.as_str() {
                    "item" => true,
                    "author" | "category" | "tag" | "term" | "title" | "link" | "base_site_url" => {
                        depth == 2
                    }
                    _ => false,
                };
                if !consume {
                    depth += 1;
                    continue;
                }
                let node = read_node(&mut rd, &e, &mut lossy, cap);
                if name != "item" {
                    let Ok(n) = node else { continue };
                    match name.as_str() {
                        "author" => header.authors.push(author_of(&n)),
                        "title" => header.title = n.text,
                        "link" => header.link = n.text.trim().to_string(),
                        "base_site_url" => header.base_site_url = n.text.trim().to_string(),
                        _ => header.terms.push(term_of(&n)),
                    }
                    continue;
                }
                ordinal += 1;
                let item = match node {
                    Ok(n) => build_item(n, ordinal, lossy),
                    Err((message, consumed)) => {
                        if !consumed {
                            skip_item(&mut rd);
                        }
                        Err(WxrItemError {
                            post_id: None,
                            ordinal,
                            message,
                        })
                    }
                };
                if !on_item(item) {
                    break;
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(header)
}

/// After a structural error: drop everything up to the broken item's `</item>`.
fn skip_item<R: BufRead>(rd: &mut Reader<R>) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let before = rd.buffer_position();
        match rd.read_event_into(&mut buf) {
            Ok(Event::End(e)) if local(e.name().as_ref(), &mut false) == "item" => break,
            Ok(Event::Eof) => break,
            Err(_) if rd.buffer_position() == before => break,
            _ => {}
        }
    }
}

pub struct WxrConnector {
    path: PathBuf,
}

impl WxrConnector {
    pub fn new(cfg: &ConnectorConfig) -> Self {
        let path = match cfg {
            ConnectorConfig::Wxr { path } => path.clone(),
            _ => PathBuf::new(),
        };
        WxrConnector { path }
    }

    fn open(&self, ctx: &StageCtx) -> Result<BufReader<std::fs::File>> {
        let p = ctx.project.dir().join(&self.path);
        let f = std::fs::File::open(&p).with_context(|| format!("opening {}", p.display()))?;
        Ok(BufReader::with_capacity(256 * 1024, f))
    }
}

fn doc_ref(post_id: &str, url: Option<String>) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: NAME.into(),
        source_id: post_id.to_string(),
        url,
    };
    (ItemId::new(ItemKind::Document, &src), src)
}

fn failed_ref(e: &WxrItemError) -> (ItemId, SourceRef) {
    match e.post_id {
        Some(p) => doc_ref(&p.to_string(), None),
        None => doc_ref(&format!("item-{}", e.ordinal), None),
    }
}

fn author_ref(login: &str) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: NAME.into(),
        source_id: login.into(),
        url: None,
    };
    (ItemId::new(ItemKind::Author, &src), src)
}

fn term_ref(t: &WxrTerm) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: NAME.into(),
        source_id: format!("{}:{}", t.domain, t.nicename),
        url: None,
    };
    (ItemId::new(ItemKind::Term, &src), src)
}

/// Writes the RawItem row and the stage status; for documents, `i` is the parsed item.
fn record(
    ctx: &StageCtx,
    kind: ItemKind,
    (id, src): (ItemId, SourceRef),
    row: RawItem,
    status: Option<ItemStatus>,
) -> Result<()> {
    ctx.project.store().put_item(kind, id, &src, &row)?;
    match status {
        Some(s) => ctx.item_status(id, kind, s),
        None => Ok(()),
    }
}

fn doc_row(i: &WxrItem) -> (ItemId, SourceRef, RawItem) {
    let (id, src) = doc_ref(
        &i.post_id.to_string(),
        Some(i.link.clone()).filter(|l| !l.is_empty()),
    );
    let row = RawItem {
        label: i.post_type.clone(),
        title: Some(i.title.clone()),
        status: Some(i.status.clone()),
        url: src.url.clone(),
        raw_path: Some(format!("items/{}.json", i.post_id)),
    };
    (id, src, row)
}

fn failed_row() -> RawItem {
    RawItem {
        label: "unparsed".into(),
        title: None,
        status: None,
        url: None,
        raw_path: None,
    }
}

fn lossy_status(lossy: bool) -> ItemStatus {
    if lossy {
        ItemStatus::Partial {
            reason: "invalid UTF-8 replaced".into(),
        }
    } else {
        ItemStatus::Done
    }
}

fn write_json<T: Serialize + ?Sized>(path: &Path, v: &T) -> Result<()> {
    Ok(std::fs::write(path, serde_json::to_vec_pretty(v)?)?)
}

fn mark_header(ctx: &StageCtx, h: &WxrHeader) -> Result<()> {
    for a in &h.authors {
        let row = RawItem {
            label: "author".into(),
            title: Some(a.display_name.clone()),
            status: None,
            url: None,
            raw_path: None,
        };
        record(
            ctx,
            ItemKind::Author,
            author_ref(&a.login),
            row,
            Some(ItemStatus::Done),
        )?;
    }
    for t in &h.terms {
        let row = RawItem {
            label: t.domain.clone(),
            title: Some(t.name.clone()),
            status: None,
            url: None,
            raw_path: None,
        };
        record(
            ctx,
            ItemKind::Term,
            term_ref(t),
            row,
            Some(ItemStatus::Done),
        )?;
    }
    Ok(())
}

/// Runs `per_item` for every parsed item until the file ends, the run is cancelled or
/// `per_item` errors (returned).
fn stream_items(
    ctx: &StageCtx,
    r: impl BufRead,
    mut per_item: impl FnMut(Result<WxrItem, WxrItemError>) -> Result<()>,
) -> Result<WxrHeader> {
    let mut fail = None;
    let mut seen = 0u64;
    let header = parse_until(r, MAX_ITEM_BYTES, |item| {
        if ctx.cancel.is_cancelled() {
            return false;
        }
        if let Err(e) = per_item(item) {
            fail = Some(e);
            return false;
        }
        seen += 1;
        if seen.is_multiple_of(500) {
            ctx.progress(seen, None, "items");
        }
        true
    })?;
    match fail {
        Some(e) => Err(e),
        None => Ok(header),
    }
}

#[async_trait]
impl Connector for WxrConnector {
    fn name(&self) -> &str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            reads: [Feature::Media, Feature::Taxonomies, Feature::Drafts]
                .into_iter()
                .collect(),
            writes: Default::default(),
        }
    }

    async fn analyse(&self, ctx: &StageCtx) -> Result<()> {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        let mut attachments = 0u64;
        let header = stream_items(ctx, self.open(ctx)?, |r| match r {
            Ok(i) => {
                *counts.entry(i.post_type.clone()).or_default() += 1;
                attachments += u64::from(i.attachment_url.is_some());
                let (id, src, mut row) = doc_row(&i);
                row.raw_path = None; // nothing on disk until extract
                record(
                    ctx,
                    ItemKind::Document,
                    (id, src),
                    row,
                    Some(ItemStatus::Done),
                )
            }
            Err(e) => {
                let status = ItemStatus::Failed {
                    error: e.to_string(),
                };
                record(
                    ctx,
                    ItemKind::Document,
                    failed_ref(&e),
                    failed_row(),
                    Some(status),
                )
            }
        })?;
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        mark_header(ctx, &header)?;
        let dir = ctx.project.raw_dir(NAME);
        std::fs::create_dir_all(&dir)?;
        let analysis = serde_json::json!({
            "items_by_type": counts,
            "authors": header.authors.len(),
            "terms": header.terms.len(),
            "attachments": attachments,
        });
        write_json(&dir.join("analysis.json"), &analysis)
    }

    async fn extract(&self, ctx: &StageCtx) -> Result<()> {
        let dir = ctx.project.raw_dir(NAME);
        let items_dir = dir.join("items");
        std::fs::create_dir_all(&items_dir)?;
        // Attachments get their final status after the download, so they wait here.
        let mut pending: Vec<(ItemId, String, bool)> = vec![];
        let header = stream_items(ctx, self.open(ctx)?, |r| match r {
            Ok(i) => {
                let (id, src, row) = doc_row(&i);
                let file = items_dir.join(format!("{}.json", i.post_id));
                let status = match write_json(&file, &i) {
                    Err(e) => Some(ItemStatus::Failed {
                        error: format!("{e:#}"),
                    }),
                    Ok(()) => match &i.attachment_url {
                        Some(url) => {
                            pending.push((id, url.clone(), i.invalid_utf8));
                            None
                        }
                        None => Some(lossy_status(i.invalid_utf8)),
                    },
                };
                record(ctx, ItemKind::Document, (id, src), row, status)
            }
            Err(e) => {
                let status = ItemStatus::Failed {
                    error: e.to_string(),
                };
                record(
                    ctx,
                    ItemKind::Document,
                    failed_ref(&e),
                    failed_row(),
                    Some(status),
                )
            }
        })?;
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        write_json(&dir.join("authors.json"), &header.authors)?;
        write_json(&dir.join("terms.json"), &header.terms)?;
        let site = serde_json::json!({
            "title": header.title, "link": header.link, "base_site_url": header.base_site_url,
        });
        write_json(&dir.join("site.json"), &site)?;
        mark_header(ctx, &header)?;
        download_attachments(ctx, pending).await
    }
}

// ponytail: sequential downloads; add a bounded JoinSet if big libraries make this slow.
async fn download_attachments(ctx: &StageCtx, pending: Vec<(ItemId, String, bool)>) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let client = media_client(USER_AGENT)?;
    let store = ctx.project.store();
    let total = pending.len() as u64;
    let mut iter = pending.into_iter().enumerate();
    while let Some((n, (id, url, lossy))) = iter.next() {
        ctx.progress(n as u64, Some(total), "attachments");
        let partial = |reason: String| ItemStatus::Partial { reason };
        let warn = |msg: String| {
            ctx.issue(
                IssueKind::MissingMedia,
                Severity::Warning,
                Some(id),
                msg,
                serde_json::json!({ "url": url }),
            )
        };
        let status = if !ctx.refresh && store.media_by_url(&url)?.is_some() {
            lossy_status(lossy)
        } else {
            match download_capped(&client, &url, DEFAULT_MEDIA_CAP, &ctx.cancel).await {
                Ok(Download::Cancelled) => {
                    // Resumable: leave this and the rest unfinished, then stop.
                    let reason = "interrupted before download".to_string();
                    ctx.item_status(id, ItemKind::Document, partial(reason.clone()))?;
                    for (_, (rest, _, _)) in iter {
                        ctx.item_status(rest, ItemKind::Document, partial(reason.clone()))?;
                    }
                    return Ok(());
                }
                Ok(Download::Ok(bytes)) => {
                    let name = url
                        .split(['?', '#'])
                        .next()
                        .and_then(|u| u.rsplit('/').next())
                        .filter(|n| !n.is_empty())
                        .unwrap_or("file");
                    store.put_media_bytes(&bytes, &url, name)?;
                    lossy_status(lossy)
                }
                Ok(Download::OverCap(n)) => {
                    warn(format!("file over size cap ({n} bytes)"))?;
                    partial("file over size cap".into())
                }
                Ok(Download::Status(c)) => {
                    warn(format!("could not download {url}: HTTP {c}"))?;
                    partial(format!("media download failed: HTTP {c}"))
                }
                Err(e) => {
                    warn(format!("could not download {url}: {e:#}"))?;
                    partial(format!("media download failed: {e:#}"))
                }
            }
        };
        ctx.item_status(id, ItemKind::Document, status)?;
    }
    Ok(())
}
