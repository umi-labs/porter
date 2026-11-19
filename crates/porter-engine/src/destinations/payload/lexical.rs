//! Neutral rich text -> Payload Lexical JSON (the value of a `richText` field).

use crate::html::PendingIssue;
use porter_model::{
    Align, Inline, IssueKind, ItemId, LinkTarget, Marks, RichText, RtBlock, Severity,
};
use regex::Regex;
use scraper::Html;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Where an item landed in the destination.
#[derive(Clone, Debug, PartialEq)]
pub struct DestRef {
    pub collection: String,
    pub id: Value,
    /// The document's new URL path, when known.
    pub path: Option<String>,
}

/// Item -> destination lookup, filled while loading.
pub trait IdMap {
    fn get(&self, id: ItemId) -> Option<DestRef>;
    /// The item's old (source) URL: what an unresolved link degrades to.
    fn old_url(&self, _id: ItemId) -> Option<String> {
        None
    }
}

/// In-memory `IdMap`.
#[derive(Default, Debug, Clone)]
pub struct MemIdMap {
    pub ids: HashMap<ItemId, DestRef>,
    pub urls: HashMap<ItemId, String>,
}

impl IdMap for MemIdMap {
    fn get(&self, id: ItemId) -> Option<DestRef> {
        self.ids.get(&id).cloned()
    }
    fn old_url(&self, id: ItemId) -> Option<String> {
        self.urls.get(&id).cloned()
    }
}

/// `{"root": …}` for `rt`, plus the issues met converting it. `raw_block` is the mapping's
/// block slug for raw HTML, embeds, tables and code; without it they degrade to text.
pub fn to_lexical(
    rt: &RichText,
    ids: &dyn IdMap,
    raw_block: Option<&str>,
) -> (Value, Vec<PendingIssue>) {
    let mut w = W {
        ids,
        raw_block,
        issues: vec![],
    };
    let children = w.blocks(&rt.blocks);
    let root = json!({"root": {"type": "root", "format": "", "indent": 0, "version": 1,
        "direction": "ltr", "children": children}});
    (root, w.issues)
}

/// `rt` as HTML, for raw blocks; issues go to `issues`.
pub(super) fn to_html(rt: &RichText, ids: &dyn IdMap, issues: &mut Vec<PendingIssue>) -> String {
    let mut w = W {
        ids,
        raw_block: None,
        issues: std::mem::take(issues),
    };
    let mut out = String::new();
    w.html_blocks(&rt.blocks, &mut out);
    *issues = w.issues;
    out
}

enum Resolved {
    Internal(DestRef),
    Url(String),
}

struct W<'a> {
    ids: &'a dyn IdMap,
    raw_block: Option<&'a str>,
    issues: Vec<PendingIssue>,
}

/// An element node; `extra` (an object) adds or overrides keys.
fn el(ty: &str, children: Vec<Value>, extra: Value) -> Value {
    let mut v = json!({"type": ty, "format": "", "indent": 0, "version": 1,
        "direction": "ltr", "children": children});
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    v
}

fn text(s: &str, format: u32) -> Value {
    json!({"type": "text", "text": s, "format": format, "detail": 0, "mode": "normal",
        "style": "", "version": 1})
}

fn linebreak() -> Value {
    json!({"type": "linebreak", "version": 1})
}

fn paragraph(children: Vec<Value>, align: Align) -> Value {
    el(
        "paragraph",
        children,
        json!({"format": align_str(align), "textFormat": 0, "textStyle": ""}),
    )
}

fn align_str(a: Align) -> &'static str {
    match a {
        Align::None => "",
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
        Align::Justify => "justify",
    }
}

fn bitmask(m: &Marks) -> u32 {
    [
        m.bold,
        m.italic,
        m.strike,
        m.underline,
        m.code,
        m.sub,
        m.sup,
    ]
    .iter()
    .enumerate()
    .filter(|(_, on)| **on)
    .map(|(i, _)| 1 << i)
    .sum()
}

fn custom(url: &str, new_tab: bool) -> Value {
    json!({"linkType": "custom", "url": url.replace(' ', "%20"), "newTab": new_tab})
}

/// One paragraph per non-empty line.
fn text_paragraphs(s: &str) -> Vec<Value> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| paragraph(vec![text(l, 0)], Align::None))
        .collect()
}

static BLOCK_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</(p|div|h[1-6]|li|tr|blockquote|pre|figure|section)>|<br\s*/?>")
        .expect("valid regex")
});

/// Text content of an HTML fragment, block ends as newlines.
fn html_text(html: &str) -> String {
    let h = BLOCK_END.replace_all(html, "$0\n");
    Html::parse_fragment(&h).root_element().text().collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn code_html(lang: &Option<String>, s: &str) -> String {
    match lang {
        Some(l) => format!(
            "<pre><code class=\"language-{}\">{}</code></pre>",
            esc(l),
            esc(s)
        ),
        None => format!("<pre><code>{}</code></pre>", esc(s)),
    }
}

fn embed_html(provider: &Option<String>, url: &str) -> String {
    let p = provider
        .as_deref()
        .map(|p| format!(" data-provider=\"{}\"", esc(p)))
        .unwrap_or_default();
    format!(
        "<figure class=\"embed\"{p}><a href=\"{u}\">{u}</a></figure>",
        u = esc(url)
    )
}

fn block_name(b: &RtBlock) -> &'static str {
    match b {
        RtBlock::Code { .. } => "code",
        RtBlock::Table { .. } => "table",
        RtBlock::Embed { .. } => "embed",
        RtBlock::Raw { .. } => "raw HTML",
        RtBlock::Media { .. } => "image",
        _ => "block",
    }
}

impl W<'_> {
    fn issue(&mut self, kind: IssueKind, severity: Severity, message: String, details: Value) {
        self.issues.push(PendingIssue {
            kind,
            severity,
            message,
            details,
        });
    }

    /// Where a link goes. Unresolved items degrade to their old URL (or `None`), reported.
    fn resolve(&mut self, target: &LinkTarget, fragment: Option<&str>) -> Option<Resolved> {
        let id = match target {
            LinkTarget::Url(u) => return Some(Resolved::Url(u.clone())),
            LinkTarget::Doc(id) | LinkTarget::Media(id) => *id,
        };
        let frag = fragment.map(|f| format!("#{f}")).unwrap_or_default();
        match self.ids.get(id) {
            Some(d) => match (fragment, &d.path) {
                (None, _) => Some(Resolved::Internal(d)),
                (Some(_), Some(p)) => Some(Resolved::Url(format!("{p}{frag}"))),
                (Some(_), None) => {
                    self.issue(
                        IssueKind::UnresolvedLink,
                        Severity::Info,
                        format!("anchor {frag} dropped: the target's new path is not known"),
                        json!({"item": id, "fragment": fragment}),
                    );
                    Some(Resolved::Internal(d))
                }
            },
            None => {
                let old = self.ids.old_url(id);
                let message = match &old {
                    Some(u) => format!("link target {id} was not loaded; linking to old URL {u}"),
                    None => format!("link target {id} was not loaded and has no URL; link removed"),
                };
                self.issue(
                    IssueKind::UnresolvedRelation,
                    Severity::Warning,
                    message,
                    json!({"item": id, "url": old}),
                );
                old.map(|u| Resolved::Url(format!("{u}{frag}")))
            }
        }
    }

    fn inlines(&mut self, inlines: &[Inline]) -> Vec<Value> {
        let mut out = vec![];
        for i in inlines {
            match i {
                Inline::Text { text: t, marks } => out.push(text(t, bitmask(marks))),
                Inline::Break => out.push(linebreak()),
                Inline::Link {
                    target,
                    new_tab,
                    inlines,
                    fragment,
                } => {
                    let kids = self.inlines(inlines);
                    let fields = match self.resolve(target, fragment.as_deref()) {
                        Some(Resolved::Url(u)) => custom(&u, *new_tab),
                        Some(Resolved::Internal(d)) => json!({"linkType": "internal",
                            "doc": {"relationTo": d.collection, "value": d.id}, "newTab": new_tab}),
                        None => {
                            out.extend(kids);
                            continue;
                        }
                    };
                    out.push(el("link", kids, json!({ "fields": fields })));
                }
            }
        }
        out
    }

    fn list(&mut self, ordered: bool, start: u32, items: &[Vec<RtBlock>]) -> Value {
        let children = items
            .iter()
            .enumerate()
            .map(|(i, item)| el("listitem", self.flow(item, true), json!({"value": i + 1})))
            .collect();
        let (list_type, tag) = if ordered {
            ("number", "ol")
        } else {
            ("bullet", "ul")
        };
        el(
            "list",
            children,
            json!({"listType": list_type, "tag": tag, "start": start}),
        )
    }

    /// Blocks as inline content (list items, quotes): blocks joined by line breaks,
    /// nested lists kept inside list items, anything else reduced to its text.
    fn flow(&mut self, blocks: &[RtBlock], in_list: bool) -> Vec<Value> {
        let mut out: Vec<Value> = vec![];
        for b in blocks {
            let nodes = match b {
                RtBlock::Paragraph { inlines, .. } | RtBlock::Heading { inlines, .. } => {
                    self.inlines(inlines)
                }
                RtBlock::List {
                    ordered,
                    start,
                    items,
                } if in_list => vec![self.list(*ordered, *start, items)],
                RtBlock::List { items, .. } => {
                    let all: Vec<RtBlock> = items.iter().flatten().cloned().collect();
                    self.flow(&all, false)
                }
                RtBlock::Quote { blocks } => self.flow(blocks, in_list),
                RtBlock::Rule => vec![],
                RtBlock::Embed { url, .. } => {
                    vec![el(
                        "link",
                        vec![text(url, 0)],
                        json!({"fields": custom(url, false)}),
                    )]
                }
                RtBlock::Media { media, .. } => {
                    self.issue(
                        IssueKind::UnsupportedBlock,
                        Severity::Warning,
                        "image inside a list or quote dropped: Lexical can't hold it there".into(),
                        json!({"item": media}),
                    );
                    vec![]
                }
                other => {
                    let s = match other {
                        RtBlock::Raw { html } => html_text(html),
                        _ => RichText {
                            blocks: vec![other.clone()],
                        }
                        .plain_text(),
                    };
                    self.issue(
                        IssueKind::UnsupportedBlock,
                        Severity::Warning,
                        format!("{} inside a list or quote kept as text", block_name(other)),
                        Value::Null,
                    );
                    let mut v = vec![];
                    for l in s.lines().filter(|l| !l.trim().is_empty()) {
                        if !v.is_empty() {
                            v.push(linebreak());
                        }
                        v.push(text(l, 0));
                    }
                    v
                }
            };
            let is_list = |v: Option<&Value>| v.is_some_and(|n| n["type"] == "list");
            if !out.is_empty()
                && !nodes.is_empty()
                && !is_list(out.last())
                && !is_list(nodes.first())
            {
                out.push(linebreak());
            }
            out.extend(nodes);
        }
        out
    }

    /// A raw block holding `html`, or else `fallback` (text) plus a warning. Only the
    /// branch taken runs, so links are resolved (and reported) once.
    fn raw(
        &mut self,
        what: &str,
        html: impl FnOnce(&mut Self) -> String,
        fallback: impl FnOnce(&mut Self) -> Vec<Value>,
    ) -> Vec<Value> {
        match self.raw_block {
            Some(slug) => vec![json!({"type": "block", "format": "", "version": 1,
                "fields": {"blockType": slug, "html": html(self)}})],
            None => {
                self.issue(
                    IssueKind::UnsupportedBlock,
                    Severity::Warning,
                    format!("{what} kept as text: set mapping.raw_block to keep its markup"),
                    json!({"block": what}),
                );
                fallback(self)
            }
        }
    }

    fn blocks(&mut self, blocks: &[RtBlock]) -> Vec<Value> {
        let mut out = vec![];
        for b in blocks {
            match b {
                RtBlock::Paragraph { inlines, align } => {
                    let kids = self.inlines(inlines);
                    out.push(paragraph(kids, *align));
                }
                RtBlock::Heading {
                    level,
                    inlines,
                    align,
                } => {
                    let kids = self.inlines(inlines);
                    out.push(el(
                        "heading",
                        kids,
                        json!({"tag": format!("h{}", level.clamp(&1, &6)), "format": align_str(*align)}),
                    ));
                }
                RtBlock::List {
                    ordered,
                    start,
                    items,
                } => out.push(self.list(*ordered, *start, items)),
                RtBlock::Quote { blocks } => {
                    let kids = self.flow(blocks, false);
                    out.push(el("quote", kids, json!({})));
                }
                RtBlock::Rule => out.push(json!({"type": "horizontalrule", "version": 1})),
                RtBlock::Media {
                    media,
                    caption,
                    align,
                    link,
                    ..
                } => {
                    out.extend(self.media(*media, *align, link.as_ref()));
                    if let Some(c) = caption {
                        out.extend(self.blocks(&c.blocks));
                    }
                }
                RtBlock::Code { lang, text } => {
                    let r = self.raw("code", |_| code_html(lang, text), |_| text_paragraphs(text));
                    out.extend(r);
                }
                RtBlock::Table { rows, .. } => {
                    let r = self.raw(
                        "table",
                        |w| {
                            let mut html = String::new();
                            w.html_blocks(std::slice::from_ref(b), &mut html);
                            html
                        },
                        |w| {
                            rows.iter()
                                .map(|row| {
                                    let mut kids = vec![];
                                    for cell in row {
                                        if !kids.is_empty() {
                                            kids.push(text("\t", 0));
                                        }
                                        kids.extend(w.flow(cell, false));
                                    }
                                    paragraph(kids, Align::None)
                                })
                                .collect()
                        },
                    );
                    out.extend(r);
                }
                RtBlock::Embed { provider, url } => {
                    let r = self.raw(
                        "embed",
                        |_| embed_html(provider, url),
                        |_| {
                            let link = el(
                                "link",
                                vec![text(url, 0)],
                                json!({"fields": custom(url, false)}),
                            );
                            vec![paragraph(vec![link], Align::None)]
                        },
                    );
                    out.extend(r);
                }
                RtBlock::Raw { html } => {
                    let r = self.raw(
                        "raw HTML",
                        |_| html.clone(),
                        |_| text_paragraphs(&html_text(html)),
                    );
                    out.extend(r);
                }
            }
        }
        out
    }

    fn media(&mut self, media: ItemId, align: Align, link: Option<&LinkTarget>) -> Vec<Value> {
        let Some(d) = self.ids.get(media) else {
            let old = self.ids.old_url(media);
            self.issue(
                IssueKind::UnresolvedRelation,
                Severity::Warning,
                format!("image {media} was not loaded"),
                json!({"item": media, "url": old}),
            );
            return old
                .map(|u| {
                    let l = el(
                        "link",
                        vec![text(&u, 0)],
                        json!({"fields": custom(&u, false)}),
                    );
                    paragraph(vec![l], Align::None)
                })
                .into_iter()
                .collect();
        };
        if let Some(l) = link {
            let to = match l {
                LinkTarget::Url(u) => u.clone(),
                LinkTarget::Doc(id) | LinkTarget::Media(id) => self
                    .ids
                    .get(*id)
                    .and_then(|d| d.path)
                    .or_else(|| self.ids.old_url(*id))
                    .unwrap_or_else(|| id.0.to_string()),
            };
            self.issue(
                IssueKind::UnsupportedBlock,
                Severity::Info,
                format!("image link to {to} dropped: Payload upload nodes can't carry links"),
                json!({"item": media, "link": to}),
            );
        }
        vec![
            json!({"type": "upload", "relationTo": d.collection, "value": d.id, "fields": {},
            "format": align_str(align), "version": 1}),
        ]
    }

    fn html_inlines(&mut self, inlines: &[Inline], out: &mut String) {
        for i in inlines {
            match i {
                Inline::Text { text, marks } => {
                    let tags: Vec<&str> = [
                        (marks.bold, "strong"),
                        (marks.italic, "em"),
                        (marks.underline, "u"),
                        (marks.strike, "s"),
                        (marks.code, "code"),
                        (marks.sub, "sub"),
                        (marks.sup, "sup"),
                    ]
                    .into_iter()
                    .filter_map(|(on, t)| on.then_some(t))
                    .collect();
                    tags.iter().for_each(|t| out.push_str(&format!("<{t}>")));
                    out.push_str(&esc(text));
                    tags.iter()
                        .rev()
                        .for_each(|t| out.push_str(&format!("</{t}>")));
                }
                Inline::Break => out.push_str("<br>"),
                Inline::Link {
                    target,
                    new_tab,
                    inlines,
                    fragment,
                } => {
                    let href = match self.resolve(target, fragment.as_deref()) {
                        Some(Resolved::Url(u)) => Some(u),
                        Some(Resolved::Internal(d)) => d.path,
                        None => None,
                    };
                    match href {
                        Some(h) => {
                            let tab = if *new_tab { " target=\"_blank\"" } else { "" };
                            out.push_str(&format!(
                                "<a href=\"{}\"{tab}>",
                                esc(&h.replace(' ', "%20"))
                            ));
                            self.html_inlines(inlines, out);
                            out.push_str("</a>");
                        }
                        None => self.html_inlines(inlines, out),
                    }
                }
            }
        }
    }

    fn html_blocks(&mut self, blocks: &[RtBlock], out: &mut String) {
        for b in blocks {
            match b {
                RtBlock::Paragraph { inlines, .. } => {
                    out.push_str("<p>");
                    self.html_inlines(inlines, out);
                    out.push_str("</p>");
                }
                RtBlock::Heading { level, inlines, .. } => {
                    let l = level.clamp(&1, &6);
                    out.push_str(&format!("<h{l}>"));
                    self.html_inlines(inlines, out);
                    out.push_str(&format!("</h{l}>"));
                }
                RtBlock::List { ordered, items, .. } => {
                    let t = if *ordered { "ol" } else { "ul" };
                    out.push_str(&format!("<{t}>"));
                    for item in items {
                        out.push_str("<li>");
                        self.html_blocks(item, out);
                        out.push_str("</li>");
                    }
                    out.push_str(&format!("</{t}>"));
                }
                RtBlock::Quote { blocks } => {
                    out.push_str("<blockquote>");
                    self.html_blocks(blocks, out);
                    out.push_str("</blockquote>");
                }
                RtBlock::Code { lang, text } => out.push_str(&code_html(lang, text)),
                RtBlock::Rule => out.push_str("<hr>"),
                RtBlock::Table { rows, header } => {
                    out.push_str("<table>");
                    for (r, row) in rows.iter().enumerate() {
                        let c = if *header && r == 0 { "th" } else { "td" };
                        out.push_str("<tr>");
                        for cell in row {
                            out.push_str(&format!("<{c}>"));
                            self.html_blocks(cell, out);
                            out.push_str(&format!("</{c}>"));
                        }
                        out.push_str("</tr>");
                    }
                    out.push_str("</table>");
                }
                RtBlock::Media { media, .. } => self.issue(
                    IssueKind::UnsupportedBlock,
                    Severity::Warning,
                    "image inside raw HTML dropped: it has no destination URL yet".into(),
                    json!({"item": media}),
                ),
                RtBlock::Embed { provider, url } => out.push_str(&embed_html(provider, url)),
                RtBlock::Raw { html } => out.push_str(html),
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use porter_model::{ItemKind, SourceRef};
    use serde_json::json;

    pub(crate) fn id(kind: ItemKind, s: &str) -> ItemId {
        ItemId::new(
            kind,
            &SourceRef {
                connector: "t".into(),
                source_id: s.into(),
                url: None,
            },
        )
    }

    pub(crate) fn txt(s: &str) -> Inline {
        Inline::Text {
            text: s.into(),
            marks: Marks::default(),
        }
    }

    fn para(inlines: Vec<Inline>) -> RtBlock {
        RtBlock::Paragraph {
            inlines,
            align: Align::None,
        }
    }

    /// Doc "about" -> pages/7 at /about, media "pic" -> media/3, doc "gone" unresolved
    /// with an old URL.
    pub(crate) fn ids() -> MemIdMap {
        let mut m = MemIdMap::default();
        m.ids.insert(
            id(ItemKind::Document, "about"),
            DestRef {
                collection: "pages".into(),
                id: json!(7),
                path: Some("/about".into()),
            },
        );
        m.ids.insert(
            id(ItemKind::Document, "nopath"),
            DestRef {
                collection: "posts".into(),
                id: json!(8),
                path: None,
            },
        );
        m.ids.insert(
            id(ItemKind::Media, "pic"),
            DestRef {
                collection: "media".into(),
                id: json!(3),
                path: None,
            },
        );
        m.urls.insert(
            id(ItemKind::Document, "gone"),
            "https://old.example.test/gone/".into(),
        );
        m
    }

    fn link(target: LinkTarget, fragment: Option<&str>) -> RichText {
        RichText {
            blocks: vec![para(vec![Inline::Link {
                target,
                new_tab: false,
                inlines: vec![txt("go")],
                fragment: fragment.map(Into::into),
            }])],
        }
    }

    fn first_link(v: &Value) -> &Value {
        &v["root"]["children"][0]["children"][0]
    }

    fn all_nodes() -> RichText {
        let marks = |f: fn(&mut Marks)| {
            let mut m = Marks::default();
            f(&mut m);
            m
        };
        RichText {
            blocks: vec![
                RtBlock::Heading {
                    level: 2,
                    inlines: vec![txt("Heading")],
                    align: Align::None,
                },
                RtBlock::Paragraph {
                    inlines: vec![
                        Inline::Text {
                            text: "all marks".into(),
                            marks: Marks {
                                bold: true,
                                italic: true,
                                strike: true,
                                underline: true,
                                code: true,
                                sub: true,
                                sup: true,
                            },
                        },
                        Inline::Break,
                        Inline::Text {
                            text: "sup".into(),
                            marks: marks(|m| m.sup = true),
                        },
                        Inline::Link {
                            target: LinkTarget::Url("https://x.example.test/a b".into()),
                            new_tab: true,
                            inlines: vec![txt("ext")],
                            fragment: None,
                        },
                        Inline::Link {
                            target: LinkTarget::Doc(id(ItemKind::Document, "nopath")),
                            new_tab: false,
                            inlines: vec![txt("int")],
                            fragment: None,
                        },
                    ],
                    align: Align::Center,
                },
                RtBlock::List {
                    ordered: true,
                    start: 3,
                    items: vec![
                        vec![para(vec![txt("one")])],
                        vec![
                            para(vec![txt("two")]),
                            RtBlock::List {
                                ordered: false,
                                start: 1,
                                items: vec![vec![para(vec![txt("nested")])]],
                            },
                        ],
                    ],
                },
                RtBlock::Quote {
                    blocks: vec![para(vec![txt("q1")]), para(vec![txt("q2")])],
                },
                RtBlock::Rule,
                RtBlock::Media {
                    media: id(ItemKind::Media, "pic"),
                    caption: Some(RichText {
                        blocks: vec![para(vec![txt("Caption")])],
                    }),
                    align: Align::Left,
                    width: Some(300),
                    link: None,
                },
                RtBlock::Code {
                    lang: Some("rust".into()),
                    text: "a < b".into(),
                },
                RtBlock::Table {
                    rows: vec![
                        vec![vec![para(vec![txt("H")])]],
                        vec![vec![para(vec![Inline::Text {
                            text: "c".into(),
                            marks: marks(|m| m.bold = true),
                        }])]],
                    ],
                    header: true,
                },
                RtBlock::Embed {
                    provider: Some("video".into()),
                    url: "https://video.example.test/v/1".into(),
                },
                RtBlock::Raw {
                    html: "<div class=\"x\">raw</div>".into(),
                },
            ],
        }
    }

    #[test]
    fn lexical_golden() {
        let (v, issues) = to_lexical(&all_nodes(), &ids(), Some("raw"));
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lexical/all-nodes.json"
        );
        if std::env::var_os("PORTER_BLESS").is_some() {
            std::fs::write(path, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
        }
        let want: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/lexical/all-nodes.json"
        ))
        .unwrap();
        assert_eq!(v, want);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn lexical_marks_bitmask() {
        let rt = RichText {
            blocks: vec![para(vec![Inline::Text {
                text: "x".into(),
                marks: Marks {
                    bold: true,
                    italic: true,
                    ..Marks::default()
                },
            }])],
        };
        let (v, _) = to_lexical(&rt, &MemIdMap::default(), None);
        assert_eq!(v["root"]["children"][0]["children"][0]["format"], 3);
    }

    #[test]
    fn lexical_internal_link_resolves() {
        let m = ids();
        let (v, issues) = to_lexical(
            &link(LinkTarget::Doc(id(ItemKind::Document, "about")), None),
            &m,
            None,
        );
        assert_eq!(
            first_link(&v)["fields"],
            json!({"linkType": "internal", "doc": {"relationTo": "pages", "value": 7}, "newTab": false})
        );
        assert_eq!(first_link(&v)["children"][0]["text"], "go");
        assert!(issues.is_empty());

        // A fragment needs a URL: the new path when known...
        let (v, issues) = to_lexical(
            &link(
                LinkTarget::Doc(id(ItemKind::Document, "about")),
                Some("team"),
            ),
            &m,
            None,
        );
        assert_eq!(
            first_link(&v)["fields"],
            json!({"linkType": "custom", "url": "/about#team", "newTab": false})
        );
        assert!(issues.is_empty());

        // ...else the link stays internal and the anchor drop is reported.
        let (v, issues) = to_lexical(
            &link(
                LinkTarget::Doc(id(ItemKind::Document, "nopath")),
                Some("team"),
            ),
            &m,
            None,
        );
        assert_eq!(first_link(&v)["fields"]["linkType"], "internal");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Info);
        assert!(issues[0].message.contains("#team"), "{}", issues[0].message);

        // Media targets relate to the media collection.
        let (v, _) = to_lexical(
            &link(LinkTarget::Media(id(ItemKind::Media, "pic")), None),
            &m,
            None,
        );
        assert_eq!(
            first_link(&v)["fields"]["doc"],
            json!({"relationTo": "media", "value": 3})
        );
    }

    #[test]
    fn lexical_unresolved_link_degrades_with_issue() {
        let (v, issues) = to_lexical(
            &link(LinkTarget::Doc(id(ItemKind::Document, "gone")), Some("x")),
            &ids(),
            None,
        );
        assert_eq!(
            first_link(&v)["fields"],
            json!({"linkType": "custom", "url": "https://old.example.test/gone/#x", "newTab": false})
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::UnresolvedRelation);
        assert_eq!(issues[0].severity, Severity::Warning);

        // No old URL either: the text survives unlinked, still reported.
        let (v, issues) = to_lexical(
            &link(LinkTarget::Doc(id(ItemKind::Document, "nowhere")), None),
            &ids(),
            None,
        );
        assert_eq!(first_link(&v)["type"], "text");
        assert_eq!(first_link(&v)["text"], "go");
        assert_eq!(issues[0].kind, IssueKind::UnresolvedRelation);
    }

    #[test]
    fn lexical_raw_without_block_degrades_to_text() {
        let rt = RichText {
            blocks: vec![
                RtBlock::Raw {
                    html: "<p>Hello <b>raw</b></p><p>two</p>".into(),
                },
                RtBlock::Code {
                    lang: None,
                    text: "l1\nl2".into(),
                },
                RtBlock::Embed {
                    provider: None,
                    url: "https://video.example.test/v/1".into(),
                },
            ],
        };
        let (v, issues) = to_lexical(&rt, &ids(), None);
        let kids = v["root"]["children"].as_array().unwrap();
        assert_eq!(kids[0]["type"], "paragraph");
        assert_eq!(kids[0]["children"][0]["text"], "Hello raw");
        assert_eq!(kids[1]["children"][0]["text"], "two");
        assert_eq!(kids[2]["children"][0]["text"], "l1");
        assert_eq!(kids[3]["children"][0]["text"], "l2");
        assert_eq!(
            kids[4]["children"][0]["fields"]["url"],
            "https://video.example.test/v/1"
        );
        assert_eq!(issues.len(), 3);
        assert!(
            issues
                .iter()
                .all(|i| i.kind == IssueKind::UnsupportedBlock && i.severity == Severity::Warning)
        );
    }

    #[test]
    fn lexical_media_edges_are_reported() {
        // Linked image: upload node kept, link reported.
        let rt = RichText {
            blocks: vec![
                RtBlock::Media {
                    media: id(ItemKind::Media, "pic"),
                    caption: None,
                    align: Align::None,
                    width: None,
                    link: Some(LinkTarget::Url("https://x.example.test/".into())),
                },
                RtBlock::Media {
                    media: id(ItemKind::Media, "missing"),
                    caption: None,
                    align: Align::None,
                    width: None,
                    link: None,
                },
            ],
        };
        let (v, issues) = to_lexical(&rt, &ids(), None);
        assert_eq!(v["root"]["children"][0]["type"], "upload");
        assert_eq!(v["root"]["children"][0]["value"], 3);
        assert_eq!(v["root"]["children"].as_array().unwrap().len(), 1);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].severity, Severity::Info);
        assert_eq!(issues[0].details["link"], "https://x.example.test/");
        assert_eq!(issues[1].kind, IssueKind::UnresolvedRelation);
    }
}
