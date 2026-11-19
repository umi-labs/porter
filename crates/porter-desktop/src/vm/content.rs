//! Content: a lazy tree of the normalised documents (type, then parent hierarchy) and
//! the detail of the selected one. Nothing is read until it is shown: every store read
//! is a `Query` the window hands to the bridge, and its `Reply` comes back to `land`.

use std::collections::{HashMap, HashSet};

use porter_engine::session::{DocNode, ItemDetail, Page, Paged, STAGES};
use porter_model::{
    Block, Document, FieldValue, Inline, ItemId, ItemStatus, LinkTarget, RichText, RtBlock,
};

use super::pipeline::{Tone, name};

/// Documents per tree page.
pub const PAGE: u64 = 200;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeKey {
    /// A document type at the top of the tree; its children are its root documents.
    Type(String),
    Doc(ItemId),
}

/// A store read the content section needs.
#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    Types,
    Children {
        key: NodeKey,
        page: Page,
    },
    Doc(ItemId),
    /// Titles of documents and filenames of media, for links and relations.
    Labels(Vec<ItemId>),
}

/// What a `Query` read. Errors are kept verbatim.
pub enum Reply {
    Types(Result<Vec<(String, u64)>, String>),
    Children {
        key: NodeKey,
        offset: u64,
        result: Result<Paged<DocNode>, String>,
    },
    Doc(ItemId, Result<Box<ItemDetail>, String>),
    /// None: no live item with that id.
    Labels(Result<Vec<(ItemId, Option<String>)>, String>),
}

/// Title (document) or filename (media) per id; None when the item does not exist.
pub type Labels = HashMap<ItemId, Option<String>>;

#[derive(Default)]
struct Kids {
    rows: Vec<DocNode>,
    total: u64,
    loading: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct Content {
    /// Bumped by `invalidate`; replies to older queries are dropped.
    pub generation: u64,
    types_asked: bool,
    types: Option<Result<Vec<(String, u64)>, String>>,
    kids: HashMap<NodeKey, Kids>,
    expanded: HashSet<NodeKey>,
    selected: Option<ItemId>,
    detail: Option<Result<Detail, String>>,
    labels: Labels,
    /// Labels asked for and not landed yet.
    asked: HashSet<ItemId>,
    pub labels_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Type {
        name: String,
        count: u64,
        expanded: bool,
    },
    Doc {
        node: DocNode,
        expanded: bool,
        selected: bool,
    },
    /// Not all of `parent`'s children are loaded.
    More {
        parent: NodeKey,
        shown: u64,
        total: u64,
    },
    Loading,
    Error(String),
    /// Nothing to show here, and why.
    Empty(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub depth: usize,
    pub kind: RowKind,
}

impl Content {
    /// The section was shown: reads the types once.
    pub fn open(&mut self) -> Vec<Query> {
        if self.types_asked {
            return vec![];
        }
        self.types_asked = true;
        vec![Query::Types]
    }

    /// Expands or collapses a node. Expanding reads its first page, once.
    pub fn toggle(&mut self, key: NodeKey) -> Vec<Query> {
        if self.expanded.remove(&key) {
            return vec![];
        }
        self.expanded.insert(key.clone());
        if self.kids.contains_key(&key) {
            return vec![];
        }
        self.kids.insert(
            key.clone(),
            Kids {
                loading: true,
                ..Default::default()
            },
        );
        vec![Query::Children {
            key,
            page: Page {
                offset: 0,
                limit: PAGE,
            },
        }]
    }

    /// Reads the next page of `key`'s children.
    pub fn more(&mut self, key: NodeKey) -> Vec<Query> {
        let Some(k) = self.kids.get_mut(&key) else {
            return vec![];
        };
        if k.loading || (k.rows.len() as u64) >= k.total {
            return vec![];
        }
        k.loading = true;
        k.error = None;
        vec![Query::Children {
            page: Page {
                offset: k.rows.len() as u64,
                limit: PAGE,
            },
            key,
        }]
    }

    pub fn select(&mut self, id: ItemId) -> Vec<Query> {
        self.selected = Some(id);
        self.detail = None;
        vec![Query::Doc(id)]
    }

    pub fn selected(&self) -> Option<ItemId> {
        self.selected
    }

    pub fn detail(&self) -> Option<&Result<Detail, String>> {
        self.detail.as_ref()
    }

    pub fn labels(&self) -> &Labels {
        &self.labels
    }

    /// The store changed (a stage ran): forget what was read and read again what is shown.
    /// ponytail: collapses the tree; re-reading each expanded node would keep it open.
    pub fn invalidate(&mut self) -> Vec<Query> {
        if !self.types_asked {
            return vec![];
        }
        let selected = self.selected;
        *self = Content {
            generation: self.generation + 1,
            ..Default::default()
        };
        let mut q = self.open();
        if let Some(id) = selected {
            q.extend(self.select(id));
        }
        q
    }

    pub fn land(&mut self, generation: u64, reply: Reply) -> Vec<Query> {
        if generation != self.generation {
            return vec![];
        }
        match reply {
            Reply::Types(t) => self.types = Some(t),
            Reply::Children {
                key,
                offset,
                result,
            } => {
                // Only the page asked for; anything else is a leftover.
                let Some(k) = self.kids.get_mut(&key) else {
                    return vec![];
                };
                if !k.loading || offset != k.rows.len() as u64 {
                    return vec![];
                }
                k.loading = false;
                match result {
                    Ok(p) => {
                        k.rows.extend(p.rows);
                        k.total = p.total;
                    }
                    Err(e) => k.error = Some(e),
                }
            }
            Reply::Doc(id, d) => {
                if self.selected != Some(id) {
                    return vec![];
                }
                self.detail = Some(d.map(|d| Detail::new(*d)));
                return self.ask_labels();
            }
            Reply::Labels(r) => match r {
                Ok(found) => {
                    for (id, l) in found {
                        self.asked.remove(&id);
                        self.labels.insert(id, l);
                    }
                }
                Err(e) => self.labels_error = Some(e),
            },
        }
        vec![]
    }

    fn ask_labels(&mut self) -> Vec<Query> {
        let Some(Ok(d)) = &self.detail else {
            return vec![];
        };
        let mut want = vec![];
        for id in d.wanted() {
            if !self.labels.contains_key(&id) && self.asked.insert(id) {
                want.push(id);
            }
        }
        if want.is_empty() {
            vec![]
        } else {
            vec![Query::Labels(want)]
        }
    }

    /// The visible tree, flattened for a list.
    pub fn rows(&self) -> Vec<Row> {
        let mut out = vec![];
        match &self.types {
            None => out.push(Row {
                depth: 0,
                kind: RowKind::Loading,
            }),
            Some(Err(e)) => out.push(Row {
                depth: 0,
                kind: RowKind::Error(e.clone()),
            }),
            Some(Ok(t)) if t.is_empty() => out.push(Row {
                depth: 0,
                kind: RowKind::Empty("No normalised documents yet: run normalise."),
            }),
            Some(Ok(types)) => {
                for (t, n) in types {
                    let key = NodeKey::Type(t.clone());
                    let expanded = self.expanded.contains(&key);
                    out.push(Row {
                        depth: 0,
                        kind: RowKind::Type {
                            name: t.clone(),
                            count: *n,
                            expanded,
                        },
                    });
                    if expanded {
                        self.children_rows(&key, 1, &mut out);
                    }
                }
            }
        }
        out
    }

    fn children_rows(&self, key: &NodeKey, depth: usize, out: &mut Vec<Row>) {
        let Some(k) = self.kids.get(key) else {
            return;
        };
        for node in &k.rows {
            let me = NodeKey::Doc(node.id);
            let expanded = self.expanded.contains(&me);
            out.push(Row {
                depth,
                kind: RowKind::Doc {
                    node: node.clone(),
                    expanded,
                    selected: self.selected == Some(node.id),
                },
            });
            if expanded {
                self.children_rows(&me, depth + 1, out);
            }
        }
        let kind = if let Some(e) = &k.error {
            Some(RowKind::Error(e.clone()))
        } else if k.loading {
            Some(RowKind::Loading)
        } else if (k.rows.len() as u64) < k.total {
            Some(RowKind::More {
                parent: key.clone(),
                shown: k.rows.len() as u64,
                total: k.total,
            })
        } else if k.rows.is_empty() {
            Some(RowKind::Empty("no documents"))
        } else {
            None
        };
        out.extend(kind.map(|kind| Row { depth, kind }));
    }
}

pub fn status_text(s: &ItemStatus) -> String {
    match s {
        ItemStatus::Done => "done".into(),
        ItemStatus::Partial { reason } => format!("partial: {reason}"),
        ItemStatus::Skipped { reason } => format!("skipped: {reason}"),
        ItemStatus::Failed { error } => format!("failed: {error}"),
    }
}

pub fn status_tone(s: &ItemStatus) -> Tone {
    match s {
        ItemStatus::Done => Tone::Success,
        ItemStatus::Partial { .. } | ItemStatus::Skipped { .. } => Tone::Warning,
        ItemStatus::Failed { .. } => Tone::Danger,
    }
}

/// How a piece of rich text shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    Plain,
    /// A link to a document or media item that exists.
    Resolved,
    /// Its target is still being looked up.
    Pending,
    /// A link that points nowhere: a missing document or media item, or a URL on the old
    /// site that did not become a document link.
    Unresolved,
    /// A link off the old site.
    External,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub mark: Mark,
    /// Where a link goes, shown after it: "→ About us#team", "unresolved: …".
    pub note: Option<String>,
}

impl Span {
    fn plain(text: impl Into<String>) -> Span {
        Span {
            text: text.into(),
            mark: Mark::Plain,
            note: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Indent and list/quote markers.
    pub prefix: String,
    pub heading: bool,
    pub spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldRow {
    pub name: String,
    pub value: String,
    pub unresolved: bool,
}

/// The selected document: the stored row, and the document it decodes to.
pub struct Detail {
    pub item: ItemDetail,
    pub doc: Result<Document, String>,
}

impl Detail {
    pub fn new(item: ItemDetail) -> Detail {
        let doc = serde_json::from_value(item.data.clone())
            .map_err(|e| format!("not a normalised document ({e}); run normalise"));
        Detail { item, doc }
    }

    /// Statuses in pipeline order.
    pub fn statuses(&self) -> Vec<(String, &ItemStatus)> {
        STAGES
            .iter()
            .filter_map(|s| {
                let (_, st) = self.item.row.statuses.iter().find(|(x, _)| x == s)?;
                Some((name(*s), st))
            })
            .collect()
    }

    /// Ids whose title or filename the detail shows.
    pub fn wanted(&self) -> Vec<ItemId> {
        let Ok(doc) = &self.doc else {
            return vec![];
        };
        let mut out = vec![];
        out.extend(doc.parent);
        for v in doc.fields.values() {
            field_ids(v, &mut out);
        }
        let mut seen = HashSet::new();
        out.retain(|id| seen.insert(*id));
        out
    }

    pub fn fields(&self, labels: &Labels) -> Vec<FieldRow> {
        let Ok(doc) = &self.doc else {
            return vec![];
        };
        doc.fields
            .iter()
            .map(|(k, v)| {
                let (value, unresolved) = summary(v, labels);
                FieldRow {
                    name: k.clone(),
                    value,
                    unresolved,
                }
            })
            .collect()
    }

    /// Every rich-text value (in fields, lists, objects and layout blocks), labelled with
    /// where it sits, rendered as lines.
    pub fn rich_texts(&self, labels: &Labels) -> Vec<(String, Vec<Line>)> {
        let Ok(doc) = &self.doc else {
            return vec![];
        };
        let mut found = vec![];
        for (k, v) in &doc.fields {
            rich_in(v, k.clone(), &mut found);
        }
        let ctx = Ctx {
            labels,
            host: self.item.source.url.as_deref().and_then(host_of),
            base: self.item.source.url.as_deref(),
        };
        found
            .into_iter()
            .map(|(label, rt)| {
                let mut lines = vec![];
                blocks(&rt.blocks, "", &ctx, &mut lines);
                (label, lines)
            })
            .collect()
    }

    /// The layout fields as an indented outline: `(depth, "section (divi et_pb_section)")`.
    pub fn outline(&self) -> Vec<(usize, String)> {
        let Ok(doc) = &self.doc else {
            return vec![];
        };
        let mut out = vec![];
        for (k, v) in &doc.fields {
            if let FieldValue::Layout(bs) = v {
                out.push((0, k.clone()));
                for b in bs {
                    outline_block(b, 1, &mut out);
                }
            }
        }
        out
    }
}

fn field_ids(v: &FieldValue, out: &mut Vec<ItemId>) {
    match v {
        FieldValue::Doc(id) | FieldValue::Media(id) => out.push(*id),
        FieldValue::List(vs) => vs.iter().for_each(|v| field_ids(v, out)),
        FieldValue::Object(m) => m.values().for_each(|v| field_ids(v, out)),
        FieldValue::RichText(rt) => rt_ids(&rt.blocks, out),
        FieldValue::Layout(bs) => {
            for b in bs {
                b.walk(&mut |b| b.props.values().for_each(|v| field_ids(v, out)));
            }
        }
        _ => {}
    }
}

fn rt_ids(bs: &[RtBlock], out: &mut Vec<ItemId>) {
    fn target(t: &LinkTarget, out: &mut Vec<ItemId>) {
        if let LinkTarget::Doc(id) | LinkTarget::Media(id) = t {
            out.push(*id);
        }
    }
    fn inl(is: &[Inline], out: &mut Vec<ItemId>) {
        for i in is {
            if let Inline::Link {
                target: t, inlines, ..
            } = i
            {
                target(t, out);
                inl(inlines, out);
            }
        }
    }
    for b in bs {
        match b {
            RtBlock::Paragraph { inlines, .. } | RtBlock::Heading { inlines, .. } => {
                inl(inlines, out)
            }
            RtBlock::List { items, .. } => items.iter().for_each(|i| rt_ids(i, out)),
            RtBlock::Quote { blocks } => rt_ids(blocks, out),
            RtBlock::Table { rows, .. } => rows.iter().flatten().for_each(|c| rt_ids(c, out)),
            RtBlock::Media {
                media,
                caption,
                link,
                ..
            } => {
                out.push(*media);
                if let Some(c) = caption {
                    rt_ids(&c.blocks, out);
                }
                if let Some(l) = link {
                    target(l, out);
                }
            }
            RtBlock::Code { .. } | RtBlock::Rule | RtBlock::Embed { .. } | RtBlock::Raw { .. } => {}
        }
    }
}

fn rich_in<'a>(v: &'a FieldValue, label: String, out: &mut Vec<(String, &'a RichText)>) {
    match v {
        FieldValue::RichText(rt) => out.push((label, rt)),
        FieldValue::List(vs) => {
            for (i, v) in vs.iter().enumerate() {
                rich_in(v, format!("{label}[{i}]"), out);
            }
        }
        FieldValue::Object(m) => {
            for (k, v) in m {
                rich_in(v, format!("{label}.{k}"), out);
            }
        }
        FieldValue::Layout(bs) => {
            let mut all = vec![];
            bs.iter().for_each(|b| preorder(b, &mut all));
            for (n, b) in all.into_iter().enumerate() {
                for (k, v) in &b.props {
                    rich_in(v, format!("{label} › {} {}.{k}", kind_name(b), n + 1), out);
                }
            }
        }
        _ => {}
    }
}

fn preorder<'a>(b: &'a Block, out: &mut Vec<&'a Block>) {
    out.push(b);
    b.children.iter().for_each(|c| preorder(c, out));
}

fn kind_name(b: &Block) -> String {
    serde_json::to_value(&b.kind)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| "block".into())
}

fn outline_block(b: &Block, depth: usize, out: &mut Vec<(usize, String)>) {
    let mut label = kind_name(b);
    if let Some(o) = &b.original {
        label.push_str(&format!(" ({} {})", o.builder, o.module));
    }
    out.push((depth, label));
    for c in &b.children {
        outline_block(c, depth + 1, out);
    }
}

fn clip(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// One line for the fields table, and whether it holds a reference that points nowhere.
fn summary(v: &FieldValue, labels: &Labels) -> (String, bool) {
    match v {
        FieldValue::Null => ("—".into(), false),
        FieldValue::Text(t) => (clip(t, 200), false),
        FieldValue::Number(n) => (n.to_string(), false),
        FieldValue::Bool(b) => (b.to_string(), false),
        FieldValue::Date(d) => (d.to_rfc3339(), false),
        FieldValue::RichText(rt) => (format!("rich text, {} blocks", rt.blocks.len()), false),
        FieldValue::Layout(bs) => (format!("layout, {} blocks", bs.len()), false),
        FieldValue::Doc(id) => match labels.get(id) {
            Some(Some(t)) => (format!("→ {t}"), false),
            Some(None) => (format!("unresolved: no document {id}"), true),
            None => ("→ resolving…".into(), false),
        },
        FieldValue::Media(id) => match labels.get(id) {
            Some(Some(f)) => (format!("media: {f}"), false),
            Some(None) => (format!("unresolved: no media {id}"), true),
            None => ("media: resolving…".into(), false),
        },
        FieldValue::List(vs) => {
            let parts: Vec<_> = vs.iter().map(|v| summary(v, labels)).collect();
            let bad = parts.iter().any(|(_, b)| *b);
            let text = parts
                .into_iter()
                .map(|(s, _)| s)
                .collect::<Vec<_>>()
                .join(", ");
            (clip(&format!("[{text}]"), 200), bad)
        }
        FieldValue::Object(m) => {
            let parts: Vec<_> = m.iter().map(|(k, v)| (k, summary(v, labels))).collect();
            let bad = parts.iter().any(|(_, (_, b))| *b);
            let text = parts
                .into_iter()
                .map(|(k, (s, _))| format!("{k}: {s}"))
                .collect::<Vec<_>>()
                .join(", ");
            (clip(&format!("{{{text}}}"), 200), bad)
        }
    }
}

struct Ctx<'a> {
    labels: &'a Labels,
    /// Host of the document's old URL: links there should have become document links.
    host: Option<String>,
    base: Option<&'a str>,
}

fn host_of(u: &str) -> Option<String> {
    let h = url::Url::parse(u).ok()?.host_str()?.to_lowercase();
    Some(h.strip_prefix("www.").unwrap_or(&h).to_string())
}

/// How a link to `t` shows, and the note naming where it goes.
fn link(t: &LinkTarget, fragment: Option<&str>, ctx: &Ctx) -> (Mark, String) {
    let frag = fragment.map(|f| format!("#{f}")).unwrap_or_default();
    let (what, id) = match t {
        LinkTarget::Doc(id) => ("document", id),
        LinkTarget::Media(id) => ("media", id),
        LinkTarget::Url(u) => {
            // An old-site URL should have resolved to a document or media item.
            let internal = !u.starts_with('#')
                && (u.starts_with('/') && !u.starts_with("//")
                    || ctx.host.is_some() && {
                        let abs = match ctx.base.and_then(|b| url::Url::parse(b).ok()) {
                            Some(b) => b.join(u).ok(),
                            None => url::Url::parse(u).ok(),
                        };
                        abs.filter(|a| matches!(a.scheme(), "http" | "https"))
                            .and_then(|a| host_of(a.as_str()))
                            == ctx.host
                    });
            return if internal {
                (Mark::Unresolved, format!("unresolved: {u}"))
            } else {
                (Mark::External, format!("→ {u}"))
            };
        }
    };
    match ctx.labels.get(id) {
        None => (Mark::Pending, "→ resolving…".into()),
        Some(None) => (
            Mark::Unresolved,
            format!("unresolved: no {what} {id}{frag}"),
        ),
        Some(Some(l)) if what == "media" => (Mark::Resolved, format!("→ media: {l}{frag}")),
        Some(Some(l)) => (Mark::Resolved, format!("→ {l}{frag}")),
    }
}

fn inline_text(is: &[Inline], out: &mut String) {
    for i in is {
        match i {
            Inline::Text { text, .. } => out.push_str(text),
            Inline::Link { inlines, .. } => inline_text(inlines, out),
            Inline::Break => out.push(' '),
        }
    }
}

fn inlines(is: &[Inline], ctx: &Ctx) -> Vec<Span> {
    is.iter()
        .map(|i| match i {
            Inline::Text { text, .. } => Span::plain(text.clone()),
            Inline::Break => Span::plain(" "),
            Inline::Link {
                target,
                inlines,
                fragment,
                ..
            } => {
                let mut text = String::new();
                inline_text(inlines, &mut text);
                let (mark, note) = link(target, fragment.as_deref(), ctx);
                Span {
                    text,
                    mark,
                    note: Some(note),
                }
            }
        })
        .collect()
}

fn blocks(bs: &[RtBlock], prefix: &str, ctx: &Ctx, out: &mut Vec<Line>) {
    let line = |spans, heading| Line {
        prefix: prefix.to_string(),
        heading,
        spans,
    };
    for b in bs {
        match b {
            RtBlock::Paragraph { inlines: is, .. } => out.push(line(inlines(is, ctx), false)),
            RtBlock::Heading {
                level, inlines: is, ..
            } => {
                let mut spans = vec![Span::plain(format!("{} ", "#".repeat(*level as usize)))];
                spans.extend(inlines(is, ctx));
                out.push(line(spans, true));
            }
            RtBlock::List {
                ordered,
                start,
                items,
            } => {
                for (i, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}. ", *start as usize + i)
                    } else {
                        "• ".into()
                    };
                    let from = out.len();
                    blocks(item, &format!("{prefix}  "), ctx, out);
                    if let Some(first) = out.get_mut(from) {
                        first.prefix = format!("{prefix}{marker}");
                    }
                }
            }
            RtBlock::Quote { blocks: bs } => blocks(bs, &format!("{prefix}> "), ctx, out),
            RtBlock::Code { text, .. } => out.push(line(vec![Span::plain(text.clone())], false)),
            RtBlock::Rule => out.push(line(vec![Span::plain("———")], false)),
            RtBlock::Table { rows, .. } => {
                for row in rows {
                    let mut spans = vec![];
                    for (i, cell) in row.iter().enumerate() {
                        if i > 0 {
                            spans.push(Span::plain(" | "));
                        }
                        let mut cl = vec![];
                        blocks(cell, "", ctx, &mut cl);
                        spans.extend(cl.into_iter().flat_map(|l| l.spans));
                    }
                    out.push(line(spans, false));
                }
            }
            RtBlock::Media {
                media,
                caption,
                link: to,
                ..
            } => {
                let (mark, note) = link(&LinkTarget::Media(*media), None, ctx);
                let mut spans = vec![Span {
                    text: "[image]".into(),
                    mark,
                    note: Some(note),
                }];
                if let Some(t) = to {
                    let (mark, note) = link(t, None, ctx);
                    spans.push(Span {
                        text: " linked".into(),
                        mark,
                        note: Some(note),
                    });
                }
                out.push(line(spans, false));
                if let Some(c) = caption {
                    blocks(&c.blocks, &format!("{prefix}  "), ctx, out);
                }
            }
            RtBlock::Embed { provider, url } => out.push(line(
                vec![Span::plain(format!(
                    "[embed{}: {url}]",
                    provider
                        .as_deref()
                        .map(|p| format!(" {p}"))
                        .unwrap_or_default()
                ))],
                false,
            )),
            RtBlock::Raw { html } => out.push(line(
                vec![Span::plain(format!("[raw HTML, {} chars]", html.len()))],
                false,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_engine::session::ItemRow;
    use porter_model::{ItemKind, Marks, SourceRef};

    fn src(id: &str) -> SourceRef {
        SourceRef {
            connector: "wordpress".into(),
            source_id: id.into(),
            url: Some(format!("https://old.test/{id}/")),
        }
    }

    fn id(s: &str) -> ItemId {
        ItemId::new(ItemKind::Document, &src(s))
    }

    fn node(i: usize, has_children: bool) -> DocNode {
        DocNode {
            id: id(&i.to_string()),
            title: format!("Doc {i}"),
            doc_type: "page".into(),
            path: format!("/{i}"),
            has_children,
            worst_status: None,
        }
    }

    fn children(key: &NodeKey, offset: u64) -> Query {
        Query::Children {
            key: key.clone(),
            page: Page {
                offset,
                limit: PAGE,
            },
        }
    }

    /// Nothing below a node is read until it is expanded, and each expansion or page is
    /// exactly one query; collapsing and re-expanding reads nothing again.
    #[test]
    fn tree_is_lazy() {
        let mut c = Content::default();
        assert_eq!(c.open(), [Query::Types]);
        assert_eq!(c.open(), [], "types are read once");
        let g = c.generation;
        c.land(
            g,
            Reply::Types(Ok(vec![("page".into(), 450), ("post".into(), 3)])),
        );
        assert_eq!(c.rows().len(), 2);

        let page = NodeKey::Type("page".into());
        assert_eq!(c.toggle(page.clone()), [children(&page, 0)]);
        assert_eq!(c.rows()[1].kind, RowKind::Loading);
        let first: Vec<DocNode> = (0..200).map(|i| node(i, i == 0)).collect();
        c.land(
            g,
            Reply::Children {
                key: page.clone(),
                offset: 0,
                result: Ok(Paged {
                    rows: first,
                    total: 450,
                }),
            },
        );
        let rows = c.rows();
        assert_eq!(rows.len(), 2 + 200 + 1);
        assert_eq!(
            rows[201].kind,
            RowKind::More {
                parent: page.clone(),
                shown: 200,
                total: 450
            }
        );

        // Collapse and re-expand: cached, no query.
        assert_eq!(c.toggle(page.clone()), []);
        assert_eq!(c.rows().len(), 2);
        assert_eq!(c.toggle(page.clone()), []);
        assert_eq!(c.rows().len(), 203);

        // The next page: one query, and none while it is in flight.
        assert_eq!(c.more(page.clone()), [children(&page, 200)]);
        assert_eq!(c.more(page.clone()), []);
        c.land(
            g,
            Reply::Children {
                key: page.clone(),
                offset: 200,
                result: Ok(Paged {
                    rows: (200..400).map(|i| node(i, false)).collect(),
                    total: 450,
                }),
            },
        );
        assert_eq!(c.rows().len(), 2 + 400 + 1);

        // A document with children: one query for its first page, nested one deeper.
        let doc = NodeKey::Doc(node(0, true).id);
        assert_eq!(c.toggle(doc.clone()), [children(&doc, 0)]);
        c.land(
            g,
            Reply::Children {
                key: doc.clone(),
                offset: 0,
                result: Ok(Paged {
                    rows: vec![node(1000, false)],
                    total: 1,
                }),
            },
        );
        let rows = c.rows();
        assert_eq!(rows[2].depth, 2);
        assert!(matches!(&rows[2].kind, RowKind::Doc { node, .. } if node.title == "Doc 1000"));

        // After the store changed, replies to old queries are dropped.
        let q = c.invalidate();
        assert_eq!(q, [Query::Types]);
        c.land(g, Reply::Types(Ok(vec![("stale".into(), 1)])));
        assert_eq!(c.rows()[0].kind, RowKind::Loading);
    }

    fn text(t: &str) -> Inline {
        Inline::Text {
            text: t.into(),
            marks: Marks::default(),
        }
    }

    fn a(t: &str, target: LinkTarget, fragment: Option<&str>) -> Inline {
        Inline::Link {
            target,
            new_tab: false,
            inlines: vec![text(t)],
            fragment: fragment.map(str::to_string),
        }
    }

    /// Links show what they resolved to; links to nothing, and old-site URLs that never
    /// became document links, are marked unresolved; external links are not.
    #[test]
    fn unresolved_links_marked() {
        let me = id("me");
        let about = id("about");
        let gone = id("gone");
        let pdf = ItemId::new(ItemKind::Media, &src("pdf"));
        let body = RichText {
            blocks: vec![
                RtBlock::Paragraph {
                    inlines: vec![
                        text("See "),
                        a("about", LinkTarget::Doc(about), Some("team")),
                        a("gone", LinkTarget::Doc(gone), None),
                        a(
                            "contact",
                            LinkTarget::Url("https://www.old.test/contact/".into()),
                            None,
                        ),
                        a(
                            "ext",
                            LinkTarget::Url("https://elsewhere.org/".into()),
                            None,
                        ),
                        a("pdf", LinkTarget::Media(pdf), None),
                    ],
                    align: Default::default(),
                },
                RtBlock::Media {
                    media: pdf,
                    caption: None,
                    align: Default::default(),
                    width: None,
                    link: None,
                },
            ],
        };
        let doc = Document {
            id: me,
            doc_type: "page".into(),
            status: porter_model::DocStatus::Published,
            locale: None,
            translations: vec![],
            slug: "me".into(),
            path: "/me".into(),
            parent: None,
            title: "Me".into(),
            fields: [
                ("body".to_string(), FieldValue::RichText(body)),
                ("related".to_string(), FieldValue::Doc(gone)),
            ]
            .into_iter()
            .collect(),
            seo: Default::default(),
            created: None,
            updated: None,
            author: None,
            original: serde_json::json!({}),
            source: src("me"),
        };
        let detail = ItemDetail {
            row: ItemRow {
                id: me,
                kind: ItemKind::Document,
                label: "https://old.test/me/".into(),
                title: Some("Me".into()),
                statuses: vec![],
            },
            source: src("me"),
            data: serde_json::to_value(&doc).unwrap(),
            issues: vec![],
        };

        let mut c = Content::default();
        assert_eq!(c.select(me), [Query::Doc(me)]);
        let q = c.land(c.generation, Reply::Doc(me, Ok(Box::new(detail))));
        let Some(Query::Labels(ids)) = q.first() else {
            panic!("expected a label lookup, got {q:?}");
        };
        let ids: HashSet<_> = ids.iter().copied().collect();
        assert_eq!(ids, HashSet::from([about, gone, pdf]));

        let spans = |c: &Content| -> Vec<Span> {
            let Some(Ok(d)) = c.detail() else {
                panic!("no detail")
            };
            let rt = d.rich_texts(c.labels());
            assert_eq!(rt[0].0, "body");
            rt[0].1.iter().flat_map(|l| l.spans.clone()).collect()
        };
        let find = |s: &[Span], t: &str| s.iter().find(|x| x.text == t).cloned().unwrap();

        // Before the titles land: pending, not called unresolved.
        let s = spans(&c);
        assert_eq!(find(&s, "about").mark, Mark::Pending);

        c.land(
            c.generation,
            Reply::Labels(Ok(vec![
                (about, Some("About us".into())),
                (gone, None),
                (pdf, Some("brochure.pdf".into())),
            ])),
        );
        let s = spans(&c);
        let about = find(&s, "about");
        assert_eq!(about.mark, Mark::Resolved);
        let note = about.note.unwrap();
        assert!(
            note.contains("About us") && note.contains("#team"),
            "{note}"
        );
        assert_eq!(find(&s, "gone").mark, Mark::Unresolved);
        let contact = find(&s, "contact");
        assert_eq!(contact.mark, Mark::Unresolved);
        assert!(contact.note.unwrap().contains("old.test/contact"));
        assert_eq!(find(&s, "ext").mark, Mark::External);
        let pdf = find(&s, "pdf");
        assert_eq!(pdf.mark, Mark::Resolved);
        assert!(pdf.note.unwrap().contains("brochure.pdf"));
        let img = find(&s, "[image]");
        assert!(img.note.unwrap().contains("brochure.pdf"));

        let Some(Ok(d)) = c.detail() else { panic!() };
        let f = d.fields(c.labels());
        let related = f.iter().find(|r| r.name == "related").unwrap();
        assert!(related.unresolved, "{related:?}");
    }
}
