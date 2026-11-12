//! Divi builder content (post_content shortcodes) -> neutral layout blocks.

use super::shortcodes::{ScNode, caption_html, default_known, parse_shortcodes, to_source};
use super::{classic_inner, flag_literal_shortcodes, layout_richtexts};
use crate::html::{HtmlCtx, MAX_DEPTH, decode_entities, too_deep_issue};
use indexmap::IndexMap;
use porter_model::{Block, BlockKind, FieldValue, IssueKind, ItemId, Original, Severity};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Global layouts can include each other; deeper than this is treated as a cycle.
const MAX_GLOBAL_DEPTH: usize = 8;

// Divi's newline placeholder, as a comment (or entity-escaped, inside code modules).
static LINE_BREAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:<|&lt;)!--\s*\[et_pb_line_break_holder\]\s*--(?:>|&gt;)").expect("valid regex")
});
static KNOWN: LazyLock<Regex> = LazyLock::new(default_known);

fn parse(content: &str) -> Vec<ScNode> {
    let src = LINE_BREAK.replace_all(content, "\n");
    parse_shortcodes(&src.replace("[et_pb_line_break_holder]", "\n"), &KNOWN)
}

/// Divi escapes `[ ] "` inside attribute values as `%91 %93 %22`; values are also entity-encoded.
fn attr(v: &str) -> String {
    decode_entities(
        &v.replace("%91", "[")
            .replace("%93", "]")
            .replace("%22", "\""),
    )
    .trim()
    .to_string()
}

fn get(n: &ScNode, k: &str) -> String {
    n.attrs.get(k).map(|v| attr(v)).unwrap_or_default()
}

fn hidden(n: &ScNode) -> bool {
    n.attrs.get("disabled").is_some_and(|v| v == "on")
        || n.attrs
            .get("disabled_on")
            .is_some_and(|v| v.starts_with("on|on|on"))
}

fn is_column(tag: &str) -> bool {
    matches!(tag, "et_pb_column" | "et_pb_column_inner")
}

/// `1_3` -> 0.333.., `4_4` -> 1.0.
fn width(t: &str) -> Option<f32> {
    let (a, b) = t.split_once('_')?;
    let (a, b): (f32, f32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (b > 0.0).then_some(a / b)
}

fn original(n: &ScNode) -> Original {
    let builder = if n.tag.is_empty() || n.tag.starts_with("et_pb_") {
        "divi"
    } else {
        "shortcode"
    };
    Original {
        builder: builder.into(),
        module: if n.tag.is_empty() {
            "#text".into()
        } else {
            n.tag.clone()
        },
        classes: get(n, "module_class")
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        settings: Value::Object(
            n.attrs
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(attr(v))))
                .collect(),
        ),
    }
}

fn block(kind: BlockKind, n: &ScNode) -> Block {
    Block {
        kind,
        props: IndexMap::new(),
        children: vec![],
        original: Some(original(n)),
    }
}

/// First node with `tag`, shallowest first.
fn find(nodes: Vec<ScNode>, tag: &str) -> Option<ScNode> {
    let mut level = nodes;
    while !level.is_empty() {
        if let Some(i) = level.iter().position(|n| n.tag == tag) {
            return Some(level.swap_remove(i));
        }
        level = level.into_iter().flat_map(|n| n.children).collect();
    }
    None
}

struct Divi<'g, 'c, 'a> {
    globals: &'g HashMap<u64, String>,
    media_ids: &'g HashMap<u64, ItemId>,
    ctx: &'c mut HtmlCtx<'a>,
    /// Nesting of `modules` calls, capped at `MAX_DEPTH`.
    level: usize,
    flagged_deep: bool,
}

impl Divi<'_, '_, '_> {
    fn issue(&mut self, kind: IssueKind, message: String, details: Value) {
        self.ctx.issue(kind, Severity::Warning, message, details);
    }

    /// Replaces global modules with their current copy from the Divi Library.
    fn expand(&mut self, nodes: Vec<ScNode>, depth: usize, level: usize) -> Vec<ScNode> {
        // Past the converter's depth cap nothing is converted, so nothing to expand.
        if level >= MAX_DEPTH {
            return nodes;
        }
        let mut out = Vec::with_capacity(nodes.len());
        for mut n in nodes {
            let id = n
                .attrs
                .get("global_module")
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|&id| id > 0);
            if let Some(id) = id {
                if depth >= MAX_GLOBAL_DEPTH {
                    self.issue(
                        IssueKind::UnsupportedBlock,
                        format!("Divi global layout {id} nests more than {MAX_GLOBAL_DEPTH} deep (does it include itself?); kept the copy saved in the page"),
                        json!({ "global_module": id }),
                    );
                    n.attrs.shift_remove("global_module");
                    out.push(n);
                    continue;
                }
                match self.globals.get(&id).and_then(|c| find(parse(c), &n.tag)) {
                    Some(mut found) => {
                        found.attrs.shift_remove("global_module");
                        out.extend(self.expand(vec![found], depth + 1, level));
                        continue;
                    }
                    None => self.issue(
                        IssueKind::UnresolvedRelation,
                        format!(
                            "Divi global layout {id} not found; kept the copy saved in the page"
                        ),
                        json!({ "global_module": id }),
                    ),
                }
            }
            let kids = std::mem::take(&mut n.children);
            n.children = self.expand(kids, depth, level + 1);
            out.push(n);
        }
        out
    }

    fn rich(&mut self, html: &str) -> Option<porter_model::RichText> {
        if html.trim().is_empty() {
            return None;
        }
        let rt = classic_inner(html, self.media_ids, self.ctx);
        (!rt.blocks.is_empty()).then_some(rt)
    }

    /// Modules in order; hidden ones dropped; consecutive toggles become one accordion.
    fn modules<'n>(&mut self, nodes: impl IntoIterator<Item = &'n ScNode>) -> Vec<Block> {
        let toggle = |b: &Block| {
            b.kind == BlockKind::Accordion
                && b.original
                    .as_ref()
                    .is_some_and(|o| o.module == "et_pb_toggle")
        };
        let mut out: Vec<Block> = vec![];
        if self.level >= MAX_DEPTH {
            return self.too_deep(nodes);
        }
        self.level += 1;
        for n in nodes.into_iter().filter(|n| !hidden(n)) {
            for b in self.node(n) {
                match out.last_mut() {
                    Some(prev) if toggle(prev) && toggle(&b) => prev.children.extend(b.children),
                    _ => out.push(b),
                }
            }
        }
        self.level -= 1;
        out
    }

    /// Past the depth cap: the rest stays as its shortcode source, with one warning.
    #[inline(never)]
    fn too_deep<'n>(&mut self, nodes: impl IntoIterator<Item = &'n ScNode>) -> Vec<Block> {
        if !self.flagged_deep {
            self.flagged_deep = true;
            too_deep_issue(self.ctx);
        }
        nodes
            .into_iter()
            .filter(|n| !hidden(n))
            .map(|n| {
                let mut b = block(BlockKind::Raw, n);
                b.props
                    .insert("html".into(), FieldValue::Text(to_source(n)));
                b
            })
            .collect()
    }

    /// Dispatch only, so the frames on the recursion path stay small (the cap is 256 deep).
    fn node(&mut self, n: &ScNode) -> Vec<Block> {
        match n.tag.as_str() {
            "" => match self.rich(&n.text) {
                Some(rt) => {
                    let mut b = block(BlockKind::RichText, n);
                    b.props.insert("text".into(), FieldValue::RichText(rt));
                    vec![b]
                }
                None => vec![],
            },
            "et_pb_section" | "et_pb_fullwidth_section" => self.section(n),
            "et_pb_row" | "et_pb_row_inner" => self.container(BlockKind::Row, n),
            tag if is_column(tag) => self.container(
                BlockKind::Column {
                    width: width(&get(n, "type")),
                },
                n,
            ),
            "et_pb_text" => self.text(n),
            "et_pb_code" | "et_pb_fullwidth_code" => self.code(n),
            "et_pb_tabs" => vec![self.panels(n, BlockKind::Tabs, "et_pb_tab")],
            "et_pb_accordion" => {
                vec![self.panels(n, BlockKind::Accordion, "et_pb_accordion_item")]
            }
            "et_pb_toggle" => {
                let mut b = block(BlockKind::Accordion, n);
                b.children.push(self.panel(n, "title"));
                vec![b]
            }
            "et_pb_slider" | "et_pb_fullwidth_slider" => self.slider(n),
            _ => self.leaf(n),
        }
    }

    /// Rows and columns: their children in place.
    fn container(&mut self, kind: BlockKind, n: &ScNode) -> Vec<Block> {
        let mut b = block(kind, n);
        b.children = self.modules(&n.children);
        vec![b]
    }

    fn section(&mut self, n: &ScNode) -> Vec<Block> {
        let mut b = block(BlockKind::Section, n);
        b.children = if n.children.iter().any(|c| !hidden(c) && is_column(&c.tag)) {
            self.specialty(n)
        } else {
            self.modules(&n.children)
        };
        vec![b]
    }

    /// A specialty section: its columns sit in the row Divi adds around them.
    #[inline(never)]
    fn specialty(&mut self, n: &ScNode) -> Vec<Block> {
        let (cols, rest): (Vec<&ScNode>, Vec<&ScNode>) =
            n.children.iter().partition(|c| is_column(&c.tag));
        let mut row = block(
            BlockKind::Row,
            &ScNode {
                tag: "et_pb_row".into(),
                ..ScNode::default()
            },
        );
        row.children = self.modules(cols);
        let mut out = vec![row];
        out.extend(self.modules(rest));
        out
    }

    fn code(&mut self, n: &ScNode) -> Vec<Block> {
        // Divi stores code entity-encoded; shortcodes inside (a form) still render.
        let code: String = n
            .children
            .iter()
            .filter(|c| c.tag.is_empty())
            .map(|c| c.text.as_str())
            .collect();
        let code = decode_entities(&code).trim().to_string();
        let mut out = vec![];
        if !code.is_empty() {
            let mut b = block(BlockKind::Raw, n);
            b.props.insert("html".into(), FieldValue::Text(code));
            out.push(b);
        }
        let inner: Vec<ScNode> = n
            .children
            .iter()
            .filter(|c| !c.tag.is_empty())
            .cloned()
            .collect();
        out.extend(self.modules(&inner));
        out
    }

    fn slider(&mut self, n: &ScNode) -> Vec<Block> {
        let mut b = block(BlockKind::Slider, n);
        for s in n
            .children
            .iter()
            .filter(|c| c.tag == "et_pb_slide" && !hidden(c))
        {
            let mut slide = self.panel(s, "heading");
            let img = get(s, "image");
            if !img.is_empty()
                && let Some(id) = self.ctx.media_or_issue(&img, None)
            {
                slide.props.insert("media".into(), FieldValue::Media(id));
            }
            self.link_prop(&mut slide, "url", get(s, "button_link"));
            b.children.push(slide);
        }
        vec![b]
    }

    /// Modules with no children to convert.
    #[inline(never)]
    fn leaf(&mut self, n: &ScNode) -> Vec<Block> {
        match n.tag.as_str() {
            "et_pb_image" | "et_pb_fullwidth_image" => {
                let mut b = block(BlockKind::Image, n);
                let src = get(n, "src");
                if !src.is_empty()
                    && let Some(id) = self.ctx.media_or_issue(&src, None)
                {
                    b.props.insert("media".into(), FieldValue::Media(id));
                }
                self.text_prop(&mut b, "alt", get(n, "alt"));
                self.text_prop(&mut b, "title", get(n, "title_text"));
                self.link_prop(&mut b, "url", get(n, "url"));
                vec![b]
            }
            "et_pb_gallery" | "gallery" => {
                let mut b = block(BlockKind::Gallery, n);
                let ids = if n.tag == "gallery" {
                    get(n, "ids")
                } else {
                    get(n, "gallery_ids")
                };
                // Attachment post ids, mapped to media items through `media_ids`.
                for id in ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let mut img = Block {
                        kind: BlockKind::Image,
                        props: IndexMap::new(),
                        children: vec![],
                        original: Some(Original {
                            builder: "divi".into(),
                            module: "attachment".into(),
                            classes: vec![],
                            settings: json!({ "id": id }),
                        }),
                    };
                    img.props
                        .insert("attachment".into(), FieldValue::Text(id.into()));
                    match id.parse::<u64>().ok().and_then(|i| self.media_ids.get(&i)) {
                        Some(m) => {
                            img.props.insert("media".into(), FieldValue::Media(*m));
                        }
                        None => self.issue(
                            IssueKind::MissingMedia,
                            format!("attachment {id} is not in the media library"),
                            json!({ "attachment": id }),
                        ),
                    }
                    b.children.push(img);
                }
                vec![b]
            }
            "et_pb_button" => {
                let mut b = block(BlockKind::Button, n);
                self.text_prop(&mut b, "text", get(n, "button_text"));
                self.link_prop(&mut b, "url", get(n, "button_url"));
                vec![b]
            }
            "et_pb_video" => {
                let mut b = block(BlockKind::Video, n);
                let src = get(n, "src");
                if let Some(id) = self.ctx.find_media([src.as_str()]) {
                    b.props.insert("media".into(), FieldValue::Media(id));
                }
                self.text_prop(&mut b, "url", src);
                vec![b]
            }
            "et_pb_map" | "et_pb_fullwidth_map" => {
                let mut b = block(BlockKind::Map, n);
                self.text_prop(&mut b, "address", get(n, "address"));
                for (k, from) in [("lat", "address_lat"), ("lng", "address_lng")] {
                    if let Ok(v) = get(n, from).parse::<f64>() {
                        b.props.insert(k.into(), FieldValue::Number(v));
                    }
                }
                vec![b]
            }
            "et_pb_divider" => vec![block(BlockKind::Divider, n)],
            "gravityform" | "gravityforms" | "ninja_form" => {
                let mut b = block(BlockKind::FormRef, n);
                self.text_prop(&mut b, "form", get(n, "id"));
                vec![b]
            }
            tag => {
                let mut b = block(BlockKind::Raw, n);
                b.props
                    .insert("html".into(), FieldValue::Text(to_source(n)));
                self.issue(
                    IssueKind::UnsupportedBlock,
                    format!("Divi module {tag} has no neutral block; kept as its shortcode"),
                    json!({ "module": tag }),
                );
                vec![b]
            }
        }
    }

    fn text_prop(&mut self, b: &mut Block, k: &str, v: String) {
        if !v.is_empty() {
            b.props.insert(k.into(), FieldValue::Text(v));
        }
    }

    fn link_prop(&mut self, b: &mut Block, k: &str, v: String) {
        if !v.is_empty() {
            let link = self.ctx.link_value(&v);
            b.props.insert(k.into(), link);
        }
    }

    /// A text module: its HTML as rich text, split where a block-level shortcode sits.
    fn text(&mut self, n: &ScNode) -> Vec<Block> {
        let mut out = vec![];
        let mut pending = String::new();
        let flush = |me: &mut Self, pending: &mut String, out: &mut Vec<Block>| {
            if let Some(rt) = me.rich(pending) {
                let mut b = block(BlockKind::RichText, n);
                b.props.insert("text".into(), FieldValue::RichText(rt));
                out.push(b);
            }
            pending.clear();
        };
        for c in n.children.iter().filter(|c| !hidden(c)) {
            match c.tag.as_str() {
                "" => pending.push_str(&c.text),
                "caption" => pending.push_str(&caption_html(c)),
                _ => {
                    flush(self, &mut pending, &mut out);
                    out.extend(self.modules(std::slice::from_ref(c)));
                }
            }
        }
        flush(self, &mut pending, &mut out);
        out
    }

    /// A titled panel (tab, accordion item, slide): rich text, nested modules as children.
    fn panel(&mut self, n: &ScNode, title: &str) -> Block {
        let mut b = block(BlockKind::RichText, n);
        self.text_prop(&mut b, "title", get(n, title));
        let mut html = String::new();
        let mut nested = vec![];
        for c in n.children.iter().filter(|c| !hidden(c)) {
            match c.tag.as_str() {
                "" => html.push_str(&c.text),
                "caption" => html.push_str(&caption_html(c)),
                _ => nested.push(c.clone()),
            }
        }
        if let Some(rt) = self.rich(&html) {
            b.props.insert("text".into(), FieldValue::RichText(rt));
        }
        b.children = self.modules(&nested);
        b
    }

    fn panels(&mut self, n: &ScNode, kind: BlockKind, item: &str) -> Block {
        let mut b = block(kind, n);
        for c in n.children.iter().filter(|c| c.tag == item && !hidden(c)) {
            let p = self.panel(c, "title");
            b.children.push(p);
        }
        b
    }
}

/// `globals` maps Divi Library (et_pb_layout) post ids to their content; `media_ids` maps
/// attachment post ids to media items.
pub fn divi_to_layout(
    content: &str,
    globals: &HashMap<u64, String>,
    media_ids: &HashMap<u64, ItemId>,
    ctx: &mut HtmlCtx,
) -> Vec<Block> {
    let mut d = Divi {
        globals,
        media_ids,
        ctx,
        level: 0,
        flagged_deep: false,
    };
    let nodes = d.expand(parse(content), 0, 0);
    let blocks = d.modules(&nodes);
    flag_literal_shortcodes(layout_richtexts(&blocks), d.ctx);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::PendingIssue;
    use crate::html::tests::{ctx, fixture};
    use porter_model::{BlockKind, FieldValue, IssueKind};

    fn run(src: &str, globals: &HashMap<u64, String>) -> (Vec<Block>, Vec<PendingIssue>) {
        let f = fixture();
        let mut issues = vec![];
        let blocks = divi_to_layout(src, globals, &HashMap::new(), &mut ctx(&f, &mut issues));
        (blocks, issues)
    }

    fn kinds(bs: &[Block]) -> Vec<String> {
        let mut out = vec![];
        for b in bs {
            b.walk(&mut |x| {
                out.push(
                    serde_json::to_value(&x.kind).unwrap()["type"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                )
            });
        }
        out
    }

    fn text_of(b: &Block) -> String {
        match b.props.get("text") {
            Some(FieldValue::RichText(rt)) => rt.plain_text(),
            Some(FieldValue::Text(t)) => t.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn divi_columns_and_widths() {
        let (bs, _) = run(
            r#"[et_pb_section][et_pb_row][et_pb_column type="1_3"][et_pb_text]a[/et_pb_text][/et_pb_column][et_pb_column type="2_3"][et_pb_text]b[/et_pb_text][/et_pb_column][/et_pb_row][/et_pb_section]"#,
            &HashMap::new(),
        );
        assert_eq!(
            kinds(&bs),
            [
                "section",
                "row",
                "column",
                "rich-text",
                "column",
                "rich-text"
            ]
        );
        let row = &bs[0].children[0];
        let w = |i: usize| match row.children[i].kind {
            BlockKind::Column { width } => width.unwrap(),
            _ => panic!(),
        };
        assert!((w(0) - 1.0 / 3.0).abs() < 1e-4);
        assert!((w(1) - 2.0 / 3.0).abs() < 1e-4);
        assert_eq!(text_of(&row.children[1].children[0]), "b");
        let o = row.children[0].original.as_ref().unwrap();
        assert_eq!(
            (o.builder.as_str(), o.module.as_str()),
            ("divi", "et_pb_column")
        );
    }

    #[test]
    fn divi_global_module_expanded() {
        let globals =
            HashMap::from([(12, "[et_pb_text]From the library[/et_pb_text]".to_string())]);
        let (bs, issues) = run(
            r#"[et_pb_text global_module="12"]stale copy[/et_pb_text]"#,
            &globals,
        );
        assert_eq!(bs.len(), 1);
        assert_eq!(text_of(&bs[0]), "From the library");
        assert!(issues.is_empty());
        // A global that includes itself stops at the depth cap.
        let cyc = HashMap::from([(
            7,
            r#"[et_pb_section][et_pb_section global_module="7"][/et_pb_section][/et_pb_section]"#
                .to_string(),
        )]);
        let (_, issues) = run(r#"[et_pb_section global_module="7"][/et_pb_section]"#, &cyc);
        assert!(issues.iter().any(|i| i.kind == IssueKind::UnsupportedBlock));
    }

    #[test]
    fn divi_hidden_module_skipped() {
        let (bs, _) = run(
            r#"[et_pb_text disabled_on="on|on|on"]gone[/et_pb_text][et_pb_text disabled="on"]gone[/et_pb_text][et_pb_text]kept[/et_pb_text]"#,
            &HashMap::new(),
        );
        assert_eq!(bs.len(), 1);
        assert_eq!(text_of(&bs[0]), "kept");
    }

    #[test]
    fn divi_unknown_module_raw_with_issue() {
        let (bs, issues) = run(
            r#"[et_pb_countdown_timer date_time="2030-01-01" /]"#,
            &HashMap::new(),
        );
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].kind, BlockKind::Raw);
        assert_eq!(
            bs[0].props["html"],
            FieldValue::Text(r#"[et_pb_countdown_timer date_time="2030-01-01" /]"#.into())
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::UnsupportedBlock);
    }

    #[test]
    fn divi_modules_map() {
        let (bs, issues) = run(
            include_str!("../../../tests/fixtures/wp-content/divi-page.txt"),
            &HashMap::new(),
        );
        assert_eq!(
            kinds(&bs),
            [
                "section",
                "row",
                "column",
                "image",
                "column",
                "rich-text",
                "button",
                "row",
                "column",
                "accordion",
                "rich-text",
                "rich-text",
                "raw",
                "raw",
                "form-ref"
            ]
        );
        let col = &bs[0].children[0].children;
        let img = &col[0].children[0];
        assert!(matches!(img.props["media"], FieldValue::Media(_)));
        assert_eq!(img.props["alt"], FieldValue::Text("A lamp".into()));
        let text = text_of(&col[1].children[0]);
        assert!(
            text.starts_with("Welcome to the “Sample” shop\nIt’s a place"),
            "{text}"
        );
        assert!(matches!(
            col[1].children[1].props["url"],
            FieldValue::Doc(_)
        ));
        assert_eq!(
            col[1].children[1].props["text"],
            FieldValue::Text("Shop now".into())
        );
        let second = &bs[0].children[1].children[0].children;
        assert_eq!(second[0].children.len(), 2);
        assert_eq!(
            second[0].children[1].props["title"],
            FieldValue::Text("Question two".into())
        );
        assert_eq!(
            second[2].props["html"],
            FieldValue::Text("<div class=\"widget\">\n<b>raw</b></div>".into())
        );
        assert_eq!(second[3].props["form"], FieldValue::Text("3".into()));
        assert_eq!(
            bs[0].original.as_ref().unwrap().classes,
            vec!["intro-band".to_string()]
        );
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [IssueKind::UnsupportedBlock]);
    }

    #[test]
    fn divi_large_page_is_fast() {
        let module = r#"[et_pb_row][et_pb_column type="1_2"][et_pb_text]<p>Some <strong>text</strong> with a <a href="/about/">link</a>.</p>[/et_pb_text][/et_pb_column][et_pb_column type="1_2"][et_pb_button button_text="Go" button_url="/about/" /][/et_pb_column][/et_pb_row]"#;
        let mut src = String::from("[et_pb_section]");
        while src.len() < 2_000_000 {
            src.push_str(module);
        }
        src.push_str("[/et_pb_section]");
        let start = std::time::Instant::now();
        let (bs, _) = run(&src, &HashMap::new());
        let took = start.elapsed();
        assert!(bs[0].children.len() > 5000);
        // Debug builds are ~10x slower than release; the bar is "well under a second" in release.
        assert!(took.as_secs_f64() < 10.0, "took {took:?}");
        eprintln!("2 MB Divi page: {took:?}");
    }

    #[test]
    fn divi_url_query_preserved() {
        let url = "https://elsewhere.test/x?a=1&region=uk&param=2&notify=1";
        let (bs, _) = run(
            &format!(r#"[et_pb_button button_text="Go &amp; see" button_url="{url}" /]"#),
            &HashMap::new(),
        );
        assert_eq!(bs[0].props["url"], FieldValue::Text(url.into()));
        assert_eq!(bs[0].props["text"], FieldValue::Text("Go & see".into()));
    }

    #[test]
    fn divi_gallery_ids_resolve_or_flag() {
        let f = fixture();
        let mut issues = vec![];
        let ids = HashMap::from([(7, f.media)]);
        let bs = divi_to_layout(
            r#"[et_pb_gallery gallery_ids="7,8" /]"#,
            &HashMap::new(),
            &ids,
            &mut ctx(&f, &mut issues),
        );
        let g = &bs[0];
        assert_eq!(g.kind, BlockKind::Gallery);
        assert_eq!(g.children[0].props["media"], FieldValue::Media(f.media));
        assert!(!g.children[1].props.contains_key("media"));
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [IssueKind::MissingMedia]);
        assert!(issues[0].message.contains('8'));
    }

    #[test]
    fn divi_tabs_keep_panels() {
        let (bs, issues) = run(
            r#"[et_pb_tabs][et_pb_tab title="One"]a[/et_pb_tab][et_pb_tab title="Two"]b[/et_pb_tab][/et_pb_tabs]"#,
            &HashMap::new(),
        );
        assert_eq!(bs[0].kind, BlockKind::Tabs);
        assert_eq!(bs[0].children.len(), 2);
        assert_eq!(
            bs[0].children[1].props["title"],
            FieldValue::Text("Two".into())
        );
        assert_eq!(text_of(&bs[0].children[1]), "b");
        assert!(issues.is_empty(), "{issues:?}");
    }
}
