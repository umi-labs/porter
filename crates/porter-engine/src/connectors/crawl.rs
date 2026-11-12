//! Public-site crawl connector: the one source porter always has access to.
//! Records every URL's status, redirect hops and head metadata, detects the platform
//! and (in `extract`) saves HTML and downloads media.

use crate::config::ConnectorConfig;
use crate::connectors::fetch::{
    Download, download_capped, media_client as new_media_client, read_capped,
};
use crate::http;
use crate::stage::{Capabilities, Connector, Feature, StageCtx};
use crate::url::{UrlKey, normalise_url};
use ::url::Url;
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use porter_model::{
    IssueKind, ItemId, ItemKind, ItemStatus, Redirect, RedirectOrigin, Route, RouteTarget,
    Severity, SourceRef,
};
use regex::Regex;
use reqwest::Client;
use reqwest::header::{CONTENT_TYPE, HeaderMap, LINK, LOCATION};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const NAME: &str = "crawl";
const USER_AGENT: &str = "porter/2.0 (+https://github.com/umi-labs/porter)";
const MAX_HOPS: usize = 10;
const PAGE_CAP: u64 = 20 * 1024 * 1024;
const MEDIA_EXTS: [&str; 24] = [
    "jpg", "jpeg", "png", "gif", "webp", "avif", "svg", "ico", "bmp", "tif", "tiff", "pdf", "mp4",
    "webm", "mov", "mp3", "wav", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "zip",
];

/// What the crawl recognised about the site. Platform names live here, in the
/// connector, never in the engine core.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Detected {
    pub platform: Option<String>,
    pub builders: Vec<String>,
    pub plugins: Vec<String>,
}

pub fn detect_platform(html: &str, headers: &HeaderMap) -> Detected {
    let lower = html.to_lowercase();
    let has = |s: &str| lower.contains(s);
    let generator = parse_head(&Html::parse_document(html))
        .generator
        .unwrap_or_default();
    let generator = generator.to_lowercase();

    let builders: Vec<String> = [
        ("divi", has("et_pb_") || has("/themes/divi")),
        ("elementor", has("elementor-")),
        ("gutenberg", has("wp-block-")),
        ("wpbakery", has("vc_row")),
    ]
    .into_iter()
    .filter(|(_, hit)| *hit)
    .map(|(n, _)| n.to_string())
    .collect();
    let plugins: Vec<String> = [
        ("yoast", has("this site is optimized with the yoast")),
        ("rank-math", has("rank math")),
        ("wpml", has("wpml-ls")),
        ("gravity-forms", has("gform_wrapper")),
        ("ninja-forms", has("nf-form-cont")),
        ("smart-slider", has("n2-section-smartslider")),
    ]
    .into_iter()
    .filter(|(_, hit)| *hit)
    .map(|(n, _)| n.to_string())
    .collect();

    let link_wp = headers
        .get_all(LINK)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.contains("wp-json") || v.contains("api.w.org"));
    let wordpress = generator.starts_with("wordpress")
        || has("/wp-content/")
        || has("/wp-json/")
        || link_wp
        || !builders.is_empty()
        || !plugins.is_empty();
    // First match wins; WordPress builders/plugins imply WordPress, a Sanity CDN beats Gatsby.
    let platform = [
        ("wordpress", wordpress),
        (
            "squarespace",
            generator.contains("squarespace") || has("squarespace.com"),
        ),
        ("webflow", has("data-wf-site")),
        ("sanity", has("cdn.sanity.io")),
        ("gatsby", has("___gatsby")),
    ]
    .into_iter()
    .find(|(_, hit)| *hit)
    .map(|(n, _)| n.to_string());
    Detected {
        platform,
        builders,
        plugins,
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PageHead {
    pub url: String,
    pub status: Option<u16>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical: Option<String>,
    pub generator: Option<String>,
    pub og: BTreeMap<String, String>,
    /// `(lang, href)`
    pub hreflang: Vec<(String, String)>,
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

fn attr(el: scraper::ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(str::to_string)
}

#[allow(clippy::field_reassign_with_default)]
fn parse_head(doc: &Html) -> PageHead {
    let mut h = PageHead::default();
    h.title = doc
        .select(&sel("title"))
        .next()
        .map(|t| t.text().collect::<String>().trim().to_string())
        .filter(|t| !t.is_empty());
    for m in doc.select(&sel("meta")) {
        let content = attr(m, "content");
        match (attr(m, "name"), attr(m, "property"), content) {
            (Some(n), _, Some(c)) if n.eq_ignore_ascii_case("description") => {
                h.description = Some(c)
            }
            (Some(n), _, Some(c)) if n.eq_ignore_ascii_case("generator") => h.generator = Some(c),
            (_, Some(p), Some(c)) if p.starts_with("og:") => {
                h.og.insert(p, c);
            }
            _ => {}
        }
    }
    h.canonical = doc
        .select(&sel("link[rel=canonical]"))
        .next()
        .and_then(|l| attr(l, "href"));
    for l in doc.select(&sel("link[rel=alternate][hreflang]")) {
        if let (Some(lang), Some(href)) = (attr(l, "hreflang"), attr(l, "href")) {
            h.hreflang.push((lang, href));
        }
    }
    h
}

fn site_of(u: &Url) -> Option<String> {
    normalise_url(u.as_str(), None).map(|k| k.0.split('/').next().unwrap_or_default().to_string())
}

fn key_of(u: &Url) -> UrlKey {
    normalise_url(u.as_str(), None).unwrap_or_else(|| UrlKey(u.to_string()))
}

fn has_media_ext(u: &Url) -> bool {
    u.path()
        .rsplit_once('.')
        .is_some_and(|(_, e)| MEDIA_EXTS.contains(&e.to_lowercase().as_str()))
}

/// URLs of an `srcset`. ponytail: tokenises on whitespace, so `a.jpg 1x,b.jpg 2x`
/// (no space after the comma) is misread; upgrade to the spec algorithm if seen.
fn srcset_urls(v: &str) -> Vec<&str> {
    let descriptor = |t: &str| {
        let t = t.trim_end_matches(',');
        t.strip_suffix(['w', 'x'])
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.'))
    };
    v.split_whitespace()
        .filter(|t| !descriptor(t))
        .map(|t| t.trim_end_matches(','))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Drops query params not named in `keep`, so calendar/facet links collapse.
fn strip_query(mut u: Url, keep: &[String]) -> Url {
    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| keep.iter().any(|n| n == k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(pairs);
    }
    u
}

struct Parsed {
    head: PageHead,
    links: Vec<Url>,
    media: Vec<String>,
    detected: Detected,
}

/// Sync on purpose: `scraper::Html` is not `Send`, so it never lives across an await.
fn parse_page(
    html: &str,
    base: &Url,
    headers: &HeaderMap,
    css_url: &Regex,
    keep_query: &[String],
) -> Parsed {
    let doc = Html::parse_document(html);
    let resolve = |raw: &str| {
        let mut u = base.join(raw.trim()).ok()?;
        u.set_fragment(None);
        matches!(u.scheme(), "http" | "https").then_some(u)
    };
    let site = site_of(base);
    let mut links = Vec::new();
    let mut media: Vec<String> = Vec::new();
    for a in doc.select(&sel("a[href]")) {
        let Some(u) = attr(a, "href").and_then(|h| resolve(&h)) else {
            continue;
        };
        if has_media_ext(&u) {
            media.push(u.to_string());
        } else if site_of(&u) == site {
            links.push(strip_query(u, keep_query));
        }
    }
    for el in doc.select(&sel("img[src], img[srcset], source[srcset]")) {
        let srcs = attr(el, "src").into_iter().chain(
            attr(el, "srcset")
                .iter()
                .flat_map(|s| srcset_urls(s).into_iter().map(str::to_string))
                .collect::<Vec<_>>(),
        );
        media.extend(srcs.filter_map(|s| resolve(&s)).map(|u| u.to_string()));
    }
    for el in doc.select(&sel("[style]")) {
        let style = el.value().attr("style").unwrap_or_default();
        for c in css_url.captures_iter(style) {
            media.extend(resolve(&c[1]).map(|u| u.to_string()));
        }
    }
    let mut seen = HashSet::new();
    media.retain(|m| seen.insert(m.clone()));
    Parsed {
        head: parse_head(&doc),
        links,
        media,
        detected: detect_platform(html, headers),
    }
}

// ---- robots.txt and sitemaps ----

#[derive(Default)]
struct Robots {
    disallow: Vec<String>,
    sitemaps: Vec<String>,
}

impl Robots {
    /// Rules of the `porter` group if there is one, else the `*` group.
    /// ponytail: `Allow` and wildcards are ignored (prefix match only), which is
    /// stricter than the spec; implement longest-match if a site needs it.
    fn parse(text: &str) -> Robots {
        let mut robots = Robots::default();
        let mut groups: Vec<(Vec<String>, Vec<String>)> = vec![];
        let (mut agents, mut rules, mut in_rules) = (vec![], vec![], false);
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let v = v.trim();
            match k.trim().to_lowercase().as_str() {
                "user-agent" => {
                    if in_rules {
                        groups.push((std::mem::take(&mut agents), std::mem::take(&mut rules)));
                        in_rules = false;
                    }
                    agents.push(v.to_lowercase());
                }
                "disallow" => {
                    in_rules = true;
                    if !v.is_empty() {
                        rules.push(v.trim_end_matches('*').to_string());
                    }
                }
                "allow" => in_rules = true,
                "sitemap" => robots.sitemaps.push(v.to_string()),
                _ => {}
            }
        }
        groups.push((agents, rules));
        let pick = |name: &str| -> Option<Vec<String>> {
            let g: Vec<_> = groups
                .iter()
                .filter(|(a, _)| a.iter().any(|a| a == name))
                .collect();
            (!g.is_empty()).then(|| g.into_iter().flat_map(|(_, r)| r.clone()).collect())
        };
        robots.disallow = pick("porter").or_else(|| pick("*")).unwrap_or_default();
        robots
    }

    fn blocks(&self, u: &Url) -> bool {
        let mut p = u.path().to_string();
        if let Some(q) = u.query() {
            p = format!("{p}?{q}");
        }
        self.disallow.iter().any(|r| p.starts_with(r.as_str()))
    }
}

fn sitemap_locs(xml: &str) -> (bool, Vec<String>) {
    use quick_xml::{Reader, events::Event};
    let mut reader = Reader::from_str(xml);
    let (mut index, mut in_loc, mut locs) = (false, false, vec![]);
    while let Ok(ev) = reader.read_event() {
        match ev {
            Event::Start(e) => {
                let n = e.local_name();
                index |= n.as_ref() == b"sitemapindex";
                in_loc = n.as_ref() == b"loc";
            }
            Event::Text(t) if in_loc => {
                if let Ok(t) = t.unescape() {
                    locs.push(t.trim().to_string());
                }
            }
            Event::CData(t) if in_loc => {
                locs.push(String::from_utf8_lossy(&t.into_inner()).trim().to_string());
            }
            Event::End(_) => in_loc = false,
            Event::Eof => break,
            _ => {}
        }
    }
    (index, locs)
}

/// GET a small text file; `None` on any failure or cancellation.
async fn get_text(client: &Client, url: &str, cancel: &CancellationToken) -> Option<String> {
    tokio::select! {
        _ = cancel.cancelled() => None,
        r = async {
            let r = http::send_with_retry(client.get(url), 2).await.ok()?;
            if !r.status().is_success() {
                return None;
            }
            let body = read_capped(r, PAGE_CAP).await.ok()?.ok()?;
            Some(String::from_utf8_lossy(&body).into_owned())
        } => r,
    }
}

// ---- fetching ----

struct Hop {
    from: Url,
    to: Url,
    code: u16,
}

struct Fetched {
    hops: Vec<Hop>,
    first_status: u16,
    last: Url,
    status: u16,
    headers: HeaderMap,
    html: Option<String>,
    /// "redirect limit" / "redirect loop"
    error: Option<String>,
    /// A hop left the site: recorded as a redirect, never fetched.
    off_host: bool,
}

/// Follows redirects by hand so every hop is recorded. `Err` is a network failure.
async fn fetch(client: &Client, start: &Url) -> Result<Fetched> {
    let site = site_of(start);
    let mut cur = start.clone();
    let mut hops: Vec<Hop> = vec![];
    let mut seen = HashSet::from([cur.to_string()]);
    let mut first = None;
    loop {
        let resp = http::send_with_retry(client.get(cur.clone()), 3).await?;
        let status = resp.status().as_u16();
        let first_status = *first.get_or_insert(status);
        let location = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok());
        let next = location
            .filter(|_| resp.status().is_redirection())
            .and_then(|l| cur.join(l).ok())
            .map(|mut u| {
                u.set_fragment(None);
                u
            });
        let headers = resp.headers().clone();
        let done = |last: Url, html, error: Option<&str>, hops| Fetched {
            hops,
            first_status,
            last,
            status,
            headers: headers.clone(),
            html,
            error: error.map(str::to_string),
            off_host: false,
        };
        if let Some(next) = next {
            hops.push(Hop {
                from: cur.clone(),
                to: next.clone(),
                code: status,
            });
            let error = if !seen.insert(next.to_string()) {
                Some("redirect loop")
            } else if hops.len() > MAX_HOPS {
                Some("redirect limit")
            } else {
                None
            };
            if error.is_some() {
                return Ok(done(next, None, error, hops));
            }
            if site_of(&next) != site {
                let mut f = done(next, None, None, hops);
                f.off_host = true;
                return Ok(f);
            }
            cur = next;
            continue;
        }
        let is_html = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_none_or(|t| t.contains("html"));
        // ponytail: a page over the cap is treated as non-HTML (not parsed).
        let html = if is_html {
            read_capped(resp, PAGE_CAP)
                .await?
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        } else {
            None
        };
        return Ok(done(cur, html, None, hops));
    }
}

// ---- the connector ----

#[derive(Clone)]
struct Opts {
    start: Url,
    concurrency: usize,
    delay: Duration,
    include: GlobSet,
    exclude: GlobSet,
    max_pages: Option<usize>,
    max_media: u64,
    keep_query: Vec<String>,
    any_include: bool,
}

impl Opts {
    fn from(cfg: &ConnectorConfig) -> Result<Opts> {
        let ConnectorConfig::Crawl {
            url,
            concurrency,
            delay_ms,
            include,
            exclude,
            max_pages,
            keep_query,
            max_media_bytes,
        } = cfg
        else {
            return Err(anyhow!("not a crawl connector config"));
        };
        let set = |pats: &[String]| -> Result<GlobSet> {
            let mut b = GlobSetBuilder::new();
            for p in pats {
                b.add(Glob::new(p).with_context(|| format!("bad glob {p}"))?);
            }
            Ok(b.build()?)
        };
        let mut start = Url::parse(url).with_context(|| format!("crawl url {url}"))?;
        start.set_fragment(None);
        Ok(Opts {
            start,
            concurrency: (*concurrency).max(1),
            delay: Duration::from_millis(*delay_ms),
            include: set(include)?,
            exclude: set(exclude)?,
            max_pages: *max_pages,
            max_media: *max_media_bytes,
            keep_query: keep_query.clone(),
            any_include: !include.is_empty(),
        })
    }

    fn allows(&self, u: &Url) -> bool {
        let p = u.path();
        !self.exclude.is_match(p) && (!self.any_include || self.include.is_match(p))
    }
}

pub struct CrawlConnector {
    opts: Result<Opts, String>,
}

impl CrawlConnector {
    pub fn new(cfg: &ConnectorConfig) -> Self {
        Self {
            opts: Opts::from(cfg).map_err(|e| format!("{e:#}")),
        }
    }

    fn opts(&self) -> Result<&Opts> {
        self.opts.as_ref().map_err(|e| anyhow!("{e}"))
    }
}

#[async_trait]
impl Connector for CrawlConnector {
    fn name(&self) -> &str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        let f = [Feature::Media, Feature::Redirects, Feature::Seo];
        Capabilities {
            reads: f.into_iter().collect(),
            writes: Default::default(),
        }
    }

    async fn analyse(&self, ctx: &StageCtx) -> Result<()> {
        crawl(ctx, self.opts()?, false).await
    }

    async fn extract(&self, ctx: &StageCtx) -> Result<()> {
        crawl(ctx, self.opts()?, true).await
    }
}

struct Shared {
    ctx: StageCtx,
    client: Client,
    media_client: Client,
    opts: Opts,
    robots: Robots,
    sem: Semaphore,
    extract: bool,
    css_url: Regex,
}

#[derive(Default)]
struct Outcome {
    links: Vec<Url>,
    landed: Option<UrlKey>,
    detected: Option<Detected>,
}

fn hash16(key: &UrlKey) -> String {
    Sha256::digest(key.0.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn route_id(url: &Url) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: NAME.into(),
        source_id: key_of(url).0,
        url: Some(url.to_string()),
    };
    (ItemId::new(ItemKind::Route, &src), src)
}

fn put_route(
    ctx: &StageCtx,
    url: &Url,
    status: Option<u16>,
    target: RouteTarget,
    title: Option<String>,
) -> Result<ItemId> {
    let (id, src) = route_id(url);
    let route = Route {
        url: url.to_string(),
        target,
        http_status: status,
        title,
    };
    ctx.project
        .store()
        .put_item(ItemKind::Route, id, &src, &route)?;
    Ok(id)
}

async fn pause_for(cancel: &CancellationToken, delay: Duration) {
    if !delay.is_zero() {
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

async fn pause(sh: &Shared) {
    pause_for(&sh.ctx.cancel, sh.opts.delay).await
}

/// One URL: fetch, record, parse; in extract also save the HTML and download media.
/// `None` when cancelled (the item was never entered).
async fn visit(sh: Arc<Shared>, url: Url) -> Result<Option<Outcome>> {
    let ctx = &sh.ctx;
    let _permit = sh.sem.acquire().await?;
    if ctx.cancel.is_cancelled() {
        return Ok(None);
    }
    let (id, _) = route_id(&url);
    if sh.robots.blocks(&url) {
        put_route(ctx, &url, None, RouteTarget::None, None)?;
        let reason = "disallowed by robots.txt".to_string();
        ctx.item_status(id, ItemKind::Route, ItemStatus::Skipped { reason })?;
        return Ok(Some(Outcome::default()));
    }
    let raw = ctx.project.raw_dir(NAME);
    let html_path = |u: &Url| {
        raw.join("html")
            .join(format!("{}.html", hash16(&key_of(u))))
    };

    // Extract reuses saved HTML unless --refresh; no network for the page itself.
    if sh.extract && !ctx.refresh && html_path(&url).exists() {
        let html = std::fs::read_to_string(html_path(&url))?;
        let parsed = parse_page(
            &html,
            &url,
            &HeaderMap::new(),
            &sh.css_url,
            &sh.opts.keep_query,
        );
        ctx.item_status(id, ItemKind::Route, ItemStatus::Done)?;
        download_media(&sh, id, &parsed.media).await?;
        pause(&sh).await;
        return Ok(Some(Outcome {
            links: parsed.links,
            landed: Some(key_of(&url)),
            detected: None,
        }));
    }
    let fetched = tokio::select! {
        _ = ctx.cancel.cancelled() => return Ok(None),
        r = fetch(&sh.client, &url) => r,
    };
    let f = match fetched {
        Ok(f) => f,
        Err(e) => {
            put_route(ctx, &url, None, RouteTarget::None, None)?;
            let error = format!("{e:#}");
            ctx.item_status(id, ItemKind::Route, ItemStatus::Failed { error })?;
            pause(&sh).await;
            return Ok(Some(Outcome::default()));
        }
    };
    let parsed = f
        .html
        .as_deref()
        .map(|h| parse_page(h, &f.last, &f.headers, &sh.css_url, &sh.opts.keep_query));
    record(ctx, &url, &f, parsed.as_ref())?;
    if let Some(p) = &parsed {
        let mut head = p.head.clone();
        head.url = f.last.to_string();
        head.status = Some(f.status);
        let dir = raw.join("pages");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(format!("{}.json", hash16(&key_of(&f.last))));
        std::fs::write(file, serde_json::to_vec_pretty(&head)?)?;
    }
    let landed = (f.error.is_none() && !f.off_host).then(|| f.last.clone());
    if sh.extract {
        if let (Some(html), Some(l)) = (&f.html, &landed) {
            let path = html_path(l);
            std::fs::create_dir_all(path.parent().unwrap_or(&raw))?;
            std::fs::write(path, html)?;
        }
        let media = parsed.as_ref().map(|p| p.media.clone()).unwrap_or_default();
        download_media(&sh, route_id(&f.last).0, &media).await?;
    }
    pause(&sh).await;
    Ok(Some(Outcome {
        links: parsed.as_ref().map(|p| p.links.clone()).unwrap_or_default(),
        landed: landed.as_ref().map(key_of),
        detected: parsed.map(|p| p.detected),
    }))
}

/// Stores the route, its redirect hops and statuses.
fn record(ctx: &StageCtx, url: &Url, f: &Fetched, parsed: Option<&Parsed>) -> Result<()> {
    let store = ctx.project.store();
    let (id, _) = route_id(url);
    let title = parsed.and_then(|p| p.head.title.clone());
    // A hop that only differs by normalisation (trailing slash, http/https, www) is the
    // same route, not a redirect worth recording.
    let hops: Vec<&Hop> = f
        .hops
        .iter()
        .filter(|h| key_of(&h.from) != key_of(&h.to))
        .collect();
    let same = key_of(&f.last) == key_of(url);
    let mut first = RouteTarget::None;
    for (i, h) in hops.iter().enumerate() {
        let src = SourceRef {
            connector: NAME.into(),
            source_id: format!("{}->{}", key_of(&h.from).0, key_of(&h.to).0),
            url: Some(h.from.to_string()),
        };
        let rid = ItemId::new(ItemKind::Redirect, &src);
        let red = Redirect {
            from: h.from.to_string(),
            to: h.to.to_string(),
            code: h.code,
            origin: RedirectOrigin::Crawl,
        };
        store.put_item(ItemKind::Redirect, rid, &src, &red)?;
        ctx.item_status(rid, ItemKind::Redirect, ItemStatus::Done)?;
        if i == 0 {
            first = RouteTarget::Redirect(rid);
        }
    }
    if f.off_host {
        put_route(ctx, url, Some(f.first_status), first, None)?;
        return ctx.item_status(id, ItemKind::Route, ItemStatus::Done);
    }
    if let Some(e) = &f.error {
        let kind = if e == "redirect loop" {
            IssueKind::RedirectLoop
        } else {
            IssueKind::RedirectChain
        };
        ctx.issue(
            kind,
            Severity::Warning,
            Some(id),
            format!("{url}: {e}"),
            serde_json::json!({ "hops": f.hops.len() }),
        )?;
        put_route(ctx, url, Some(f.first_status), first, None)?;
        let error = e.clone();
        return ctx.item_status(id, ItemKind::Route, ItemStatus::Failed { error });
    }
    if hops.len() > 1 {
        ctx.issue(
            IssueKind::RedirectChain,
            Severity::Info,
            Some(id),
            format!("{url} redirects through {} hops", hops.len()),
            serde_json::json!({ "hops": hops.len() }),
        )?;
    }
    if same {
        put_route(ctx, url, Some(f.status), first, title)?;
        return ctx.item_status(id, ItemKind::Route, ItemStatus::Done);
    }
    put_route(ctx, url, Some(f.first_status), first, None)?;
    ctx.item_status(id, ItemKind::Route, ItemStatus::Done)?;
    let lid = put_route(ctx, &f.last, Some(f.status), RouteTarget::None, title)?;
    ctx.item_status(lid, ItemKind::Route, ItemStatus::Done)
}

/// Id for a media file we declined to download (no bytes, so no hash-based id).
fn media_placeholder(url: &str) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: NAME.into(),
        source_id: url.to_string(),
        url: Some(url.to_string()),
    };
    (ItemId::new(ItemKind::Media, &src), src)
}

async fn download_media(sh: &Shared, route: ItemId, urls: &[String]) -> Result<()> {
    let ctx = &sh.ctx;
    let store = ctx.project.store();
    let warn = |msg: String, url: &str| {
        ctx.issue(
            IssueKind::MissingMedia,
            Severity::Warning,
            Some(route),
            msg,
            serde_json::json!({ "url": url }),
        )
    };
    for url in urls {
        if ctx.cancel.is_cancelled() {
            break;
        }
        if !ctx.refresh && store.media_by_url(url)?.is_some() {
            continue;
        }
        let bytes =
            match download_capped(&sh.media_client, url, sh.opts.max_media, &ctx.cancel).await {
                Ok(Download::Cancelled) => break,
                Ok(Download::Ok(b)) => Ok(b),
                Ok(Download::OverCap(n)) => {
                    let (id, _) = media_placeholder(url);
                    let reason = "over size cap".to_string();
                    ctx.item_status(id, ItemKind::Media, ItemStatus::Skipped { reason })?;
                    warn(format!("file over size cap ({n} bytes)"), url)?;
                    pause(sh).await;
                    continue;
                }
                Ok(Download::Status(c)) => Err(format!("HTTP {c}")),
                Err(e) => Err(format!("{e:#}")),
            };
        match bytes {
            Err(e) => warn(format!("could not download {url}: {e}"), url)?,
            Ok(bytes) => {
                let name = url
                    .split(['?', '#'])
                    .next()
                    .and_then(|u| u.rsplit('/').next())
                    .filter(|n| !n.is_empty())
                    .unwrap_or("file");
                let h = store.put_media_bytes(&bytes, url, name)?;
                if h.replaced.is_some() {
                    ctx.issue(
                        IssueKind::MissingMedia,
                        Severity::Info,
                        Some(h.id),
                        "content at URL changed; earlier file kept",
                        serde_json::json!({ "url": url }),
                    )?;
                }
                ctx.item_status(h.id, ItemKind::Media, ItemStatus::Done)?;
            }
        }
        pause(sh).await;
    }
    Ok(())
}

/// Sitemap URLs from robots.txt and `/sitemap.xml`; indexes are followed 3 levels deep.
async fn sitemap_urls(
    client: &Client,
    start: &Url,
    robots: &Robots,
    cancel: &CancellationToken,
    delay: Duration,
) -> Vec<Url> {
    let mut queue: Vec<(String, u8)> = robots.sitemaps.iter().map(|s| (s.clone(), 0)).collect();
    if let Ok(u) = start.join("/sitemap.xml") {
        queue.push((u.to_string(), 0));
    }
    let (mut seen, mut pages) = (HashSet::new(), vec![]);
    while let Some((s, depth)) = queue.pop() {
        if cancel.is_cancelled() {
            break;
        }
        if !seen.insert(s.clone()) {
            continue;
        }
        let xml = get_text(client, &s, cancel).await;
        pause_for(cancel, delay).await;
        let Some(xml) = xml else {
            continue;
        };
        let (index, locs) = sitemap_locs(&xml);
        if index && depth < 3 {
            queue.extend(locs.into_iter().map(|l| (l, depth + 1)));
        } else {
            pages.extend(locs.iter().filter_map(|l| Url::parse(l).ok()));
        }
    }
    pages
}

fn enqueue(seen: &mut HashSet<UrlKey>, frontier: &mut Vec<Url>, mut u: Url) {
    u.set_fragment(None);
    if seen.insert(key_of(&u)) {
        frontier.push(u);
    }
}

async fn crawl(ctx: &StageCtx, opts: &Opts, extract: bool) -> Result<()> {
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()?;
    let media_client = new_media_client(USER_AGENT)?;
    let robots = match start_text(&client, &opts.start, "/robots.txt", &ctx.cancel).await {
        Some(t) => Robots::parse(&t),
        None => Robots::default(),
    };
    let site = site_of(&opts.start);
    let mut seen = HashSet::new();
    let mut frontier = vec![];
    enqueue(&mut seen, &mut frontier, opts.start.clone());
    if !ctx.cancel.is_cancelled() {
        for u in sitemap_urls(&client, &opts.start, &robots, &ctx.cancel, opts.delay).await {
            if site_of(&u) == site && opts.allows(&u) {
                enqueue(&mut seen, &mut frontier, u);
            }
        }
    }
    let concurrency = opts.concurrency;
    let max_pages = opts.max_pages;
    let sh = Arc::new(Shared {
        ctx: ctx.clone(),
        client,
        media_client,
        robots,
        sem: Semaphore::new(concurrency),
        extract,
        css_url: Regex::new(r#"url\(\s*['"]?([^'")]+)"#)?,
        opts: opts.clone(),
    });

    let (mut visited, mut done) = (0usize, 0u64);
    let mut platforms: BTreeMap<String, usize> = BTreeMap::new();
    let (mut builders, mut plugins) = (BTreeSet::new(), BTreeSet::new());
    let mut capped = false;
    while !frontier.is_empty() && !ctx.cancel.is_cancelled() {
        if let Some(max) = max_pages {
            let room = max.saturating_sub(visited);
            capped |= frontier.len() > room;
            frontier.truncate(room);
            if frontier.is_empty() {
                break;
            }
        }
        visited += frontier.len();
        let mut set = JoinSet::new();
        for u in std::mem::take(&mut frontier) {
            set.spawn(visit(sh.clone(), u));
        }
        while let Some(r) = set.join_next().await {
            let Some(o) = r?? else { continue };
            done += 1;
            ctx.progress(done, None, "pages");
            seen.extend(o.landed);
            for l in o.links {
                if site_of(&l) == site && sh.opts.allows(&l) {
                    enqueue(&mut seen, &mut frontier, l);
                }
            }
            if let Some(d) = o.detected {
                if let Some(p) = d.platform {
                    *platforms.entry(p).or_default() += 1;
                }
                builders.extend(d.builders);
                plugins.extend(d.plugins);
            }
        }
    }
    if let (true, Some(max)) = (capped, max_pages) {
        ctx.issue(
            IssueKind::AccessGap,
            Severity::Warning,
            None,
            format!("crawl stopped at max_pages ({max})"),
            serde_json::json!({ "max_pages": max }),
        )?;
    }
    if !extract {
        let platform = platforms
            .into_iter()
            .min_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)))
            .map(|(p, _)| p);
        let detected = Detected {
            platform,
            builders: builders.into_iter().collect(),
            plugins: plugins.into_iter().collect(),
        };
        let raw = ctx.project.raw_dir(NAME);
        std::fs::create_dir_all(&raw)?;
        std::fs::write(
            raw.join("detect.json"),
            serde_json::to_vec_pretty(&detected)?,
        )?;
    }
    Ok(())
}

async fn start_text(
    client: &Client,
    start: &Url,
    path: &str,
    cancel: &CancellationToken,
) -> Option<String> {
    get_text(client, start.join(path).ok()?.as_str(), cancel).await
}
