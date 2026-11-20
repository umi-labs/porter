use crate::mapping::MappingConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `porter.toml`. Every section defaults so a partial file loads;
/// `validate` reports what is missing.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub source: SourceConfig,
    pub connectors: Vec<ConnectorConfig>,
    pub destination: DestinationConfig,
    pub mapping: MappingConfig,
    pub redirects: Vec<ManualRedirect>,
    pub verify: VerifyConfig,
    /// Written by `porter hub link`. Never holds a token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub: Option<HubConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HubConfig {
    /// Hub origin, e.g. `https://hub.example.com`.
    pub url: String,
    /// Project slug on the hub.
    pub project: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct ProjectSection {
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct SourceConfig {
    pub adapter: String,
}

/// One connector, tagged by `kind`. Unknown keys are errors, so a credential pasted in
/// (`password = "..."`) is refused rather than silently ignored.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
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
        /// Uploads folder when it isn't at `wp_root/wp-content/uploads` (e.g. copied apart).
        #[serde(default)]
        uploads_dir: Option<PathBuf>,
    },
    UmbracoJson {
        /// Umbraco JSON export files; relative paths resolve against the project folder.
        paths: Vec<PathBuf>,
    },
    SanityApi {
        project_id: String,
        dataset: String,
        /// Name of the env var holding the read token; the token itself never goes here.
        #[serde(default = "default_sanity_token_env")]
        token_env: String,
        #[serde(default = "default_sanity_api_version")]
        api_version: String,
        #[serde(default)]
        assets: SanityAssets,
    },
    SanityExport {
        /// `sanity dataset export` archive or folder; relative paths resolve against the
        /// project folder.
        path: PathBuf,
        #[serde(default)]
        assets: SanityAssets,
    },
}

/// Which Sanity assets are fetched.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SanityAssets {
    /// Only assets some document references.
    #[default]
    Referenced,
    All,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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

fn default_sanity_token_env() -> String {
    "PORTER_SANITY_TOKEN".into()
}

fn default_sanity_api_version() -> String {
    "v2021-06-07".into()
}

fn default_table_prefix() -> String {
    "wp_".into()
}

fn default_code() -> u16 {
    301
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct VerifyConfig {
    pub site_url: Option<String>,
}

/// An environment variable NAME (`[A-Za-z_][A-Za-z0-9_]*`). A pasted secret is not one.
pub fn is_env_var_name(s: &str) -> bool {
    let mut c = s.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
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
        // Never echoed: a value that is not a name may be the key itself.
        let env = &self.destination.api_key_env;
        if !env.trim().is_empty() && !is_env_var_name(env) {
            errs.push(
                "destination.api_key_env must be the NAME of an env var (letters, digits, underscore), not a key"
                    .into(),
            );
        }
        // Credentials never go in porter.toml, so a URL carrying `user:pass@` is refused
        // (and not echoed back, since that would print the password).
        let mut url_ok = |key: &str, v: &str| {
            if v.trim().is_empty() {
                return;
            }
            match ::url::Url::parse(v) {
                Err(_) => errs.push(format!("{key} is not a valid URL: {v}")),
                Ok(u) if !u.username().is_empty() || u.password().is_some() => errs.push(format!(
                    "{key} must not contain credentials (user:pass@); use the env vars or keyring instead"
                )),
                Ok(_) => {}
            }
        };
        url_ok("destination.url", &self.destination.url);
        url_ok(
            "verify.site_url",
            self.verify.site_url.as_deref().unwrap_or(""),
        );
        for (i, c) in self.connectors.iter().enumerate() {
            match c {
                ConnectorConfig::Crawl { url, .. } | ConnectorConfig::WpRest { url, .. } => {
                    url_ok(&format!("connectors[{i}].url"), url)
                }
                ConnectorConfig::WpDb {
                    site_url: Some(u), ..
                } => url_ok(&format!("connectors[{i}].site_url"), u),
                _ => {}
            }
        }
        for (i, c) in self.connectors.iter().enumerate() {
            match c {
                ConnectorConfig::SanityApi {
                    project_id,
                    dataset,
                    token_env,
                    ..
                } => {
                    if project_id.trim().is_empty() {
                        errs.push(format!("connectors[{i}].project_id is missing"));
                    }
                    if dataset.trim().is_empty() {
                        errs.push(format!("connectors[{i}].dataset is missing"));
                    }
                    if !is_env_var_name(token_env) {
                        // Never echoed: a value that is not a name may be the token itself.
                        errs.push(format!(
                            "connectors[{i}].token_env must be the NAME of an env var (letters, digits, underscore), not a token"
                        ));
                    }
                }
                ConnectorConfig::SanityExport { path, .. } if path.as_os_str().is_empty() => {
                    errs.push(format!("connectors[{i}].path is missing"));
                }
                _ => {}
            }
        }
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
        errs.extend(self.mapping.validate());
        errs
    }
}

#[cfg(test)]
mod env_name_tests {
    use super::*;

    #[test]
    fn pasted_secret_is_not_an_env_var_name_and_is_not_echoed() {
        for ok in ["K", "_x", "PORTER_PAYLOAD_API_KEY", "a1"] {
            assert!(is_env_var_name(ok), "{ok}");
        }
        for bad in ["", "1A", "sk-live-abc123", "a b", "a=b", "k\u{e9}y"] {
            assert!(!is_env_var_name(bad), "{bad}");
        }
        let mut c = ProjectConfig::default();
        c.destination.api_key_env = "sk-live-abc123".into();
        let errs = c.validate();
        assert!(
            errs.iter()
                .any(|e| e.starts_with("destination.api_key_env")),
            "{errs:?}"
        );
        assert!(!errs.iter().any(|e| e.contains("sk-live")), "{errs:?}");
    }
}

#[cfg(test)]
mod typo_tests {
    use super::*;

    #[test]
    fn hub_section_loads_and_refuses_a_token() {
        let c = toml::from_str::<ProjectConfig>(
            "[hub]\nurl = \"https://hub.example.com\"\nproject = \"acme\"\n",
        )
        .unwrap();
        assert_eq!(c.hub.unwrap().project, "acme");
        let e = toml::from_str::<ProjectConfig>(
            "[hub]\nurl = \"https://h\"\nproject = \"a\"\ntoken = \"x\"\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("token"), "{e}");
    }

    #[test]
    fn typo_in_destination_errors() {
        let e = toml::from_str::<ProjectConfig>(
            "[destination]\nadapter = \"p\"\napi_key = \"secret\"\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("api_key"), "{e}");
    }
}

#[cfg(test)]
mod connector_key_tests {
    use super::*;

    #[test]
    fn unknown_connector_key_errors() {
        let e = toml::from_str::<ProjectConfig>(
            "[[connectors]]\nkind = \"wp-rest\"\nurl = \"https://example.com\"\npassword = \"hunter2\"\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("password"), "{e}");
    }

    #[test]
    fn url_userinfo_is_rejected() {
        let mut c: ProjectConfig = toml::from_str(
            "[project]\nname = \"x\"\n[source]\nadapter = \"wordpress\"\n[destination]\nadapter = \"payload\"\nurl = \"https://u:p@cms.example.com\"\napi_key_env = \"K\"\n[verify]\nsite_url = \"https://u@site.example.com\"\n[[connectors]]\nkind = \"crawl\"\nurl = \"https://u:p@example.com\"\n[[connectors]]\nkind = \"wp-rest\"\nurl = \"https://example.com\"\n",
        )
        .unwrap();
        let errs = c.validate();
        for k in ["destination.url", "verify.site_url", "connectors[0].url"] {
            assert!(
                errs.iter()
                    .any(|e| e.starts_with(k) && e.contains("credentials")),
                "{k}: {errs:?}"
            );
        }
        assert!(!errs.iter().any(|e| e.contains("p@")), "{errs:?}");
        c.destination.url = "https://cms.example.com".into();
        c.verify.site_url = None;
        c.connectors.remove(0);
        assert!(c.validate().is_empty(), "{:?}", c.validate());
    }
}

#[cfg(test)]
mod sanity_tests {
    use super::*;

    #[test]
    fn sanity_api_parses_with_defaults() {
        let c: ProjectConfig = toml::from_str(
            "[[connectors]]\nkind = \"sanity-api\"\nproject_id = \"abc\"\ndataset = \"production\"\n",
        )
        .unwrap();
        match &c.connectors[0] {
            ConnectorConfig::SanityApi {
                token_env,
                api_version,
                assets,
                ..
            } => {
                assert_eq!(token_env, "PORTER_SANITY_TOKEN");
                assert_eq!(api_version, "v2021-06-07");
                assert_eq!(*assets, SanityAssets::Referenced);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sanity_empty_ids_and_path_are_rejected() {
        let c: ProjectConfig = toml::from_str(
            "[[connectors]]\nkind = \"sanity-api\"\nproject_id = \" \"\ndataset = \"\"\n\n[[connectors]]\nkind = \"sanity-export\"\npath = \"\"\n",
        )
        .unwrap();
        let errs = c.validate();
        for k in [
            "connectors[0].project_id",
            "connectors[0].dataset",
            "connectors[1].path",
        ] {
            assert!(
                errs.iter()
                    .any(|e| e.starts_with(k) && e.contains("missing")),
                "{k}: {errs:?}"
            );
        }
    }

    #[test]
    fn sanity_token_env_must_be_env_var_name() {
        let c: ProjectConfig = toml::from_str(
            "[[connectors]]\nkind = \"sanity-api\"\nproject_id = \"abc\"\ndataset = \"p\"\ntoken_env = \"not a name\"\n",
        )
        .unwrap();
        let errs = c.validate();
        assert!(errs.iter().any(|e| e.contains("token_env")), "{errs:?}");
    }

    #[test]
    fn sanity_unknown_key_rejected() {
        let e = toml::from_str::<ProjectConfig>(
            "[[connectors]]\nkind = \"sanity-export\"\npath = \"x.tar.gz\"\ntoken = \"s\"\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("token"), "{e}");
    }
}
