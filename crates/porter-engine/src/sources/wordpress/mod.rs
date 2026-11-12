pub mod analyse;
pub mod db;
pub mod rest;
pub mod wxr;

use crate::config::{ConnectorConfig, ProjectConfig};
use crate::connectors::crawl::CrawlConnector;
use crate::stage::{Capabilities, Connector, Source, StageCtx};
use anyhow::{Result, bail};
use async_trait::async_trait;

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
