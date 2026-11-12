//! Payload CMS destination, written through Payload's REST API.
pub mod client;

use crate::config::DestinationConfig;
use crate::stage::{Capabilities, Destination, Feature, SchemaMode, StageCtx};
use anyhow::{Result, bail};
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

    async fn schema(&self, _mode: SchemaMode, _ctx: &StageCtx) -> Result<()> {
        bail!("not implemented yet")
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
