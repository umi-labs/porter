//! Sanity source: reads a dataset through the API or from a `sanity dataset export`.
//! Connectors, normalise and assets land in later tasks; this is the skeleton.

use crate::config::ConnectorConfig;
use crate::connectors::crawl::CrawlConnector;
use crate::stage::{Capabilities, Connector, Feature, Source, StageCtx};
use anyhow::{Result, bail};
use async_trait::async_trait;

/// Connector name of normalised items.
pub(crate) const CANON: &str = "sanity";
/// Connector names of the raw rows.
const API_NAME: &str = "sanity-api";
const EXPORT_NAME: &str = "sanity-export";

pub struct SanitySource;

impl Source for SanitySource {
    fn canonical(&self) -> &'static str {
        CANON
    }

    fn connectors(&self, cfg: &crate::config::ProjectConfig) -> Result<Vec<Box<dyn Connector>>> {
        let mut out: Vec<Box<dyn Connector>> = vec![];
        let mut reads_dataset = false;
        for c in &cfg.connectors {
            match c {
                ConnectorConfig::SanityApi { .. } => {
                    reads_dataset = true;
                    out.push(Box::new(SanityConnector { name: API_NAME }));
                }
                ConnectorConfig::SanityExport { .. } => {
                    reads_dataset = true;
                    out.push(Box::new(SanityConnector { name: EXPORT_NAME }));
                }
                ConnectorConfig::Crawl { .. } => out.push(Box::new(CrawlConnector::new(c))),
                _ => bail!(
                    "only \"sanity-api\", \"sanity-export\" and \"crawl\" connectors apply to a sanity source"
                ),
            }
        }
        if !reads_dataset {
            bail!(
                "a sanity source needs a [[connectors]] entry: kind = \"sanity-api\" or \"sanity-export\""
            );
        }
        Ok(out)
    }

    fn normalise(&self, _ctx: &StageCtx) -> Result<()> {
        Ok(())
    }
}

/// Stub until the API and export readers land.
struct SanityConnector {
    name: &'static str,
}

#[async_trait]
impl Connector for SanityConnector {
    fn name(&self) -> &str {
        self.name
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            reads: [Feature::Media].into_iter().collect(),
            writes: Default::default(),
        }
    }

    async fn analyse(&self, _ctx: &StageCtx) -> Result<()> {
        Ok(())
    }

    async fn extract(&self, _ctx: &StageCtx) -> Result<()> {
        Ok(())
    }
}
