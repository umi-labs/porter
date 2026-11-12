//! Payload CMS destination, written through Payload's REST API.
pub mod blocks;
pub mod client;
pub mod lexical;
pub mod schema_check;
pub mod schema_generate;
pub mod schema_prompt;
pub mod ts;

use crate::config::DestinationConfig;
use crate::mapping::summary::TypeSummary;
use crate::stage::{Capabilities, Destination, Feature, SchemaMode, StageCtx};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use client::PayloadClient;
use porter_model::ItemId;

const DEFAULT_KEY_ENV: &str = "PORTER_PAYLOAD_API_KEY";

pub struct PayloadDestination {
    cfg: DestinationConfig,
}

impl PayloadDestination {
    pub fn new(cfg: &DestinationConfig) -> Self {
        Self { cfg: cfg.clone() }
    }

    /// Reads the key from the env var named in config; it is never logged or stored.
    fn client(&self) -> Result<PayloadClient> {
        let var = match self.cfg.api_key_env.as_str() {
            "" => DEFAULT_KEY_ENV,
            v => v,
        };
        let key = match std::env::var(var) {
            Ok(k) if !k.is_empty() => k,
            _ => bail!("environment variable {var} is not set (destination.api_key_env)"),
        };
        PayloadClient::new(&self.cfg.url, &key, &self.cfg.auth_collection)
    }
}

#[async_trait]
impl Destination for PayloadDestination {
    fn capabilities(&self) -> Capabilities {
        use Feature::*;
        Capabilities {
            reads: Default::default(),
            writes: [
                Layouts,
                RichText,
                Media,
                Forms,
                Submissions,
                Locales,
                Drafts,
                Menus,
                Redirects,
                Taxonomies,
                Seo,
            ]
            .into(),
        }
    }

    async fn schema(&self, mode: SchemaMode, ctx: &StageCtx) -> Result<()> {
        let summary = TypeSummary::from_store(ctx.project.store())?;
        let mapping = &ctx.config.mapping;
        let project = ctx.project.dir();
        let config_dir = self.cfg.config_dir.as_ref().map(|d| project.join(d));
        let reports = ctx.project.reports_dir();
        let mut written = vec![];
        match mode {
            SchemaMode::Check => {
                let Some(dir) = config_dir else {
                    bail!(
                        "destination.config_dir is not set: point it at the Payload site's source folder"
                    );
                };
                let dest = ts::read_payload_config(&dir)?;
                let issues = schema_check::check(&summary, mapping, &dest);
                let json: Vec<_> = issues
                    .iter()
                    .map(|i| {
                        serde_json::json!({
                            "kind": i.kind,
                            "severity": i.severity,
                            "message": i.message,
                            "details": i.details,
                        })
                    })
                    .collect();
                let p = reports.join("schema-check.json");
                write(&p, &serde_json::to_string_pretty(&json)?)?;
                written.push(p);
                for i in issues {
                    ctx.issue(i.kind, i.severity, None, i.message, i.details)?;
                }
            }
            SchemaMode::Generate => {
                let out = project.join("generated").join("payload");
                written = schema_generate::generate(&summary, mapping, &out)?;
            }
            SchemaMode::Prompt => {
                let dest = config_dir.unwrap_or_else(|| project.join("<payload site>"));
                let p = reports.join("schema-prompt.md");
                write(
                    &p,
                    &schema_prompt::prompt(&summary, mapping, &dest, project),
                )?;
                written.push(p);
            }
            SchemaMode::Spec => {
                let dest = config_dir.unwrap_or_else(|| project.join("<payload site>"));
                let (md, html) = schema_prompt::spec(&summary, mapping, &dest, project);
                for (ext, text) in [("md", md), ("html", html)] {
                    let p = reports.join(format!("schema-spec.{ext}"));
                    write(&p, &text)?;
                    written.push(p);
                }
            }
        }
        let n = written.len() as u64;
        for (i, p) in written.iter().enumerate() {
            ctx.progress(i as u64 + 1, Some(n), format!("wrote {}", p.display()));
        }
        Ok(())
    }

    async fn load(&self, _ctx: &StageCtx, _apply: bool) -> Result<()> {
        bail!("not implemented yet")
    }

    async fn count(&self, _ctx: &StageCtx, dest_type: &str) -> Result<u64> {
        self.client()?.count(dest_type).await
    }

    async fn fetch_loaded(
        &self,
        _ctx: &StageCtx,
        _id: ItemId,
    ) -> Result<Option<serde_json::Value>> {
        // Task 21 adds the dest_ids table this needs.
        Ok(None)
    }
}

fn write(path: &std::path::Path, text: &str) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("create {}", d.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_env_names_the_var() {
        let cfg = DestinationConfig {
            api_key_env: "PORTER_TEST_PAYLOAD_KEY_UNSET".into(),
            ..Default::default()
        };
        let e = PayloadDestination::new(&cfg)
            .client()
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("PORTER_TEST_PAYLOAD_KEY_UNSET"), "{e}");
    }
}
