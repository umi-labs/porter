//! HTML -> neutral rich text, shared by every source. Links and images resolve through the store.

use crate::store::Store;
use crate::url::normalise_url;
use ::url::Url;
use ego_tree::NodeRef;
use porter_model::{
    Align, FieldValue, Inline, IssueKind, ItemId, LinkTarget, Marks, RichText, RouteTarget,
    RtBlock, Severity,
};
use regex::Regex;
use scraper::{ElementRef, Html, Node, node::Element};
use serde_json::{Value, json};
use std::sync::LazyLock;

/// An issue found while converting, for the caller to attach to the item it is converting.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingIssue {
    pub kind: IssueKind,
    pub severity: Severity,
    pub message: String,
    pub details: Value,
}

pub struct HtmlCtx<'a> {
    pub store: &'a Store,
    /// Hosts whose links are internal; `www.` and case don't matter.
    pub site_hosts: Vec<String>,
    pub issues: &'a mut Vec<PendingIssue>,
    /// Other URLs a source serves the same file under (e.g. a resized copy's original), tried
    /// in order when a media URL itself isn't stored. `no_alternates` for none.
    pub media_alternates: fn(&str) -> Vec<String>,
}

/// For `HtmlCtx::media_alternates`: no other URLs.
pub fn no_alternates(_: &str) -> Vec<String> {
    vec![]
}

impl HtmlCtx<'_> {
    pub fn issue(&mut self, kind: IssueKind, severity: Severity, message: String, details: Value) {
        self.issues.push(PendingIssue {
            kind,
            severity,
            message,
            details,
        });
    }

    /// `https://<first site host>/`, the base relative URLs resolve against.
    fn base(&self) -> Option<Url> {
        Url::parse(&format!("https://{}/", self.site_hosts.first()?)).ok()
    }

    /// Absolute form of `href`: relative ones resolve against the first site host.
    fn absolute(&self, href: &str) -> Option<String> {
        if href.starts_with("//") {
            return Some(format!("https:{href}"));
        }
        match Url::parse(href) {
            Ok(u) => Some(u.to_string()),
            Err(_) => Some(self.base()?.join(href).ok()?.to_string()),
        }
    }

    /// True for relative URLs and URLs on one of `site_hosts`.
    pub fn is_internal(&self, href: &str) -> bool {
        if href.starts_with("//") {
            return self.on_site(&format!("https:{href}"));
        }
        match Url::parse(href) {
            Ok(u) => matches!(u.scheme(), "http" | "https") && self.on_site(href),
            Err(_) => true,
        }
    }

    fn on_site(&self, abs: &str) -> bool {
        let Some(host) = host_key(abs) else {
            return false;
        };
        self.site_hosts
            .iter()
            .any(|h| host_key(&format!("https://{h}/")).is_some_and(|k| k == host))
    }

    /// Where a link goes: internal links resolve through the store's routes, documents and
    /// media; an internal link that matches nothing stays a URL with an `unresolved-link`
    /// warning.
    pub fn link_target(&mut self, href: &str) -> LinkTarget {
        self.link(href).0
    }

    /// `link_target`, plus the `#fragment` when the link resolved to a document or media.
    pub fn link(&mut self, href: &str) -> (LinkTarget, Option<String>) {
        let h = clean_href(href);
        if h.is_empty() || h.starts_with('#') || !self.is_internal(&h) {
            return (LinkTarget::Url(h), None);
        }
        let fragment = h
            .split_once('#')
            .map(|(_, f)| f.to_string())
            .filter(|f| !f.is_empty());
        let found = match self.absolute(&h) {
            Some(abs) => self.store.lookup_url(&abs),
            None => Ok(None),
        };
        match found {
            Ok(Some(RouteTarget::Document(id))) => (LinkTarget::Doc(id), fragment),
            Ok(Some(RouteTarget::Media(id))) => (LinkTarget::Media(id), fragment),
            other => {
                let why = match other {
                    Err(e) => format!("lookup failed: {e:#}"),
                    Ok(Some(RouteTarget::Redirect(_))) => "it is a redirect".into(),
                    _ => "no page or file has that URL".into(),
                };
                self.issue(
                    IssueKind::UnresolvedLink,
                    Severity::Warning,
                    format!("internal link {h} does not resolve: {why}"),
                    json!({ "href": h }),
                );
                (LinkTarget::Url(h), None)
            }
        }
    }

    /// The media item for the first of `urls` the store knows. No issue.
    pub fn find_media<'u>(&self, urls: impl IntoIterator<Item = &'u str>) -> Option<ItemId> {
        let urls: Vec<String> = urls
            .into_iter()
            .filter_map(|u| self.absolute(u.trim()))
            .collect();
        let get = |u: &str| self.store.media_by_url(u).ok().flatten();
        urls.iter().find_map(|u| get(u)).or_else(|| {
            urls.iter()
                .flat_map(|u| (self.media_alternates)(u))
                .find_map(|u| get(&u))
        })
    }

    /// `find_media` over a URL, with a `missing-media` warning when it isn't stored.
    pub fn media_or_issue(&mut self, url: &str, srcset: Option<&str>) -> Option<ItemId> {
        let candidates = std::iter::once(url).chain(
            srcset
                .unwrap_or("")
                .split(',')
                .filter_map(|c| c.split_whitespace().next()),
        );
        let found = self.find_media(candidates);
        if found.is_none() {
            self.issue(
                IssueKind::MissingMedia,
                Severity::Warning,
                format!("{url} is not in the media library"),
                json!({ "src": url }),
            );
        }
        found
    }

    /// A link as a field value: a document or media reference when it resolves, else text.
    pub fn link_value(&mut self, href: &str) -> FieldValue {
        match self.link_target(href) {
            LinkTarget::Doc(id) => FieldValue::Doc(id),
            LinkTarget::Media(id) => FieldValue::Media(id),
            LinkTarget::Url(u) => FieldValue::Text(u),
        }
    }
}

/// Host part of a URL's key (`www.` and case dropped).
fn host_key(abs: &str) -> Option<String> {
    let k = normalise_url(abs, None)?.0;
    Some(k[..k.find('/').unwrap_or(k.len())].to_string())
}

/// No spaces in URLs (destinations reject them); phone numbers lose theirs.
fn clean_href(href: &str) -> String {
    let h = href.trim();
    if h.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("tel:")) {
        h.chars().filter(|c| !c.is_whitespace()).collect()
    } else {
        h.replace(char::is_whitespace, "%20")
    }
}

static ENTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"&(?:#([0-9]{1,7})|#[xX]([0-9a-fA-F]{1,6})|([A-Za-z][A-Za-z0-9]{1,31}));")
        .expect("valid regex")
});

/// Decodes well-formed character references (`&amp;`, `&#8217;`, `&#x2019;`). The
/// semicolon is required, so a query string like `?a=1&region=uk` is left alone.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    ENTITY
        .replace_all(s, |c: &regex::Captures| {
            let num = c.get(1).and_then(|d| d.as_str().parse().ok()).or_else(|| {
                c.get(2)
                    .and_then(|h| u32::from_str_radix(h.as_str(), 16).ok())
            });
            if let Some(n) = num {
                return char::from_u32(n)
                    .filter(|&ch| ch != '\0')
                    .map_or_else(|| c[0].to_string(), String::from);
            }
            let name = &c[3];
            match name {
                "amp" => return "&".into(),
                "lt" => return "<".into(),
                "gt" => return ">".into(),
                "quot" => return "\"".into(),
                "apos" => return "'".into(),
                "nbsp" => return "\u{a0}".into(),
                _ => {}
            }
            // The parser knows every named reference; a full match consumes the ';', a
            // legacy prefix match (`&notit;` -> `¬it;`) doesn't, so it is rejected.
            let text: String = Html::parse_fragment(&format!("<p>{}</p>", &c[0]))
                .root_element()
                .text()
                .collect();
            if text != c[0] && (!text.ends_with(';') || name == "semi") {
                text
            } else {
                c[0].to_string()
            }
        })
        .into_owned()
}

/// Converters stop recursing here; anything deeper is kept as raw source.
pub const MAX_DEPTH: usize = 256;

/// The one warning for content nested deeper than `MAX_DEPTH`.
pub fn too_deep_issue(ctx: &mut HtmlCtx) {
    ctx.issue(
        IssueKind::UnsupportedBlock,
        Severity::Warning,
        format!("nesting deeper than {MAX_DEPTH}; the rest is kept as raw source"),
        json!({ "max_depth": MAX_DEPTH }),
    );
}

static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid regex"));
static ALIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)text-align:\s*(left|center|right|justify)").expect("valid regex")
});

/// Elements kept as HTML: rich text can't hold them.
const RAW: &[&str] = &[
    "table", "iframe", "script", "form", "video", "audio", "object", "embed", "style", "svg",
    "noscript", "canvas", "select", "input", "textarea", "button", "map", "math", "dl",
];
/// Containers whose children are converted in their place.
const WRAPPERS: &[&str] = &[
    "div", "section", "article", "aside", "header", "footer", "main", "nav", "center", "html",
    "body", "hgroup", "address", "span", "font",
];
/// Elements that sit inside a paragraph.
const INLINE: &[&str] = &[
    "a", "b", "strong", "i", "em", "u", "ins", "s", "del", "strike", "span", "br", "sup", "sub",
    "small", "big", "abbr", "acronym", "code", "kbd", "tt", "samp", "var", "font", "mark", "cite",
    "q", "dfn", "time", "label", "bdi", "bdo", "data", "wbr", "img",
];

fn align_of(e: &Element) -> Option<Align> {
    let style = e.attr("style").unwrap_or("");
    let class = e.attr("class").unwrap_or("");
    let a = ALIGN
        .captures(style)
        .map(|c| c[1].to_ascii_lowercase())
        .or_else(|| e.attr("align").map(str::to_ascii_lowercase))
        .or_else(|| {
            class
                .split_whitespace()
                .find_map(|c| c.strip_prefix("has-text-align-"))
                .map(str::to_string)
        })?;
    Some(match a.as_str() {
        "left" => Align::Left,
        "center" => Align::Center,
        "right" => Align::Right,
        "justify" => Align::Justify,
        _ => return None,
    })
}

fn img_align(e: &Element) -> Align {
    let class = e.attr("class").unwrap_or("");
    for c in class.split_whitespace() {
        match c {
            "alignleft" => return Align::Left,
            "aligncenter" => return Align::Center,
            "alignright" => return Align::Right,
            _ => {}
        }
    }
    Align::None
}

fn outer_html(n: NodeRef<'_, Node>) -> String {
    ElementRef::wrap(n).map(|e| e.html()).unwrap_or_default()
}

fn element(n: NodeRef<'_, Node>) -> Option<&Element> {
    match n.value() {
        Node::Element(e) => Some(e),
        _ => None,
    }
}

fn is_inline(n: NodeRef<'_, Node>) -> bool {
    match n.value() {
        Node::Text(_) | Node::Comment(_) => true,
        // A top-level image is its own block; images inside text are lifted out after it.
        Node::Element(e) => INLINE.contains(&e.name()) && e.name() != "img",
        _ => false,
    }
}

/// Drops edge line breaks and whitespace, empty text, and merges neighbouring text runs.
fn trim_edges(mut v: Vec<Inline>) -> Vec<Inline> {
    while matches!(v.first(), Some(Inline::Break)) {
        v.remove(0);
    }
    while matches!(v.last(), Some(Inline::Break)) {
        v.pop();
    }
    if let Some(Inline::Text { text, .. }) = v.first_mut() {
        *text = text.trim_start().to_string();
    }
    if let Some(Inline::Text { text, .. }) = v.last_mut() {
        *text = text.trim_end().to_string();
    }
    let mut out: Vec<Inline> = Vec::with_capacity(v.len());
    for mut i in v {
        // Whitespace beside a line break doesn't render.
        if let (Some(Inline::Break), Inline::Text { text, .. }) = (out.last(), &mut i) {
            *text = text.trim_start().to_string();
        }
        if let (Some(Inline::Text { text, .. }), Inline::Break) = (out.last_mut(), &i) {
            *text = text.trim_end().to_string();
        }
        match (out.last_mut(), i) {
            (_, Inline::Text { text, .. }) if text.is_empty() => {}
            (Some(Inline::Text { text: a, marks: ma }), Inline::Text { text: b, marks: mb })
                if *ma == mb =>
            {
                // Two collapsed runs meeting keep a single space.
                if a.ends_with(' ') && b.starts_with(' ') {
                    a.push_str(&b[1..]);
                } else {
                    a.push_str(&b);
                }
            }
            (_, i) => out.push(i),
        }
    }
    out.retain(|i| !matches!(i, Inline::Text { text, .. } if text.is_empty()));
    out
}

fn has_content(v: &[Inline]) -> bool {
    v.iter().any(|i| match i {
        Inline::Link { .. } => true,
        Inline::Text { text, .. } => !text.trim().is_empty(),
        Inline::Break => false,
    })
}

struct Conv<'c, 'a> {
    ctx: &'c mut HtmlCtx<'a>,
    depth: usize,
    flagged: bool,
}

impl Conv<'_, '_> {
    /// True at the depth cap; warns once per conversion.
    fn too_deep(&mut self) -> bool {
        if self.depth < MAX_DEPTH {
            return false;
        }
        if !self.flagged {
            self.flagged = true;
            too_deep_issue(self.ctx);
        }
        true
    }

    fn image(&mut self, n: NodeRef<'_, Node>, e: &Element) -> RtBlock {
        let src = e.attr("src").unwrap_or("");
        match self.ctx.media_or_issue(src, e.attr("srcset")) {
            Some(media) => RtBlock::Media {
                media,
                caption: None,
                align: img_align(e),
                width: e.attr("width").and_then(|w| w.trim().parse().ok()),
                link: None,
            },
            None => RtBlock::Raw {
                html: outer_html(n),
            },
        }
    }

    fn inlines(
        &mut self,
        n: NodeRef<'_, Node>,
        marks: Marks,
        lifted: &mut Vec<RtBlock>,
    ) -> Vec<Inline> {
        let e = match n.value() {
            Node::Text(t) => {
                let text = WS.replace_all(t, " ").into_owned();
                return if text.is_empty() {
                    vec![]
                } else {
                    vec![Inline::Text { text, marks }]
                };
            }
            Node::Element(e) => e,
            _ => return vec![],
        };
        if self.too_deep() {
            lifted.push(RtBlock::Raw {
                html: outer_html(n),
            });
            return vec![];
        }
        let mut m = marks;
        match e.name() {
            "br" => return vec![Inline::Break],
            "img" => {
                let b = self.image(n, e);
                lifted.push(b);
                return vec![];
            }
            name if RAW.contains(&name) => {
                lifted.push(RtBlock::Raw {
                    html: outer_html(n),
                });
                return vec![];
            }
            "strong" | "b" => m.bold = true,
            "em" | "i" => m.italic = true,
            "u" | "ins" => m.underline = true,
            "s" | "del" | "strike" => m.strike = true,
            "code" | "kbd" | "tt" | "samp" => m.code = true,
            "sub" => m.sub = true,
            "sup" => m.sup = true,
            _ => {}
        }
        let first_lifted = lifted.len();
        self.depth += 1;
        let kids: Vec<Inline> = n
            .children()
            .flat_map(|k| self.inlines(k, m, lifted))
            .collect();
        self.depth -= 1;
        if e.name() != "a" {
            return kids;
        }
        let href = e.attr("href").unwrap_or("").trim();
        let image = lifted[first_lifted..]
            .iter()
            .any(|b| matches!(b, RtBlock::Media { link: None, .. }));
        if href.is_empty() || !(has_content(&kids) || image) {
            return kids;
        }
        let (target, fragment) = self.ctx.link(href);
        // Images the link wraps keep it.
        for b in &mut lifted[first_lifted..] {
            if let RtBlock::Media { link: l @ None, .. } = b {
                *l = Some(target.clone());
            }
        }
        if !has_content(&kids) {
            return kids;
        }
        vec![Inline::Link {
            target,
            new_tab: e.attr("target") == Some("_blank"),
            inlines: kids,
            fragment,
        }]
    }

    /// Inline content of `n`'s children as one run, lifted blocks after it.
    fn run(&mut self, nodes: Vec<NodeRef<'_, Node>>) -> (Vec<Inline>, Vec<RtBlock>) {
        let mut lifted = vec![];
        let v: Vec<Inline> = nodes
            .into_iter()
            .flat_map(|k| self.inlines(k, Marks::default(), &mut lifted))
            .collect();
        (trim_edges(v), lifted)
    }

    fn paragraph(&mut self, nodes: Vec<NodeRef<'_, Node>>, align: Align, out: &mut Vec<RtBlock>) {
        let (inlines, lifted) = self.run(nodes);
        if has_content(&inlines) {
            out.push(RtBlock::Paragraph { inlines, align });
        }
        out.extend(lifted);
    }

    /// Loose inline content between blocks becomes one paragraph per run.
    fn blocks<'n>(
        &mut self,
        nodes: impl Iterator<Item = NodeRef<'n, Node>>,
        align: Align,
        out: &mut Vec<RtBlock>,
    ) {
        let mut run = vec![];
        for n in nodes {
            if is_inline(n) {
                run.push(n);
            } else {
                self.paragraph(std::mem::take(&mut run), align, out);
                self.block(n, align, out);
            }
        }
        self.paragraph(run, align, out);
    }

    fn block(&mut self, n: NodeRef<'_, Node>, inherited: Align, out: &mut Vec<RtBlock>) {
        if element(n).is_none() {
            return;
        }
        if self.too_deep() {
            out.push(RtBlock::Raw {
                html: outer_html(n),
            });
            return;
        }
        self.depth += 1;
        match element(n) {
            // Wrappers are the common deep path: kept out of `element_block`'s big frame.
            Some(e) if WRAPPERS.contains(&e.name()) => {
                let align = align_of(e).unwrap_or(inherited);
                self.blocks(n.children(), align, out);
            }
            _ => self.element_block(n, inherited, out),
        }
        self.depth -= 1;
    }

    #[inline(never)]
    fn element_block(&mut self, n: NodeRef<'_, Node>, inherited: Align, out: &mut Vec<RtBlock>) {
        let Some(e) = element(n) else {
            return;
        };
        let align = align_of(e).unwrap_or(inherited);
        let name = e.name();
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let (inlines, lifted) = self.run(n.children().collect());
                if has_content(&inlines) {
                    let level = name[1..].parse().unwrap_or(2);
                    out.push(RtBlock::Heading {
                        level,
                        inlines,
                        align,
                    });
                }
                out.extend(lifted);
            }
            "ul" | "ol" => {
                let items = n
                    .children()
                    .filter(|c| element(*c).is_some_and(|e| e.name() == "li"))
                    .map(|li| {
                        let mut v = vec![];
                        self.blocks(li.children(), Align::None, &mut v);
                        v
                    })
                    .collect();
                out.push(RtBlock::List {
                    ordered: name == "ol",
                    start: e
                        .attr("start")
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(1),
                    items,
                });
            }
            "blockquote" => {
                let mut blocks = vec![];
                self.blocks(n.children(), align, &mut blocks);
                if !blocks.is_empty() {
                    out.push(RtBlock::Quote { blocks });
                }
            }
            "hr" => out.push(RtBlock::Rule),
            "pre" => out.push(RtBlock::Code {
                lang: None,
                text: ElementRef::wrap(n)
                    .map(|e| e.text().collect())
                    .unwrap_or_default(),
            }),
            "img" => out.push(self.image(n, e)),
            "figure" => {
                let mut inner = vec![];
                let mut caption = None;
                for c in n.children() {
                    if element(c).is_some_and(|e| e.name() == "figcaption") {
                        let (inlines, lifted) = self.run(c.children().collect());
                        if has_content(&inlines) {
                            caption = Some(RichText {
                                blocks: vec![RtBlock::Paragraph {
                                    inlines,
                                    align: Align::None,
                                }],
                            });
                        }
                        inner.extend(lifted);
                    } else {
                        self.blocks(std::iter::once(c), align, &mut inner);
                    }
                }
                match (
                    inner
                        .iter_mut()
                        .rev()
                        .find(|b| matches!(b, RtBlock::Media { .. })),
                    caption,
                ) {
                    (Some(RtBlock::Media { caption: slot, .. }), Some(c)) => *slot = Some(c),
                    (_, Some(c)) => inner.extend(c.blocks),
                    _ => {}
                }
                out.extend(inner);
            }
            "p" | "li" | "dt" | "dd" | "figcaption" | "caption" => {
                self.paragraph(n.children().collect(), align, out)
            }
            name if WRAPPERS.contains(&name) => self.blocks(n.children(), align, out),
            _ => out.push(RtBlock::Raw {
                html: outer_html(n),
            }),
        }
    }
}

/// HTML as stored (already through any platform filters) -> rich text. Tables, iframes,
/// scripts, forms and unknown elements are kept as `Raw` HTML.
pub fn html_to_richtext(html: &str, ctx: &mut HtmlCtx) -> RichText {
    let frag = Html::parse_fragment(html);
    let mut blocks = vec![];
    Conv {
        ctx,
        depth: 0,
        flagged: false,
    }
    .blocks(frag.root_element().children(), Align::None, &mut blocks);
    RichText { blocks }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use porter_model::{
        Inline, ItemId, ItemKind, LinkTarget, Route, RouteTarget, RtBlock, SourceRef,
    };

    pub(crate) struct Fixture {
        _dir: tempfile::TempDir,
        pub store: Store,
        pub about: ItemId,
        pub media: ItemId,
    }

    /// A store with a page at /about/ (a route) and /rooms/garden/ (a document url), and
    /// two images.
    pub(crate) fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("p.db"), dir.path().join("media")).unwrap();
        let src = |id: &str, url: &str| SourceRef {
            connector: "t".into(),
            source_id: id.into(),
            url: Some(url.into()),
        };
        let about = ItemId::new(ItemKind::Document, &src("1", ""));
        let rs = src("r1", "https://example.test/about/");
        store
            .put_item(
                ItemKind::Route,
                ItemId::new(ItemKind::Route, &rs),
                &rs,
                &Route {
                    url: "https://example.test/about/".into(),
                    target: RouteTarget::Document(about),
                    http_status: Some(200),
                    title: None,
                },
            )
            .unwrap();
        let ds = src("2", "https://example.test/rooms/garden/");
        store
            .put_item(
                ItemKind::Document,
                ItemId::new(ItemKind::Document, &ds),
                &ds,
                &serde_json::json!({"label": "page"}),
            )
            .unwrap();
        let media = store
            .put_media_bytes(
                b"lamp",
                "https://example.test/files/2020/01/lamp.jpg",
                "lamp.jpg",
            )
            .unwrap()
            .id;
        store
            .put_media_bytes(
                b"room",
                "https://example.test/files/2021/05/room.jpg",
                "room.jpg",
            )
            .unwrap();
        store
            .put_media_bytes(
                b"door",
                "https://example.test/files/2019/03/door.png",
                "door.png",
            )
            .unwrap();
        Fixture {
            _dir: dir,
            store,
            about,
            media,
        }
    }

    pub(crate) fn ctx<'a>(f: &'a Fixture, issues: &'a mut Vec<PendingIssue>) -> HtmlCtx<'a> {
        HtmlCtx {
            store: &f.store,
            site_hosts: vec!["www.example.test".into()],
            issues,
            media_alternates: no_alternates,
        }
    }

    fn links(rt: &RichText) -> Vec<LinkTarget> {
        let mut out = vec![];
        for b in &rt.blocks {
            if let RtBlock::Paragraph { inlines, .. } = b {
                for i in inlines {
                    if let Inline::Link { target, .. } = i {
                        out.push(target.clone());
                    }
                }
            }
        }
        out
    }

    #[test]
    fn html_links_resolve_or_flag() {
        let f = fixture();
        let mut issues = vec![];
        let rt = html_to_richtext(
            r#"<p><a href="https://EXAMPLE.test/about">a</a> <a href="/missing/">b</a>
               <a href="https://elsewhere.test/x">c</a> <a href="mailto:x@example.test">d</a>
               <a href="/rooms/garden/">e</a></p>"#,
            &mut ctx(&f, &mut issues),
        );
        let l = links(&rt);
        assert_eq!(l[0], LinkTarget::Doc(f.about));
        assert_eq!(l[1], LinkTarget::Url("/missing/".into()));
        assert_eq!(l[2], LinkTarget::Url("https://elsewhere.test/x".into()));
        assert_eq!(l[3], LinkTarget::Url("mailto:x@example.test".into()));
        assert!(matches!(l[4], LinkTarget::Doc(_)));
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].kind, IssueKind::UnresolvedLink);
        assert_eq!(issues[0].severity, Severity::Warning);
    }

    #[test]
    fn html_blocks_marks_and_media() {
        let f = fixture();
        let mut issues = vec![];
        let rt = html_to_richtext(
            r#"<h3 style="text-align: center">Hi <em>there</em></h3>
               <p>One <strong>two</strong><br>three<img src="/files/2020/01/lamp.jpg"></p>
               <ul><li>a</li><li>b<ol><li>c</li></ol></li></ul>
               <blockquote><p>q</p></blockquote><hr>
               <table><tr><td>x</td></tr></table>
               <img src="https://example.test/nope.jpg"><custom-thing>z</custom-thing>"#,
            &mut ctx(&f, &mut issues),
        );
        let kinds: Vec<_> = rt
            .blocks
            .iter()
            .map(|b| {
                serde_json::to_value(b).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "heading",
                "paragraph",
                "media",
                "list",
                "quote",
                "rule",
                "raw",
                "raw",
                "raw"
            ]
        );
        assert!(matches!(
            &rt.blocks[0],
            RtBlock::Heading {
                level: 3,
                align: porter_model::Align::Center,
                ..
            }
        ));
        assert!(matches!(&rt.blocks[2], RtBlock::Media { media, .. } if *media == f.media));
        assert_eq!(rt.plain_text().lines().next(), Some("Hi there"));
        assert!(rt.plain_text().contains("One two\nthree"));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::MissingMedia);
    }

    #[test]
    fn non_ascii_href_no_panic() {
        let f = fixture();
        let mut issues = vec![];
        let rt = html_to_richtext(
            r#"<p><a href="/Русский/">Русский</a> <a href="Ελληνικά">Ελληνικά</a>
               <a href="https://example.test/العربية/">العربية</a> <a href="😀">😀 emoji</a>
               <a href="тел:1">x</a> <a href=" é ">é</a></p>"#,
            &mut ctx(&f, &mut issues),
        );
        assert_eq!(links(&rt).len(), 6);
        assert!(
            rt.plain_text()
                .contains("Русский Ελληνικά العربية 😀 emoji")
        );
    }

    #[test]
    fn linked_image_keeps_link() {
        let f = fixture();
        let mut issues = vec![];
        let rt = html_to_richtext(
            r#"<p><a href="/about/"><img src="/files/2020/01/lamp.jpg"></a></p>"#,
            &mut ctx(&f, &mut issues),
        );
        assert_eq!(rt.blocks.len(), 1, "{rt:?}");
        assert!(matches!(
            &rt.blocks[0],
            RtBlock::Media { link: Some(LinkTarget::Doc(id)), .. } if *id == f.about
        ));
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn fragment_kept_on_doc_link() {
        let f = fixture();
        let mut issues = vec![];
        let rt = html_to_richtext(
            r##"<p><a href="/about/#team">a</a> <a href="#top">b</a></p>"##,
            &mut ctx(&f, &mut issues),
        );
        let RtBlock::Paragraph { inlines, .. } = &rt.blocks[0] else {
            panic!()
        };
        assert!(matches!(
            &inlines[0],
            Inline::Link { target: LinkTarget::Doc(id), fragment: Some(fr), .. }
                if *id == f.about && fr == "team"
        ));
        assert!(matches!(
            &inlines[2],
            Inline::Link { target: LinkTarget::Url(u), fragment: None, .. } if u == "#top"
        ));
    }
}
