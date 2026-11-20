//! Portable Text -> neutral rich text.

use super::shape::asset_ref;
use crate::html::PendingIssue;
use porter_model::{
    Align, Inline, IssueKind, ItemId, LinkTarget, Marks, RichText, RtBlock, Severity,
};
use serde_json::{Value, json};

// used by normalise (Task 6)
#[allow(dead_code)]
pub(crate) struct PtCx<'a> {
    pub resolve_doc: &'a dyn Fn(&str) -> Option<ItemId>,
    pub resolve_asset: &'a dyn Fn(&str) -> Option<ItemId>,
    pub issues: &'a mut Vec<PendingIssue>,
}

impl PtCx<'_> {
    fn warn(&mut self, kind: IssueKind, message: String, details: Value) {
        self.issues.push(PendingIssue {
            kind,
            severity: Severity::Warning,
            message,
            details,
        });
    }
}

/// One list-item block, waiting to be grouped into a nested list.
struct ListEntry {
    level: u64,
    ordered: bool,
    block: RtBlock,
}

// used by normalise (Task 6)
#[allow(dead_code)]
pub(crate) fn pt_to_richtext(blocks: &[Value], cx: &mut PtCx) -> RichText {
    let mut out = vec![];
    let mut run: Vec<ListEntry> = vec![];
    for item in blocks {
        let ty = item.get("_type").and_then(Value::as_str);
        if ty == Some("block") && item.get("listItem").is_some() {
            run.push(ListEntry {
                level: item.get("level").and_then(Value::as_u64).unwrap_or(1),
                ordered: item.get("listItem").and_then(Value::as_str) == Some("number"),
                block: paragraph(item, cx),
            });
            continue;
        }
        flush_lists(&mut run, &mut out);
        match ty {
            Some("block") => out.push(styled(item, cx)),
            Some("image") => {
                // alt has no slot here; normalise (Task 6) sets Media.alt from the asset's first use.
                let Some(aref) = asset_ref(item) else {
                    cx.warn(
                        IssueKind::MissingMedia,
                        "image has no asset reference".to_string(),
                        json!({}),
                    );
                    continue;
                };
                match (cx.resolve_asset)(aref) {
                    Some(media) => {
                        let caption = match item.get("caption") {
                            Some(Value::String(s)) if !s.is_empty() => Some(RichText {
                                blocks: vec![RtBlock::Paragraph {
                                    inlines: vec![Inline::Text {
                                        text: s.clone(),
                                        marks: Marks::default(),
                                    }],
                                    align: Align::default(),
                                }],
                            }),
                            Some(Value::Array(a)) if !a.is_empty() => Some(pt_to_richtext(a, cx)),
                            _ => None,
                        };
                        out.push(RtBlock::Media {
                            media,
                            caption,
                            align: Align::default(),
                            width: None,
                            link: None,
                        })
                    }
                    None => cx.warn(
                        IssueKind::MissingMedia,
                        format!("image asset {aref} not found"),
                        json!({"ref": aref}),
                    ),
                }
            }
            Some(t) => {
                cx.warn(
                    IssueKind::ConverterMissing,
                    format!("no converter for Portable Text object type {t}"),
                    json!({"type": t}),
                );
                out.push(RtBlock::Raw {
                    html: item.to_string(),
                });
            }
            None => {}
        }
    }
    flush_lists(&mut run, &mut out);
    RichText { blocks: out }
}

fn flush_lists(run: &mut Vec<ListEntry>, out: &mut Vec<RtBlock>) {
    let entries = std::mem::take(run);
    let mut pos = 0;
    while pos < entries.len() {
        let level = entries[pos].level;
        out.push(build_list(&entries, &mut pos, level));
    }
}

/// Consumes entries from `pos` at `level`, nesting deeper ones under the previous item.
/// Always consumes at least the entry at `pos`.
fn build_list(entries: &[ListEntry], pos: &mut usize, level: u64) -> RtBlock {
    let ordered = entries[*pos].ordered;
    let mut items: Vec<Vec<RtBlock>> = vec![];
    while *pos < entries.len() {
        let e = &entries[*pos];
        if e.level < level {
            break;
        }
        if e.level > level {
            let nested = build_list(entries, pos, e.level);
            match items.last_mut() {
                Some(last) => last.push(nested),
                None => items.push(vec![nested]),
            }
        } else if e.ordered != ordered {
            break;
        } else {
            items.push(vec![e.block.clone()]);
            *pos += 1;
        }
    }
    RtBlock::List {
        ordered,
        start: 1,
        items,
    }
}

fn styled(block: &Value, cx: &mut PtCx) -> RtBlock {
    let style = block
        .get("style")
        .and_then(Value::as_str)
        .unwrap_or("normal");
    let inlines = inlines(block, cx);
    let align = Align::default();
    if let Some(level) = style
        .strip_prefix('h')
        .and_then(|n| n.parse::<u8>().ok())
        .filter(|n| (1..=6).contains(n))
    {
        return RtBlock::Heading {
            level,
            inlines,
            align,
        };
    }
    let p = RtBlock::Paragraph { inlines, align };
    if style == "blockquote" {
        RtBlock::Quote { blocks: vec![p] }
    } else {
        p
    }
}

fn paragraph(block: &Value, cx: &mut PtCx) -> RtBlock {
    RtBlock::Paragraph {
        inlines: inlines(block, cx),
        align: Align::default(),
    }
}

fn inlines(block: &Value, cx: &mut PtCx) -> Vec<Inline> {
    let defs: &[Value] = block
        .get("markDefs")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let mut out: Vec<Inline> = vec![];
    let mut last_link: Option<String> = None;
    let children = block.get("children").and_then(Value::as_array);
    for child in children.into_iter().flatten() {
        if child.get("_type").and_then(Value::as_str) != Some("span") {
            let t = child
                .get("_type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            cx.warn(
                IssueKind::ConverterMissing,
                format!("no converter for inline object type {t}"),
                json!({"type": t}),
            );
            continue;
        }
        let text = child
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut marks = Marks::default();
        let mut link: Option<(String, LinkTarget, bool)> = None;
        let names = child.get("marks").and_then(Value::as_array);
        for m in names.into_iter().flatten().filter_map(Value::as_str) {
            match m {
                "strong" => marks.bold = true,
                "em" => marks.italic = true,
                "underline" => marks.underline = true,
                "code" => marks.code = true,
                "strike-through" => marks.strike = true,
                "sub" => marks.sub = true,
                "sup" => marks.sup = true,
                key => {
                    if link.is_none()
                        && let Some(def) = defs.iter().find(|d| d["_key"] == key)
                        && let Some(t) = link_target(def, cx)
                    {
                        let blank = def.get("blank").and_then(Value::as_bool).unwrap_or(false);
                        link = Some((key.to_string(), t, blank));
                    }
                }
            }
        }
        let span = Inline::Text {
            text: text.to_string(),
            marks,
        };
        match link {
            Some((key, target, new_tab)) => {
                if last_link.as_deref() == Some(&key)
                    && let Some(Inline::Link { inlines, .. }) = out.last_mut()
                {
                    inlines.push(span);
                } else {
                    out.push(Inline::Link {
                        target,
                        new_tab,
                        inlines: vec![span],
                        fragment: None,
                    });
                }
                last_link = Some(key);
            }
            None => {
                out.push(span);
                last_link = None;
            }
        }
    }
    out
}

/// None leaves the text unlinked (after pushing an issue when the def is a lost internal link).
fn link_target(def: &Value, cx: &mut PtCx) -> Option<LinkTarget> {
    if let Some(href) = def.get("href").and_then(Value::as_str) {
        return Some(LinkTarget::Url(href.to_string()));
    }
    let refd = def
        .as_object()?
        .values()
        .find_map(|v| v.get("_ref").and_then(Value::as_str));
    match refd {
        Some(r) => match (cx.resolve_doc)(r) {
            Some(id) => Some(LinkTarget::Doc(id)),
            None => {
                cx.warn(
                    IssueKind::UnresolvedLink,
                    format!("internal link to {r} not found"),
                    json!({"ref": r}),
                );
                None
            }
        },
        None => {
            let t = def
                .get("_type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            cx.warn(
                IssueKind::ConverterMissing,
                format!("no converter for annotation type {t}"),
                json!({"type": t}),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn id(n: u128) -> ItemId {
        ItemId(Uuid::from_u128(n))
    }

    fn t(text: &str, marks: Marks) -> Inline {
        Inline::Text {
            text: text.into(),
            marks,
        }
    }

    fn p(inlines: Vec<Inline>) -> RtBlock {
        RtBlock::Paragraph {
            inlines,
            align: Align::None,
        }
    }

    fn run(blocks: Vec<Value>) -> (RichText, Vec<PendingIssue>) {
        let mut issues = vec![];
        let rd = |r: &str| (r == "doc-1").then(|| id(1));
        let ra = |r: &str| (r == "image-abc-1x1-png").then(|| id(2));
        let mut cx = PtCx {
            resolve_doc: &rd,
            resolve_asset: &ra,
            issues: &mut issues,
        };
        let rt = pt_to_richtext(&blocks, &mut cx);
        (rt, issues)
    }

    fn block(style: &str, children: Value) -> Value {
        json!({"_type":"block","style":style,"children":children,"markDefs":[]})
    }

    fn li(kind: &str, level: u64, text: &str) -> Value {
        json!({"_type":"block","style":"normal","listItem":kind,"level":level,
               "children":[{"_type":"span","text":text,"marks":[]}],"markDefs":[]})
    }

    #[test]
    fn pt_styles_lists_marks_links() {
        let plain = Marks::default();
        let (rt, issues) = run(vec![
            block("h2", json!([{"_type":"span","text":"Title","marks":[]}])),
            block(
                "blockquote",
                json!([{"_type":"span","text":"Quote","marks":[]}]),
            ),
            li("bullet", 1, "one"),
            li("bullet", 2, "nested"),
            li("bullet", 1, "two"),
            json!({"_type":"block","style":"normal","markDefs":[
                {"_key":"k1","_type":"link","href":"https://example.com"},
                {"_key":"k2","_type":"link","reference":{"_ref":"doc-1","_type":"reference"}}],
              "children":[
                {"_type":"span","text":"b","marks":["strong"]},
                {"_type":"span","text":"i","marks":["em"]},
                {"_type":"span","text":"u","marks":["underline"]},
                {"_type":"span","text":"c","marks":["code"]},
                {"_type":"span","text":"s","marks":["strike-through"]},
                {"_type":"span","text":"ext","marks":["k1"]},
                {"_type":"span","text":"int","marks":["k2"]}]}),
            json!({"_type":"image","asset":{"_ref":"image-abc-1x1-png"}}),
        ]);
        assert!(issues.is_empty(), "{issues:?}");
        let m = |f: fn(&mut Marks)| {
            let mut x = Marks::default();
            f(&mut x);
            x
        };
        assert_eq!(
            rt.blocks,
            vec![
                RtBlock::Heading {
                    level: 2,
                    inlines: vec![t("Title", plain)],
                    align: Align::None
                },
                RtBlock::Quote {
                    blocks: vec![p(vec![t("Quote", plain)])]
                },
                RtBlock::List {
                    ordered: false,
                    start: 1,
                    items: vec![
                        vec![
                            p(vec![t("one", plain)]),
                            RtBlock::List {
                                ordered: false,
                                start: 1,
                                items: vec![vec![p(vec![t("nested", plain)])]],
                            },
                        ],
                        vec![p(vec![t("two", plain)])],
                    ],
                },
                p(vec![
                    t("b", m(|x| x.bold = true)),
                    t("i", m(|x| x.italic = true)),
                    t("u", m(|x| x.underline = true)),
                    t("c", m(|x| x.code = true)),
                    t("s", m(|x| x.strike = true)),
                    Inline::Link {
                        target: LinkTarget::Url("https://example.com".into()),
                        new_tab: false,
                        inlines: vec![t("ext", plain)],
                        fragment: None,
                    },
                    Inline::Link {
                        target: LinkTarget::Doc(id(1)),
                        new_tab: false,
                        inlines: vec![t("int", plain)],
                        fragment: None,
                    },
                ]),
                RtBlock::Media {
                    media: id(2),
                    caption: None,
                    align: Align::None,
                    width: None,
                    link: None,
                },
            ]
        );
    }

    #[test]
    fn pt_ordered_list_and_unresolved_link() {
        let (rt, issues) = run(vec![
            li("number", 1, "a"),
            json!({"_type":"block","style":"normal",
              "markDefs":[{"_key":"k","_type":"link","internal":{"_ref":"gone","_type":"reference"}}],
              "children":[{"_type":"span","text":"x","marks":["k"]}]}),
            json!({"_type":"image","asset":{"_ref":"image-zzz-1x1-png"}}),
        ]);
        assert!(matches!(&rt.blocks[0], RtBlock::List { ordered: true, .. }));
        assert_eq!(rt.blocks[1], p(vec![t("x", Marks::default())]));
        assert_eq!(rt.blocks.len(), 2);
        let kinds: Vec<_> = issues.iter().map(|i| (i.kind, i.severity)).collect();
        assert_eq!(
            kinds,
            vec![
                (IssueKind::UnresolvedLink, Severity::Warning),
                (IssueKind::MissingMedia, Severity::Warning)
            ]
        );
        assert!(issues[0].message.contains("gone"));
    }

    #[test]
    fn pt_image_without_ref_warns_and_caption_is_carried() {
        let (rt, issues) = run(vec![
            json!({"_type":"image","alt":"x"}),
            json!({"_type":"image","asset":{"_ref":"image-abc-1x1-png"},"caption":"Hi"}),
        ]);
        assert_eq!(rt.blocks.len(), 1);
        assert!(matches!(
            &rt.blocks[0],
            RtBlock::Media { caption: Some(c), .. }
                if c.blocks == vec![p(vec![t("Hi", Marks::default())])]
        ));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::MissingMedia);
        assert_eq!(issues[0].severity, Severity::Warning);
    }

    #[test]
    fn pt_unknown_inline_object_is_converter_missing() {
        let obj = json!({"_type":"youtube","url":"https://y.t/1"});
        let (rt, issues) = run(vec![obj.clone()]);
        assert_eq!(
            rt.blocks,
            vec![RtBlock::Raw {
                html: obj.to_string()
            }]
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::ConverterMissing);
        assert_eq!(issues[0].severity, Severity::Warning);
    }
}
