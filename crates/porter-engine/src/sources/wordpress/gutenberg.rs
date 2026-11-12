//! Gutenberg block content (`<!-- wp:name {json} -->` comments) -> neutral layout blocks.

use super::texturize::texturize;
use super::{flag_literal_shortcodes, layout_richtexts};
use crate::html::{HtmlCtx, MAX_DEPTH, html_to_richtext, too_deep_issue};
use indexmap::IndexMap;
use porter_model::{Block, BlockKind, FieldValue, IssueKind, Original, Severity};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Deeper nesting stays as HTML text inside the block at this depth, so trees stay
/// bounded. Conversion caps lower, at `html::MAX_DEPTH`.
const MAX_PARSE_DEPTH: usize = 2 * MAX_DEPTH;

/// Reusable blocks can include each other; deeper than this is treated as a cycle.
const MAX_REUSABLE_DEPTH: usize = 8;

// WordPress's block delimiter (wp-includes/class-wp-block-parser.php), with a lazy attrs match.
static DELIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<!--\s+(/)?wp:([a-z][a-z0-9_-]*/)?([a-z][a-z0-9_-]*)\s+(\{.*?\}\s+)?(/)?-->")
        .expect("valid regex")
});
static SEL_IMG: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("img").expect("valid selector"));
static SEL_CAPTION: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("figcaption").expect("valid selector"));
static SEL_A: LazyLock<Selector> = LazyLock::new(|| Selector::parse("a").expect("valid selector"));

enum Piece {
    Html(String),
    Block(GbBlock),
}

struct GbBlock {
    name: String,
    attrs: Value,
    inner: Vec<Piece>,
}

impl GbBlock {
    /// The block's saved HTML, inner blocks included.
    fn html(&self) -> String {
        let mut s = String::new();
        for p in &self.inner {
            match p {
                Piece::Html(h) => s.push_str(h),
                Piece::Block(b) => s.push_str(&b.html()),
            }
        }
        s
    }

    /// The block as written, delimiters included.
    fn source(&self) -> String {
        let name = self.name.strip_prefix("core/").unwrap_or(&self.name);
        let attrs = match &self.attrs {
            Value::Object(m) if m.is_empty() => String::new(),
            Value::Null => String::new(),
            a => format!("{a} "),
        };
        let mut s = format!("<!-- wp:{name} {attrs}-->");
        for p in &self.inner {
            match p {
                Piece::Html(h) => s.push_str(h),
                Piece::Block(b) => s.push_str(&b.source()),
            }
        }
        s.push_str(&format!("<!-- /wp:{name} -->"));
        s
    }

    fn children(&self) -> impl Iterator<Item = &GbBlock> {
        self.inner.iter().filter_map(|p| match p {
            Piece::Block(b) => Some(b),
            Piece::Html(_) => None,
        })
    }

    fn str_attr(&self, k: &str) -> Option<&str> {
        self.attrs
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    }
}

fn parse(content: &str) -> Vec<Piece> {
    // stack[0] collects the top level.
    let mut stack: Vec<GbBlock> = vec![GbBlock {
        name: String::new(),
        attrs: Value::Null,
        inner: vec![],
    }];
    let push = |stack: &mut Vec<GbBlock>, p: Piece| {
        if let Some(top) = stack.last_mut() {
            if let (Piece::Html(h), Some(Piece::Html(prev))) = (&p, top.inner.last_mut()) {
                prev.push_str(h);
            } else {
                top.inner.push(p);
            }
        }
    };
    let close = |stack: &mut Vec<GbBlock>| {
        if stack.len() > 1
            && let Some(b) = stack.pop()
            && let Some(top) = stack.last_mut()
        {
            top.inner.push(Piece::Block(b));
        }
    };
    let mut last = 0;
    // Blocks opened past the depth cap: kept as HTML text, with their closers.
    let mut overflow = 0usize;
    for c in DELIM.captures_iter(content) {
        let Some(m) = c.get(0) else { continue };
        if m.start() > last {
            push(
                &mut stack,
                Piece::Html(content[last..m.start()].to_string()),
            );
        }
        last = m.end();
        let ns = c.get(2).map_or("core/", |m| m.as_str());
        let name = format!("{ns}{}", &c[3]);
        let opens = c.get(1).is_none() && c.get(5).is_none();
        if (c.get(1).is_some() && overflow > 0) || (opens && stack.len() > MAX_PARSE_DEPTH) {
            overflow = if opens { overflow + 1 } else { overflow - 1 };
            push(&mut stack, Piece::Html(m.as_str().to_string()));
            continue;
        }
        if c.get(1).is_some() {
            // Close back to the matching opener; a stray closer is dropped.
            if let Some(at) = stack
                .iter()
                .rposition(|b| b.name == name)
                .filter(|&at| at > 0)
            {
                while stack.len() > at {
                    close(&mut stack);
                }
            }
            continue;
        }
        let attrs = c
            .get(4)
            .and_then(|a| serde_json::from_str(a.as_str().trim()).ok())
            .unwrap_or(Value::Object(Default::default()));
        let b = GbBlock {
            name,
            attrs,
            inner: vec![],
        };
        if c.get(5).is_some() {
            push(&mut stack, Piece::Block(b));
        } else {
            stack.push(b);
        }
    }
    if last < content.len() {
        push(&mut stack, Piece::Html(content[last..].to_string()));
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().map(|b| b.inner).unwrap_or_default()
}

fn block(kind: BlockKind, b: &GbBlock) -> Block {
    let classes = b
        .str_attr("className")
        .map(|c| c.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    Block {
        kind,
        props: IndexMap::new(),
        children: vec![],
        original: Some(Original {
            builder: "gutenberg".into(),
            module: b.name.clone(),
            classes,
            settings: b.attrs.clone(),
        }),
    }
}

/// `"66.66%"` -> 0.6666; other units have no share of the row.
fn width(v: Option<&Value>) -> Option<f32> {
    match v? {
        Value::String(s) => s
            .trim()
            .strip_suffix('%')?
            .trim()
            .parse::<f32>()
            .ok()
            .map(|p| p / 100.0),
        Value::Number(n) => n.as_f64().map(|p| (p / 100.0) as f32),
        _ => None,
    }
}

struct Gb<'r, 'c, 'a> {
    reusable: &'r HashMap<u64, String>,
    ctx: &'c mut HtmlCtx<'a>,
    depth: usize,
    /// Nesting of `pieces` calls, capped at `MAX_DEPTH`.
    level: usize,
    flagged_deep: bool,
}

impl Gb<'_, '_, '_> {
    fn rich(&mut self, html: &str, b: &GbBlock) -> Option<Block> {
        if html.trim().is_empty() {
            return None;
        }
        let rt = html_to_richtext(&texturize(html), self.ctx);
        if rt.blocks.is_empty() {
            return None;
        }
        let mut out = block(BlockKind::RichText, b);
        out.props.insert("text".into(), FieldValue::RichText(rt));
        Some(out)
    }

    fn pieces(&mut self, pieces: &[Piece]) -> Vec<Block> {
        let freeform = GbBlock {
            name: "core/freeform".into(),
            attrs: json!({}),
            inner: vec![],
        };
        let mut out = vec![];
        if self.level >= MAX_DEPTH {
            // Too deep: the rest stays as its source.
            if !self.flagged_deep {
                self.flagged_deep = true;
                too_deep_issue(self.ctx);
            }
            // In document order, loose HTML included.
            for p in pieces {
                let (src, html) = match p {
                    Piece::Block(b) => (b, b.source()),
                    Piece::Html(h) if h.trim().is_empty() => continue,
                    Piece::Html(h) => (&freeform, h.clone()),
                };
                let mut raw = block(BlockKind::Raw, src);
                raw.props.insert("html".into(), FieldValue::Text(html));
                out.push(raw);
            }
            return out;
        }
        self.level += 1;
        for p in pieces {
            match p {
                Piece::Html(h) => out.extend(self.rich(h, &freeform)),
                Piece::Block(b) => out.extend(self.block(b)),
            }
        }
        self.level -= 1;
        out
    }

    fn raw(&mut self, b: &GbBlock) -> Block {
        let mut out = block(BlockKind::Raw, b);
        out.props.insert("html".into(), FieldValue::Text(b.html()));
        out
    }

    fn image(&mut self, b: &GbBlock, html: &str) -> Block {
        let mut out = block(BlockKind::Image, b);
        let doc = Html::parse_fragment(html);
        if let Some(img) = doc.select(&SEL_IMG).next() {
            let e = img.value();
            let src = e.attr("src").unwrap_or("");
            if let Some(id) = self.ctx.media_or_issue(src, e.attr("srcset")) {
                out.props.insert("media".into(), FieldValue::Media(id));
            }
            if let Some(alt) = e.attr("alt").filter(|a| !a.is_empty()) {
                out.props.insert("alt".into(), FieldValue::Text(alt.into()));
            }
        }
        if let Some(c) = doc.select(&SEL_CAPTION).next() {
            let text: String = c.text().collect();
            if !text.trim().is_empty() {
                out.props
                    .insert("caption".into(), FieldValue::Text(text.trim().into()));
            }
        }
        if let Some(href) = doc
            .select(&SEL_A)
            .next()
            .and_then(|a| a.value().attr("href"))
        {
            let link = self.ctx.link_value(href);
            out.props.insert("url".into(), link);
        }
        out
    }

    fn button(&mut self, b: &GbBlock) -> Block {
        let mut out = block(BlockKind::Button, b);
        let doc = Html::parse_fragment(&b.html());
        if let Some(a) = doc.select(&SEL_A).next() {
            let text: String = a.text().collect();
            out.props
                .insert("text".into(), FieldValue::Text(text.trim().into()));
            if let Some(href) = a.value().attr("href") {
                let link = self.ctx.link_value(href);
                out.props.insert("url".into(), link);
            }
        }
        out
    }

    /// Dispatch only, so the frames on the recursion path stay small (the cap is 256 deep).
    fn block(&mut self, b: &GbBlock) -> Vec<Block> {
        let kind = match b.name.as_str() {
            "core/columns" => BlockKind::Row,
            "core/column" => BlockKind::Column {
                width: width(b.attrs.get("width")),
            },
            "core/group" => BlockKind::Section,
            "core/block" => return self.reusable_block(b),
            _ => return self.leaf(b),
        };
        let mut out = block(kind, b);
        out.children = self.pieces(&b.inner);
        vec![out]
    }

    fn reusable_block(&mut self, b: &GbBlock) -> Vec<Block> {
        let id = b.attrs.get("ref").and_then(Value::as_u64);
        let content = id.and_then(|id| self.reusable.get(&id));
        match content {
            Some(c) if self.depth < MAX_REUSABLE_DEPTH => {
                self.depth += 1;
                let out = self.pieces(&parse(c));
                self.depth -= 1;
                return out;
            }
            Some(_) => {
                self.ctx.issue(
                    IssueKind::UnsupportedBlock,
                    Severity::Warning,
                    format!(
                        "reusable block {} nests more than {MAX_REUSABLE_DEPTH} deep (does it include itself?)",
                        id.unwrap_or(0)
                    ),
                    json!({ "ref": id }),
                );
            }
            None => self.ctx.issue(
                IssueKind::UnresolvedRelation,
                Severity::Warning,
                format!(
                    "reusable block {} not found",
                    b.attrs.get("ref").unwrap_or(&Value::Null)
                ),
                json!({ "ref": b.attrs.get("ref") }),
            ),
        }
        vec![self.raw(b)]
    }

    /// Blocks with no inner blocks to convert.
    #[inline(never)]
    fn leaf(&mut self, b: &GbBlock) -> Vec<Block> {
        let name = b.name.as_str();
        let one = match name {
            "core/paragraph" | "core/heading" | "core/list" | "core/quote" | "core/table" => {
                return self.rich(&b.html(), b).into_iter().collect();
            }
            "core/image" => self.image(b, &b.html()),
            "core/gallery" => {
                let mut out = block(BlockKind::Gallery, b);
                let inner: Vec<&GbBlock> =
                    b.children().filter(|c| c.name == "core/image").collect();
                if inner.is_empty() {
                    // Older galleries: one <img> per item in the block's own HTML.
                    let doc = Html::parse_fragment(&b.html());
                    let imgs: Vec<String> = doc.select(&SEL_IMG).map(|i| i.html()).collect();
                    for img in imgs {
                        out.children.push(self.image(b, &img));
                    }
                } else {
                    for c in inner {
                        out.children.push(self.image(c, &c.html()));
                    }
                }
                out
            }
            "core/buttons" => return b.children().map(|c| self.button(c)).collect(),
            "core/button" => self.button(b),
            "core/embed" => {
                let mut out = block(BlockKind::Embed, b);
                if let Some(u) = b.str_attr("url") {
                    out.props.insert("url".into(), FieldValue::Text(u.into()));
                }
                out
            }
            "core/html" => self.raw(b),
            "core/separator" => block(BlockKind::Divider, b),
            "core/spacer" => block(BlockKind::Spacer, b),
            _ => {
                self.ctx.issue(
                    IssueKind::UnsupportedBlock,
                    Severity::Warning,
                    format!("block {name} has no neutral block; kept as HTML"),
                    json!({ "block": name }),
                );
                self.raw(b)
            }
        };
        vec![one]
    }
}

/// `reusable` maps reusable-block (wp_block) post ids to their content, for `core/block`.
pub fn gutenberg_to_layout(
    content: &str,
    reusable: &HashMap<u64, String>,
    ctx: &mut HtmlCtx,
) -> Vec<Block> {
    let mut gb = Gb {
        reusable,
        ctx,
        depth: 0,
        level: 0,
        flagged_deep: false,
    };
    let blocks = gb.pieces(&parse(content));
    flag_literal_shortcodes(layout_richtexts(&blocks), gb.ctx);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::PendingIssue;
    use crate::html::tests::{ctx, fixture};
    use porter_model::{BlockKind, FieldValue, IssueKind};

    fn run(src: &str, reusable: &HashMap<u64, String>) -> (Vec<Block>, Vec<PendingIssue>) {
        let f = fixture();
        let mut issues = vec![];
        let blocks = gutenberg_to_layout(src, reusable, &mut ctx(&f, &mut issues));
        (blocks, issues)
    }

    #[test]
    fn gutenberg_columns_image() {
        let (bs, issues) = run(
            include_str!("../../../tests/fixtures/wp-content/gutenberg-page.txt"),
            &HashMap::new(),
        );
        let kinds: Vec<_> = bs.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                BlockKind::RichText,
                BlockKind::Row,
                BlockKind::Divider,
                BlockKind::Raw
            ]
        );
        match &bs[0].props["text"] {
            FieldValue::RichText(rt) => assert_eq!(rt.plain_text(), "Our “rooms”"),
            v => panic!("{v:?}"),
        }
        let row = &bs[1];
        assert_eq!(row.children.len(), 2);
        assert!(
            matches!(row.children[0].kind, BlockKind::Column { width: Some(w) } if (w - 0.6666).abs() < 1e-3)
        );
        let img = &row.children[1].children[0];
        assert_eq!(img.kind, BlockKind::Image);
        assert!(matches!(img.props["media"], FieldValue::Media(_)));
        assert_eq!(img.props["alt"], FieldValue::Text("A room".into()));
        assert_eq!(
            img.props["caption"],
            FieldValue::Text("The garden room".into())
        );
        let o = img.original.as_ref().unwrap();
        assert_eq!(
            (o.builder.as_str(), o.module.as_str()),
            ("gutenberg", "core/image")
        );
        assert_eq!(o.settings["id"], 42);
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [IssueKind::UnsupportedBlock], "{issues:?}");
    }

    #[test]
    fn gutenberg_unknown_block_raw() {
        let (bs, issues) = run(
            "<!-- wp:acme/widget {\"a\":1} -->\n<div class=\"w\">x</div>\n<!-- /wp:acme/widget -->",
            &HashMap::new(),
        );
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].kind, BlockKind::Raw);
        assert_eq!(
            bs[0].props["html"],
            FieldValue::Text("\n<div class=\"w\">x</div>\n".into())
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::UnsupportedBlock);
    }

    #[test]
    fn gutenberg_reusable_and_simple_blocks() {
        let reusable = HashMap::from([(
            5,
            "<!-- wp:paragraph --><p>Shared</p><!-- /wp:paragraph -->".to_string(),
        )]);
        let (bs, issues) = run(
            concat!(
                "<!-- wp:block {\"ref\":5} /-->",
                "<!-- wp:buttons --><div class=\"wp-block-buttons\"><!-- wp:button -->",
                "<div class=\"wp-block-button\"><a class=\"wp-block-button__link\" href=\"https://elsewhere.test/\">Go</a></div>",
                "<!-- /wp:button --></div><!-- /wp:buttons -->",
                "<!-- wp:embed {\"url\":\"https://video.example/v/1\",\"providerNameSlug\":\"video\"} --><figure></figure><!-- /wp:embed -->",
                "<!-- wp:spacer {\"height\":\"40px\"} --><div style=\"height:40px\"></div><!-- /wp:spacer -->",
                "<!-- wp:group --><div class=\"wp-block-group\"><!-- wp:html --><marquee>hi</marquee><!-- /wp:html --></div><!-- /wp:group -->",
                "<!-- wp:block {\"ref\":6} /-->",
            ),
            &reusable,
        );
        let kinds: Vec<_> = bs.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                BlockKind::RichText,
                BlockKind::Button,
                BlockKind::Embed,
                BlockKind::Spacer,
                BlockKind::Section,
                BlockKind::Raw
            ]
        );
        assert_eq!(bs[1].props["text"], FieldValue::Text("Go".into()));
        assert_eq!(
            bs[1].props["url"],
            FieldValue::Text("https://elsewhere.test/".into())
        );
        assert_eq!(
            bs[2].props["url"],
            FieldValue::Text("https://video.example/v/1".into())
        );
        assert_eq!(bs[4].children[0].kind, BlockKind::Raw);
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [IssueKind::UnresolvedRelation]);
    }
}
