//! Sanity source: reads a dataset through the API or from a `sanity dataset export`.
//! Normalise and assets land in later tasks.

mod export;
mod portable_text;
mod shape;

use crate::config::ConnectorConfig;
use crate::connectors::RawItem;
use crate::connectors::crawl::CrawlConnector;
use crate::stage::{Capabilities, Connector, Feature, Source, StageCtx};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use export::{DATA_FILE, SanityApiCfg};
use porter_model::{ItemId, ItemKind, ItemStatus, SourceRef};
use serde_json::Value;

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
                    out.push(Box::new(SanityConnector::new(API_NAME, c.clone())));
                }
                ConnectorConfig::SanityExport { .. } => {
                    reads_dataset = true;
                    out.push(Box::new(SanityConnector::new(EXPORT_NAME, c.clone())));
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

struct SanityConnector {
    name: &'static str,
    cfg: ConnectorConfig,
}

impl SanityConnector {
    fn new(name: &'static str, cfg: ConnectorConfig) -> Self {
        Self { name, cfg }
    }

    /// Fetches or reads the export into `raw/sanity/data.ndjson`.
    async fn read_export(&self, ctx: &StageCtx) -> Result<()> {
        let out = ctx.project.raw_dir(CANON);
        match &self.cfg {
            ConnectorConfig::SanityApi {
                project_id,
                dataset,
                token_env,
                api_version,
                ..
            } => {
                let token = std::env::var(token_env)
                    .ok()
                    .filter(|t| !t.trim().is_empty())
                    .with_context(|| {
                        format!("set the env var {token_env} to a Sanity read token (Viewer role)")
                    })?;
                // PORTER_SANITY_API_BASE is for tests only: it points the fetch at a mock server.
                let base = std::env::var("PORTER_SANITY_API_BASE")
                    .unwrap_or_else(|_| format!("https://{project_id}.api.sanity.io"));
                let cfg = SanityApiCfg {
                    project_id: project_id.clone(),
                    dataset: dataset.clone(),
                    api_version: api_version.clone(),
                    token_env: token_env.clone(),
                };
                export::fetch_api(&cfg, &base, token.trim(), &out, &ctx.cancel).await
            }
            ConnectorConfig::SanityExport { path, .. } => {
                export::read_archive(&ctx.project.dir().join(path), &out)
            }
            _ => bail!("not a sanity connector"),
        }
    }
}

fn row(doc: &Value) -> RawItem {
    let id = doc.get("_id").and_then(Value::as_str).unwrap_or("");
    let text = |k: &str| doc.get(k).and_then(Value::as_str).map(str::to_string);
    RawItem {
        label: text("_type").unwrap_or_default(),
        title: text("title").or_else(|| text("name")),
        status: Some(
            if id.starts_with("drafts.") {
                "draft"
            } else {
                "published"
            }
            .into(),
        ),
        url: None,
        raw_path: None,
    }
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

    async fn analyse(&self, ctx: &StageCtx) -> Result<()> {
        self.read_export(ctx).await?;
        let ex = export::load(&ctx.project.raw_dir(CANON))?;
        for doc in &ex.docs {
            let Some(sid) = doc.get("_id").and_then(Value::as_str) else {
                continue;
            };
            let src = SourceRef {
                connector: self.name.into(),
                source_id: sid.into(),
                url: None,
            };
            let id = ItemId::new(ItemKind::Document, &src);
            ctx.project
                .store()
                .put_item(ItemKind::Document, id, &src, &row(doc))?;
            ctx.item_status(id, ItemKind::Document, ItemStatus::Done)?;
        }
        Ok(())
    }

    async fn extract(&self, ctx: &StageCtx) -> Result<()> {
        // The export is re-read only on --refresh (or when analyse never saved it).
        if ctx.refresh || !ctx.project.raw_dir(CANON).join(DATA_FILE).is_file() {
            self.read_export(ctx).await?;
        }
        // Asset download lands in a later task.
        Ok(())
    }
}
