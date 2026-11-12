pub mod analyse;
pub mod autop;
pub mod db;
pub mod divi;
pub mod gutenberg;
pub mod rest;
pub mod shortcodes;
pub mod texturize;
pub mod wxr;

use crate::config::{ConnectorConfig, ProjectConfig};
use crate::connectors::crawl::CrawlConnector;
use crate::html::{HtmlCtx, html_to_richtext};
use crate::stage::{Capabilities, Connector, Source, StageCtx};
use anyhow::{Result, bail};
use async_trait::async_trait;
use porter_model::RichText;

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

    fn normalise(&self, _ctx: &StageCtx) -> Result<()> {
        bail!("normalise not implemented yet")
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

/// Classic-editor content as WordPress renders it: `[caption]`s as figures, wptexturize,
/// wpautop, then HTML. Other shortcodes stay as text.
pub fn classic_to_richtext(content: &str, ctx: &mut HtmlCtx) -> RichText {
    html_to_richtext(
        &autop::autop(&texturize::texturize(&captions(content))),
        ctx,
    )
}

fn captions(content: &str) -> std::borrow::Cow<'_, str> {
    static CAPTION: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new("^caption$").expect("valid regex"));
    if !content.contains("[caption") {
        return content.into();
    }
    shortcodes::parse_shortcodes(content, &CAPTION)
        .iter()
        .map(|n| match n.tag.as_str() {
            "caption" => shortcodes::caption_html(n),
            _ => shortcodes::to_source(n),
        })
        .collect::<String>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::tests::{ctx, fixture};
    use porter_model::{IssueKind, RtBlock};

    #[test]
    fn classic_content() {
        let f = fixture();
        let mut issues = vec![];
        let rt = classic_to_richtext(
            include_str!("../../../tests/fixtures/wp-content/classic.txt"),
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
}
