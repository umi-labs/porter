//! Neutral layout blocks -> a Payload `blocks` field value, via the `[[mapping.blocks]]` maps.

use super::lexical::{IdMap, to_html, to_lexical};
use crate::html::PendingIssue;
use crate::mapping::BlockMap;
use porter_model::{Block, BlockKind, FieldValue, IssueKind, Severity};
use serde_json::{Map, Value, json};

/// The blocks array for `layout`, plus the issues met converting it. `raw_block` is the
/// mapping's raw HTML block slug, the fallback for blocks no map matches.
pub fn to_blocks(
    layout: &[Block],
    mapping: &[BlockMap],
    ids: &dyn IdMap,
    raw_block: Option<&str>,
) -> (Vec<Value>, Vec<PendingIssue>) {
    let mut w = W {
        maps: mapping,
        ids,
        raw_block,
        issues: vec![],
    };
    let mut out = vec![];
    for b in layout {
        w.block(b, &mut out);
    }
    (out, w.issues)
}

struct W<'a> {
    maps: &'a [BlockMap],
    ids: &'a dyn IdMap,
    raw_block: Option<&'a str>,
    issues: Vec<PendingIssue>,
}

/// The kind's kebab-case wire name (`rich-text`, `column`, ...), what `match.kind` names.
fn kind_name(k: &BlockKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v["type"].as_str().map(String::from))
        .unwrap_or_default()
}

/// Every condition the map sets must hold; a map setting none matches nothing.
fn matches(m: &BlockMap, kind: &str, module: Option<&str>) -> bool {
    let (k, md) = (m.r#match.kind.as_deref(), m.r#match.module.as_deref());
    (k.is_some() || md.is_some())
        && k.is_none_or(|k| k == kind)
        && md.is_none_or(|md| Some(md) == module)
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

    fn blocks(&mut self, bs: &[Block]) -> Vec<Value> {
        let mut out = vec![];
        for b in bs {
            self.block(b, &mut out);
        }
        out
    }

    fn block(&mut self, b: &Block, out: &mut Vec<Value>) {
        let kind = kind_name(&b.kind);
        let module = b.original.as_ref().map(|o| o.module.as_str());
        if let Some(m) = self.maps.iter().find(|m| matches(m, &kind, module)) {
            let mut obj = Map::new();
            obj.insert("blockType".into(), json!(m.to));
            let mut unmapped = vec![];
            for (k, v) in &b.props {
                match m.fields.get(k) {
                    Some(dest) => {
                        if let Some(x) = self.value(v) {
                            obj.insert(dest.clone(), x);
                        }
                    }
                    None => unmapped.push(k.as_str()),
                }
            }
            if !unmapped.is_empty() {
                self.issue(
                    IssueKind::UnmappedField,
                    Severity::Warning,
                    format!(
                        "{kind} block -> {}: no destination field for {}",
                        m.to,
                        unmapped.join(", ")
                    ),
                    json!({"block": m.to, "props": unmapped}),
                );
            }
            // Children go in the field `children` maps to, else follow the block, flattened.
            let kids = self.blocks(&b.children);
            match m.fields.get("children") {
                Some(dest) => {
                    obj.insert(dest.clone(), Value::Array(kids));
                }
                None => {
                    out.push(Value::Object(obj));
                    out.extend(kids);
                    return;
                }
            }
            out.push(Value::Object(obj));
            return;
        }
        if !matches!(
            b.kind,
            BlockKind::Section | BlockKind::Row | BlockKind::Column { .. }
        ) {
            self.unmatched(b, &kind, module, out);
        }
        for c in &b.children {
            self.block(c, out);
        }
    }

    /// No map for a content block: an error, plus the raw block when one is configured.
    fn unmatched(&mut self, b: &Block, kind: &str, module: Option<&str>, out: &mut Vec<Value>) {
        let html = match self.raw_block {
            Some(_) => self.block_html(b),
            None => String::new(),
        };
        let kept = match self.raw_block {
            Some(slug) if !html.is_empty() => {
                out.push(json!({"blockType": slug, "html": html}));
                "; kept as raw HTML"
            }
            _ => "",
        };
        self.issue(
            IssueKind::UnsupportedBlock,
            Severity::Error,
            format!("no [[mapping.blocks]] entry matches {kind} block{kept}"),
            json!({"kind": kind, "module": module}),
        );
    }

    /// The block's `html` prop, else its rich text and text props as HTML.
    fn block_html(&mut self, b: &Block) -> String {
        if let Some(FieldValue::Text(h)) = b.props.get("html") {
            return h.clone();
        }
        let mut html = String::new();
        for v in b.props.values() {
            match v {
                FieldValue::RichText(rt) => html.push_str(&to_html(rt, self.ids, &mut self.issues)),
                FieldValue::Text(t) => html.push_str(&format!(
                    "<p>{}</p>",
                    t.replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                )),
                _ => {}
            }
        }
        html
    }

    /// A prop as a Payload field value; `None` (field omitted) for nulls and unresolved refs.
    fn value(&mut self, v: &FieldValue) -> Option<Value> {
        Some(match v {
            FieldValue::Null => return None,
            FieldValue::Text(s) => json!(s),
            FieldValue::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => json!(*n as i64),
            FieldValue::Number(n) => json!(n),
            FieldValue::Bool(b) => json!(b),
            FieldValue::Date(d) => json!(d.to_rfc3339()),
            FieldValue::RichText(rt) => {
                let (v, issues) = to_lexical(rt, self.ids, self.raw_block);
                self.issues.extend(issues);
                v
            }
            FieldValue::Layout(bs) => Value::Array(self.blocks(bs)),
            // ponytail: plain id fits upload and single-collection relationship fields;
            // polymorphic ones need `{relationTo, value}` once the schema stage knows.
            FieldValue::Media(id) | FieldValue::Doc(id) => match self.ids.get(*id) {
                Some(d) => d.id,
                None => {
                    self.issue(
                        IssueKind::UnresolvedRelation,
                        Severity::Warning,
                        format!("{id} was not loaded; field left empty"),
                        json!({"item": id, "url": self.ids.old_url(*id)}),
                    );
                    return None;
                }
            },
            FieldValue::List(xs) => Value::Array(xs.iter().filter_map(|x| self.value(x)).collect()),
            FieldValue::Object(o) => Value::Object(
                o.iter()
                    .filter_map(|(k, x)| Some((k.clone(), self.value(x)?)))
                    .collect(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destinations::payload::lexical::tests::{id, ids, txt};
    use crate::mapping::BlockMatch;
    use indexmap::IndexMap;
    use porter_model::{
        Align, BlockKind, FieldValue, IssueKind, ItemKind, Original, RichText, RtBlock, Severity,
    };
    use serde_json::json;

    fn b(kind: BlockKind, children: Vec<Block>) -> Block {
        Block {
            kind,
            props: IndexMap::new(),
            children,
            original: None,
        }
    }

    fn map(kind: &str, to: &str, fields: &[(&str, &str)]) -> BlockMap {
        BlockMap {
            r#match: BlockMatch {
                kind: Some(kind.into()),
                module: None,
            },
            to: to.into(),
            fields: fields
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }

    fn image(media: &str) -> Block {
        let mut img = b(BlockKind::Image, vec![]);
        img.props.insert(
            "media".into(),
            FieldValue::Media(id(ItemKind::Media, media)),
        );
        img
    }

    #[test]
    fn blocks_flatten_containers_and_map() {
        let tree = b(
            BlockKind::Section,
            vec![b(
                BlockKind::Row,
                vec![b(BlockKind::Column { width: None }, vec![image("pic")])],
            )],
        );
        let maps = [map("image", "mediaBlock", &[("media", "media")])];
        let (v, issues) = to_blocks(&[tree], &maps, &ids(), None);
        assert_eq!(v, vec![json!({"blockType": "mediaBlock", "media": 3})]);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn blocks_map_props_by_type() {
        let mut rt = b(BlockKind::RichText, vec![]);
        rt.props.insert(
            "text".into(),
            FieldValue::RichText(RichText {
                blocks: vec![RtBlock::Paragraph {
                    inlines: vec![txt("hi")],
                    align: Align::None,
                }],
            }),
        );
        rt.props
            .insert("title".into(), FieldValue::Text("T".into()));
        rt.props.insert("empty".into(), FieldValue::Null);
        rt.props.insert(
            "page".into(),
            FieldValue::Doc(id(ItemKind::Document, "about")),
        );
        rt.props.insert("unmapped".into(), FieldValue::Bool(true));
        let maps = [map(
            "rich-text",
            "richText",
            &[
                ("text", "content"),
                ("title", "heading"),
                ("empty", "x"),
                ("page", "page"),
            ],
        )];
        let (v, issues) = to_blocks(&[rt, image("missing")], &maps, &ids(), None);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0]["blockType"], "richText");
        assert_eq!(
            v[0]["content"]["root"]["children"][0]["children"][0]["text"],
            "hi"
        );
        assert_eq!(v[0]["heading"], "T");
        assert_eq!(v[0]["page"], 7);
        assert!(v[0].get("x").is_none(), "nulls are omitted");
        // `unmapped` reported; the unmatched image is an error.
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert_eq!(issues[0].kind, IssueKind::UnmappedField);
        assert_eq!(issues[1].kind, IssueKind::UnsupportedBlock);
    }

    #[test]
    fn blocks_unmatched_is_error_issue() {
        let mut raw = b(BlockKind::Raw, vec![]);
        raw.props
            .insert("html".into(), FieldValue::Text("<i>x</i>".into()));
        raw.original = Some(Original {
            builder: "b".into(),
            module: "mod_x".into(),
            classes: vec![],
            settings: json!({}),
        });
        let (v, issues) = to_blocks(&[raw.clone()], &[], &ids(), None);
        assert!(v.is_empty());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::UnsupportedBlock);
        assert_eq!(issues[0].severity, Severity::Error);
        assert_eq!(issues[0].details["module"], "mod_x");

        // With a raw block configured the content still lands.
        let (v, issues) = to_blocks(&[raw.clone()], &[], &ids(), Some("raw"));
        assert_eq!(v, vec![json!({"blockType": "raw", "html": "<i>x</i>"})]);
        assert_eq!(issues[0].severity, Severity::Error);

        // Module match wins over kind.
        let by_module = BlockMap {
            r#match: BlockMatch {
                kind: None,
                module: Some("mod_x".into()),
            },
            to: "custom".into(),
            fields: [("html".to_string(), "code".to_string())]
                .into_iter()
                .collect(),
        };
        let (v, issues) = to_blocks(&[raw], &[by_module], &ids(), Some("raw"));
        assert_eq!(v, vec![json!({"blockType": "custom", "code": "<i>x</i>"})]);
        assert!(issues.is_empty());
    }

    #[test]
    fn blocks_matched_container_keeps_children() {
        let mut col = b(BlockKind::Column { width: None }, vec![image("pic")]);
        col.props
            .insert("bg".into(), FieldValue::Text("dark".into()));
        let maps = [
            map("column", "column", &[("bg", "bg"), ("children", "items")]),
            map("image", "image", &[("media", "image")]),
        ];
        let (v, issues) = to_blocks(&[col], &maps, &ids(), None);
        assert_eq!(
            v,
            vec![json!({"blockType": "column", "bg": "dark",
                "items": [{"blockType": "image", "image": 3}]})]
        );
        assert!(issues.is_empty(), "{issues:?}");
    }
}
