//! Payload CMS destination, written through Payload's REST API.
pub mod blocks;
pub mod client;
pub mod lexical;
mod load;
pub use load::{DRY_RUN, PREVIEW_DONE};
mod refs;
pub mod schema_check;
pub mod schema_generate;
pub mod schema_prompt;
pub mod ts;

use crate::config::DestinationConfig;
use crate::mapping::summary::TypeSummary;
use crate::stage::{Capabilities, Destination, Feature, LoadedRefs, SchemaMode, StageCtx};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use client::PayloadClient;
use porter_model::ItemId;

/// Where schema mode "generate" writes its Payload collection files, under the project dir.
pub fn generated_dir(project: &std::path::Path) -> std::path::PathBuf {
    project.join("generated").join("payload")
}

const DEFAULT_KEY_ENV: &str = "PORTER_PAYLOAD_API_KEY";

pub struct PayloadDestination {
    cfg: DestinationConfig,
    /// The Payload config, read once for `references` (None: no `config_dir`).
    schema: std::sync::OnceLock<Option<ts::DestSchema>>,
}

impl PayloadDestination {
    pub fn new(cfg: &DestinationConfig) -> Self {
        Self {
            cfg: cfg.clone(),
            schema: Default::default(),
        }
    }

    fn schema(&self, ctx: &StageCtx) -> Result<Option<&ts::DestSchema>> {
        if let Some(s) = self.schema.get() {
            return Ok(s.as_ref());
        }
        let dir = self
            .cfg
            .config_dir
            .as_ref()
            .map(|d| ctx.project.dir().join(d));
        let read = dir.map(|d| ts::read_payload_config(&d)).transpose()?;
        Ok(self.schema.get_or_init(|| read).as_ref())
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
                let out = generated_dir(project);
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

    async fn load(&self, ctx: &StageCtx, apply: bool) -> Result<()> {
        let dir = self
            .cfg
            .config_dir
            .as_ref()
            .map(|d| ctx.project.dir().join(d));
        load::run(ctx, || self.client(), dir, apply).await
    }

    async fn count(&self, _ctx: &StageCtx, dest_type: &str) -> Result<u64> {
        self.client()?.count(dest_type).await
    }

    /// `where[<key>][exists]=true`; media and other keyed loads use `legacyId`.
    async fn count_keyed(
        &self,
        _ctx: &StageCtx,
        dest_type: &str,
        key_field: Option<&str>,
    ) -> Result<u64> {
        let key = key_field.unwrap_or("legacyId");
        self.client()?
            .count_where(
                dest_type,
                &[(format!("where[{key}][exists]"), "true".into())],
            )
            .await
    }

    /// The loaded doc for `id` (depth 0, latest draft); None if it wasn't loaded, landed
    /// in a global, or is gone from Payload.
    async fn fetch_loaded(&self, ctx: &StageCtx, id: ItemId) -> Result<Option<serde_json::Value>> {
        load::fetch(&self.client()?, ctx, id).await
    }

    fn references(
        &self,
        ctx: &StageCtx,
        collection: &str,
        loaded: &serde_json::Value,
    ) -> Result<LoadedRefs> {
        Ok(refs::references(loaded, collection, self.schema(ctx)?))
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
