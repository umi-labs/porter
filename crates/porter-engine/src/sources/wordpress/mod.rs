mod acf;
pub mod analyse;
pub mod autop;
pub mod db;
pub mod divi;
mod forms;
pub mod gutenberg;
mod load;
mod media;
pub mod normalise;
mod redirects;
pub mod rest;
mod seo;
pub mod shortcodes;
pub mod texturize;
pub mod wxr;

use crate::config::{ConnectorConfig, ProjectConfig};
use crate::connectors::crawl::CrawlConnector;
use crate::html::{HtmlCtx, html_to_richtext};
use crate::stage::{Capabilities, Connector, Source, StageCtx};
use anyhow::{Result, bail};
use async_trait::async_trait;
use porter_model::{Align, Block, FieldValue, IssueKind, ItemId, RichText, RtBlock, Severity};
use regex::Regex;
use serde_json::json;
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

pub struct WordPressSource;

/// Priority: wp-db > wp-rest > wxr > crawl.
fn rank(c: &ConnectorConfig) -> Result<(u8, Box<dyn Connector>)> {
    Ok(match c {
        ConnectorConfig::WpDb { .. } => (0, Box::new(db::WpDbConnector::new(c))),
        ConnectorConfig::WpRest { .. } => (1, Box::new(rest::WpRestConnector::new(c))),
        ConnectorConfig::Wxr { .. } => (2, Box::new(wxr::WxrConnector::new(c))),
        ConnectorConfig::Crawl { .. } => (3, Box::new(CrawlConnector::new(c))),
        ConnectorConfig::UmbracoJson { .. } => {
            bail!("connector kind \"umbraco-json\" does not apply to a wordpress source")
        }
    })
}

impl Source for WordPressSource {
    fn canonical(&self) -> &'static str {
        normalise::CONNECTOR
    }

    fn connectors(&self, cfg: &ProjectConfig) -> Result<Vec<Box<dyn Connector>>> {
        let mut v = cfg
            .connectors
            .iter()
            .map(rank)
            .collect::<Result<Vec<_>>>()?;
        v.sort_by_key(|(r, _)| *r);
        let mut out: Vec<_> = v.into_iter().map(|(_, c)| c).collect();
        out.push(Box::new(Merge));
        Ok(out)
    }

    fn normalise(&self, ctx: &StageCtx) -> Result<()> {
        normalise::run(ctx)
    }
}

/// Runs last: merges what the others found. Reads only the store, so extract is a no-op.
struct Merge;

#[async_trait]
impl Connector for Merge {
    fn name(&self) -> &str {
        "wp-merge"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    async fn analyse(&self, ctx: &StageCtx) -> Result<()> {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        analyse::merge(ctx).map(|_| ())
    }

    async fn extract(&self, _ctx: &StageCtx) -> Result<()> {
        Ok(())
    }
}

/// Classic-editor content as WordPress renders it: `[caption]`s as figures and
/// `[gallery]`s as images (through `media_ids`, attachment post id -> media), wptexturize,
/// wpautop, then HTML. Other shortcodes stay as text, each tag flagged once.
pub fn classic_to_richtext(
    content: &str,
    media_ids: &HashMap<u64, ItemId>,
    ctx: &mut HtmlCtx,
) -> RichText {
    let rt = classic_inner(content, media_ids, ctx);
    flag_literal_shortcodes([&rt], ctx);
    rt
}

/// `classic_to_richtext` without the literal-shortcode check (callers check once per item).
pub(crate) fn classic_inner(
    content: &str,
    media_ids: &HashMap<u64, ItemId>,
    ctx: &mut HtmlCtx,
) -> RichText {
    static CLASSIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new("^(caption|gallery)$").expect("valid regex"));
    let html = |s: &str, ctx: &mut HtmlCtx| {
        html_to_richtext(&autop::autop(&texturize::texturize(s)), ctx).blocks
    };
    if !content.contains("[caption") && !content.contains("[gallery") {
        return RichText {
            blocks: html(content, ctx),
        };
    }
    let mut blocks = vec![];
    let mut pending = String::new();
    for n in shortcodes::parse_shortcodes(content, &CLASSIC) {
        match n.tag.as_str() {
            "caption" => pending.push_str(&shortcodes::caption_html(&n)),
            "gallery" => {
                blocks.extend(html(&std::mem::take(&mut pending), ctx));
                let ids = n.attrs.get("ids").map(String::as_str).unwrap_or("");
                if ids.trim().is_empty() {
                    // "Show the post's attached images": nothing here says which.
                    blocks.push(RtBlock::Raw {
                        html: shortcodes::to_source(&n),
                    });
                    ctx.issue(
                        IssueKind::UnsupportedBlock,
                        Severity::Warning,
                        "[gallery] without ids (attached images) kept as raw".into(),
                        json!({ "shortcode": "gallery" }),
                    );
                    continue;
                }
                for id in ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    match id.parse::<u64>().ok().and_then(|i| media_ids.get(&i)) {
                        Some(m) => blocks.push(RtBlock::Media {
                            media: *m,
                            caption: None,
                            align: Align::None,
                            width: None,
                            link: None,
                        }),
                        None => ctx.issue(
                            IssueKind::MissingMedia,
                            Severity::Warning,
                            format!("attachment {id} is not in the media library"),
                            json!({ "attachment": id }),
                        ),
                    }
                }
            }
            _ => pending.push_str(&shortcodes::to_source(&n)),
        }
    }
    blocks.extend(html(&pending, ctx));
    RichText { blocks }
}

/// Every rich-text prop in a layout.
pub(crate) fn layout_richtexts(blocks: &[Block]) -> Vec<&RichText> {
    fn walk<'b>(b: &'b Block, out: &mut Vec<&'b RichText>) {
        for v in b.props.values() {
            if let FieldValue::RichText(rt) = v {
                out.push(rt);
            }
        }
        for c in &b.children {
            walk(c, out);
        }
    }
    let mut out = vec![];
    for b in blocks {
        walk(b, &mut out);
    }
    out
}

/// One `unsupported-block` warning per distinct shortcode tag left visible in the text.
pub(crate) fn flag_literal_shortcodes<'r>(
    texts: impl IntoIterator<Item = &'r RichText>,
    ctx: &mut HtmlCtx,
) {
    static LITERAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([a-z][\w-]*)([^\]]*)\]").expect("valid regex"));
    static KNOWN: LazyLock<Regex> = LazyLock::new(shortcodes::default_known);
    let mut tags = BTreeSet::new();
    for rt in texts {
        // Code is shown as written; brackets in it are not shortcodes.
        let text = RichText {
            blocks: rt
                .blocks
                .iter()
                .filter(|b| !matches!(b, RtBlock::Code { .. }))
                .cloned()
                .collect(),
        }
        .plain_text();
        for c in LITERAL.captures_iter(&text) {
            // Prose like `arr[i]`, `[sic]` or `[note]` is not flagged: only known tags,
            // or tags with an attribute.
            if KNOWN.is_match(&c[1]) || c[2].contains('=') {
                tags.insert(c[1].to_string());
            }
        }
    }
    for tag in tags {
        ctx.issue(
            IssueKind::UnsupportedBlock,
            Severity::Warning,
            format!("[{tag}] left as text"),
            json!({ "shortcode": tag }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::tests::{ctx, fixture};

    #[test]
    fn classic_content() {
        let f = fixture();
        let mut issues = vec![];
        let rt = classic_to_richtext(
            include_str!("../../../tests/fixtures/wp-content/classic.txt"),
            &HashMap::new(),
            &mut ctx(&f, &mut issues),
        );
        let text = rt.plain_text();
        assert!(
            text.starts_with(
                "Opening hours\nWe’re open from 9–5, Monday to Friday.\nClosed on “bank holidays”."
            ),
            "{text}"
        );
        assert!(matches!(
            rt.blocks[2],
            RtBlock::Media {
                width: Some(300),
                ..
            }
        ));
        assert!(matches!(rt.blocks[3], RtBlock::List { .. }));
        assert!(matches!(rt.blocks[4], RtBlock::Raw { .. }));
        match &rt.blocks[5] {
            RtBlock::Media {
                caption: Some(c), ..
            } => assert_eq!(c.plain_text(), "The reading lamp"),
            b => panic!("{b:?}"),
        }
        assert_eq!(rt.blocks.len(), 6);
        assert!(
            issues.iter().all(|i| i.kind != IssueKind::MissingMedia),
            "{issues:?}"
        );
    }

    #[test]
    fn deep_nesting_is_raw() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let f = fixture();
                let deep = |open: &str, close: &str| {
                    format!("{}x{}", open.repeat(5000), close.repeat(5000))
                };
                let unsupported = |issues: &[crate::html::PendingIssue]| {
                    issues
                        .iter()
                        .filter(|i| {
                            i.kind == IssueKind::UnsupportedBlock
                                && i.message.contains("nesting deeper than 256")
                        })
                        .count()
                };

                let mut issues = vec![];
                let src = deep("<div>", "</div>");
                let rt = crate::html::html_to_richtext(&src, &mut ctx(&f, &mut issues));
                assert!(matches!(&rt.blocks[0], RtBlock::Raw { html } if html.contains('x')));
                assert_eq!(unsupported(&issues), 1);

                let mut issues = vec![];
                let src = deep("<span>", "</span>");
                let rt = crate::html::html_to_richtext(&src, &mut ctx(&f, &mut issues));
                assert!(rt.blocks.iter().any(|b| matches!(b, RtBlock::Raw { .. })));
                assert_eq!(unsupported(&issues), 1);

                let mut issues = vec![];
                let src = deep("[et_pb_section]", "[/et_pb_section]");
                let bs = divi::divi_to_layout(
                    &src,
                    &HashMap::new(),
                    &HashMap::new(),
                    &mut ctx(&f, &mut issues),
                );
                assert_eq!(bs.len(), 1);
                assert_eq!(unsupported(&issues), 1);
                let mut raw = String::new();
                bs[0].walk(&mut |b| {
                    if let Some(porter_model::FieldValue::Text(h)) = b.props.get("html") {
                        raw.push_str(h);
                    }
                });
                assert_eq!(raw.matches("[/et_pb_section]").count(), 5000 - 256);

                let mut issues = vec![];
                let src = deep("<!-- wp:group -->", "<!-- /wp:group -->");
                let bs = gutenberg::gutenberg_to_layout(
                    &src,
                    &HashMap::new(),
                    &mut ctx(&f, &mut issues),
                );
                assert_eq!(bs.len(), 1);
                assert_eq!(unsupported(&issues), 1);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn literal_shortcode_flagged() {
        let f = fixture();
        let flagged = |issues: &[crate::html::PendingIssue]| {
            let mut v: Vec<String> = issues
                .iter()
                .filter(|i| i.kind == IssueKind::UnsupportedBlock)
                .map(|i| i.message.clone())
                .collect();
            v.sort();
            v
        };
        let ids = HashMap::from([(7, f.media)]);
        let mut issues = vec![];
        let rt = classic_to_richtext(
            "Intro [contact-form id=\"1\"] and [contact-form] [/x]\n\n[gallery ids=\"7,9\"]\n\nAfter",
            &ids,
            &mut ctx(&f, &mut issues),
        );
        assert_eq!(flagged(&issues), ["[contact-form] left as text"]);
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
        assert_eq!(kinds, ["paragraph", "media", "paragraph"]);
        assert!(
            issues
                .iter()
                .any(|i| i.kind == IssueKind::MissingMedia && i.message.contains('9'))
        );

        let mut issues = vec![];
        divi::divi_to_layout(
            "[et_pb_text]a [foo] b [foo x=1][/et_pb_text][et_pb_text][bar y=2][/et_pb_text]",
            &HashMap::new(),
            &ids,
            &mut ctx(&f, &mut issues),
        );
        assert_eq!(
            flagged(&issues),
            ["[bar] left as text", "[foo] left as text"]
        );

        let mut issues = vec![];
        gutenberg::gutenberg_to_layout(
            "<!-- wp:paragraph --><p>Try [promo code=1]</p><!-- /wp:paragraph -->",
            &HashMap::new(),
            &mut ctx(&f, &mut issues),
        );
        assert_eq!(flagged(&issues), ["[promo] left as text"]);
    }

    #[test]
    fn gallery_without_ids_kept_raw() {
        let f = fixture();
        let mut issues = vec![];
        let rt = classic_to_richtext(
            "Before\n\n[gallery columns=\"2\"]\n\nAfter",
            &HashMap::new(),
            &mut ctx(&f, &mut issues),
        );
        assert!(
            rt.blocks
                .iter()
                .any(|b| matches!(b, RtBlock::Raw { html } if html == "[gallery columns=\"2\" /]")),
            "{rt:?}"
        );
        let msgs: Vec<_> = issues
            .iter()
            .map(|i| (i.kind, i.message.as_str()))
            .collect();
        assert_eq!(
            msgs,
            [(
                IssueKind::UnsupportedBlock,
                "[gallery] without ids (attached images) kept as raw"
            )]
        );
    }

    #[test]
    fn literal_shortcode_scan_ignores_prose_and_code() {
        let f = fixture();
        let mut issues = vec![];
        classic_to_richtext(
            "See arr[i] and [sic] and [note], also [embed].\n\n<pre>[widget size=\"2\"]</pre>",
            &HashMap::new(),
            &mut ctx(&f, &mut issues),
        );
        let msgs: Vec<_> = issues.iter().map(|i| i.message.as_str()).collect();
        assert_eq!(msgs, ["[embed] left as text"]);
    }

    #[test]
    fn gutenberg_loose_html_at_cap_kept() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let f = fixture();
                let mut issues = vec![];
                // At the cap, the innermost group's siblings: loose text and a paragraph block.
                let src = format!(
                    "{}LOOSE-TEXT<!-- wp:paragraph --><p>inner</p><!-- /wp:paragraph -->{}",
                    "<!-- wp:group -->".repeat(crate::html::MAX_DEPTH),
                    "<!-- /wp:group -->".repeat(crate::html::MAX_DEPTH)
                );
                let bs = gutenberg::gutenberg_to_layout(
                    &src,
                    &HashMap::new(),
                    &mut ctx(&f, &mut issues),
                );
                let mut raw = vec![];
                bs[0].walk(&mut |b| {
                    if let Some(porter_model::FieldValue::Text(h)) = b.props.get("html") {
                        raw.push(h.clone());
                    }
                });
                assert_eq!(
                    raw,
                    [
                        "LOOSE-TEXT",
                        "<!-- wp:paragraph --><p>inner</p><!-- /wp:paragraph -->"
                    ]
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
