use crate::mapping::MappingConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `porter.toml`. Every section defaults so a partial file loads;
/// `validate` reports what is missing.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub source: SourceConfig,
    pub connectors: Vec<ConnectorConfig>,
    pub destination: DestinationConfig,
    pub mapping: MappingConfig,
    pub redirects: Vec<ManualRedirect>,
    pub verify: VerifyConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ProjectSection {
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct SourceConfig {
    pub adapter: String,
}

/// One connector, tagged by `kind`. Variant bodies are untyped until the
/// tasks that build each connector give them fields.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ConnectorConfig {
    Crawl {
        url: String,
        #[serde(default = "default_concurrency")]
        concurrency: usize,
        #[serde(default = "default_delay_ms")]
        delay_ms: u64,
        #[serde(default)]
        include: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
        #[serde(default = "default_max_pages")]
        max_pages: Option<usize>,
        /// Query params kept on discovered links; all others are dropped.
        #[serde(default)]
        keep_query: Vec<String>,
        #[serde(default = "default_max_media_bytes")]
        max_media_bytes: u64,
    },
    WpRest {
        /// Site root, e.g. `https://example.com`.
        url: String,
        /// `basic`, `bearer` or `apikey`; the credentials themselves come from the
        /// `PORTER_WP_*` env vars or the keyring entry, never from this file.
        #[serde(default)]
        auth: Option<String>,
        /// Keyring entry name (service `porter`, account = this name).
        #[serde(default)]
        keyring: Option<String>,
    },
    Wxr {
        /// WordPress export file; relative paths resolve against the project folder.
        path: PathBuf,
    },
    WpDb {
        /// SQL dump (plain or gzip); relative paths resolve against the project folder.
        dump: PathBuf,
        /// WordPress core, themes and plugins. Without it only a SQL fallback runs.
        #[serde(default)]
        wp_root: Option<PathBuf>,
        #[serde(default = "default_table_prefix")]
        table_prefix: String,
        /// Overrides the dump's `home` for permalinks.
        #[serde(default)]
        site_url: Option<String>,
    },
    UmbracoJson {
        #[serde(flatten)]
        extra: toml::Table,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct DestinationConfig {
    pub adapter: String,
    pub url: String,
    /// Name of the env var holding the API key; the key itself never goes in config.
    pub api_key_env: String,
    pub auth_collection: String,
    pub config_dir: Option<PathBuf>,
}

impl Default for DestinationConfig {
    fn default() -> Self {
        Self {
            adapter: String::new(),
            url: String::new(),
            api_key_env: String::new(),
            auth_collection: "users".into(),
            config_dir: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ManualRedirect {
    pub from: String,
    pub to: String,
    #[serde(default = "default_code")]
    pub code: u16,
}

fn default_concurrency() -> usize {
    4
}

fn default_max_pages() -> Option<usize> {
    Some(10_000)
}

fn default_max_media_bytes() -> u64 {
    200 * 1024 * 1024
}

fn default_delay_ms() -> u64 {
    250
}

fn default_table_prefix() -> String {
    "wp_".into()
}

fn default_code() -> u16 {
    301
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct VerifyConfig {
    pub site_url: Option<String>,
}

impl ProjectConfig {
    pub fn load(path: &Path) -> Result<ProjectConfig> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Empty means valid. Each message starts with the exact key at fault.
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let mut required = |key: &str, v: &str| {
            if v.trim().is_empty() {
                errs.push(format!("{key} is missing"));
            }
        };
        required("project.name", &self.project.name);
        required("source.adapter", &self.source.adapter);
        required("destination.adapter", &self.destination.adapter);
        required("destination.url", &self.destination.url);
        required("destination.api_key_env", &self.destination.api_key_env);
        required(
            "destination.auth_collection",
            &self.destination.auth_collection,
        );
        let mut url_ok = |key: &str, v: &str| {
            if !v.trim().is_empty() && ::url::Url::parse(v).is_err() {
                errs.push(format!("{key} is not a valid URL: {v}"));
            }
        };
        url_ok("destination.url", &self.destination.url);
        url_ok(
            "verify.site_url",
            self.verify.site_url.as_deref().unwrap_or(""),
        );
        for (i, r) in self.redirects.iter().enumerate() {
            if r.from.trim().is_empty() {
                errs.push(format!("redirects[{i}].from is missing"));
            }
            if r.to.trim().is_empty() {
                errs.push(format!("redirects[{i}].to is missing"));
            }
            if !(300..=399).contains(&r.code) {
                errs.push(format!("redirects[{i}].code must be 3xx, got {}", r.code));
            }
        }
        errs
    }
}
