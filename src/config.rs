use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use anyhow::{Result, anyhow};
use colored::Colorize;
// use crate::adapters::{AuthConfig, AuthType};
use crate::util::auth::build_auth_config_from_metadata;
use crate::sources::wordpress::WordPressApiConnector;
use crate::sources::wordpress::config::WordPressConfig;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CollectionConfig {
    pub name: String,
    pub source_data: String,
    pub collection_path: Option<String>,
    pub locale: Option<String>,
    pub related_collections: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PayloadSection {
    pub config_entry: String,
    pub module_system: String, // "esm" | "cjs"
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TypescriptSection {
    pub tsconfig_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_aliases: Option<Vec<String>>, // e.g., ["@"]
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct IoSection {
    pub graphs_dir: String,
    pub templates_dir: String,
    pub seeds_dir: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct MediaSection {
    pub policy: String, // "ignore" (MVP)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PorterConfig {
    pub source: String,
    pub target: String,
    pub output: String,
    pub collections: Vec<CollectionConfig>,
    pub plugin_dir: Option<String>,
    pub interactive: bool,
    pub verbose: bool,
    pub dry_run: bool,
    pub debug: bool,
    pub fixtures: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<PayloadSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typescript: Option<TypescriptSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io: Option<IoSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaSection>,
}

impl Default for PorterConfig {
    fn default() -> Self {
        Self {
            source: String::new(),
            target: String::new(),
            output: "./seed".to_string(),
            collections: Vec::new(),
            plugin_dir: None,
            interactive: true,
            verbose: true,
            dry_run: false,
            debug: false,
            fixtures: false,
            metadata: None,
            payload: None,
            typescript: None,
            io: None,
            media: None,
        }
    }
}

impl PorterConfig {
    /// Loads configuration from a file
    pub fn load_from_file(path: &str) -> Result<Self> {
        if !Path::new(path).exists() {
            return Err(anyhow!("Configuration file not found: {}", path));
        }

        let content = std::fs::read_to_string(path)?;
        
        if path.ends_with(".toml") {
            let config: PorterConfig = toml::from_str(&content)?;
            Ok(config)
        } else if path.ends_with(".json") {
            let config: PorterConfig = serde_json::from_str(&content)?;
            Ok(config)
        } else {
            Err(anyhow!("Unsupported configuration file format. Use .toml or .json"))
        }
    }

    /// Saves configuration to a file
    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let content = if path.ends_with(".toml") {
            toml::to_string_pretty(self)?
        } else if path.ends_with(".json") {
            serde_json::to_string_pretty(self)?
        } else {
            return Err(anyhow!("Unsupported configuration file format. Use .toml or .json"));
        };

        std::fs::write(path, content)?;
        println!("{}", format!("✓ Configuration saved to {}", path).green());
        Ok(())
    }

    /// Creates a new configuration file with default values
    pub fn init_config_file(path: &str) -> Result<Self> {
        let config = PorterConfig::default();
        config.save_to_file(path)?;
        Ok(config)
    }

    /// Merges this configuration with another, preferring non-empty values from other
    pub fn merge(&mut self, other: &PorterConfig) {
        if !other.source.is_empty() {
            self.source = other.source.clone();
        }
        if !other.target.is_empty() {
            self.target = other.target.clone();
        }
        if !other.output.is_empty() {
            self.output = other.output.clone();
        }
        if !other.collections.is_empty() {
            self.collections = other.collections.clone();
        }
        if other.plugin_dir.is_some() {
            self.plugin_dir = other.plugin_dir.clone();
        }
        self.interactive = other.interactive;
        self.verbose = other.verbose;
        self.dry_run = other.dry_run;
        self.debug = other.debug;
        self.fixtures = other.fixtures;
        if other.metadata.is_some() { self.metadata = other.metadata.clone(); }
        if other.payload.is_some() { self.payload = other.payload.clone(); }
        if other.typescript.is_some() { self.typescript = other.typescript.clone(); }
        if other.io.is_some() { self.io = other.io.clone(); }
        if other.media.is_some() { self.media = other.media.clone(); }
    }

    /// Validates that all required fields are present
    pub fn validate(&self) -> Result<()> {
        if self.source.is_empty() {
            return Err(anyhow!("Source is required"));
        }
        if self.target.is_empty() {
            return Err(anyhow!("Target is required"));
        }
        if self.collections.is_empty() {
            return Err(anyhow!("At least one collection is required"));
        }
        
        // Validate each collection
        for collection in &self.collections {
            if collection.name.is_empty() {
                return Err(anyhow!("Collection name is required"));
            }
            if collection.source_data.is_empty() {
                return Err(anyhow!("Source data is required for collection '{}'", collection.name));
            }
            if self.target == "payload" && collection.collection_path.is_none() {
                return Err(anyhow!("Collection path is required for Payload CMS collection '{}'", collection.name));
            }
        }

        // Additional required keys for target=payload
        if self.target == "payload" {
            let payload_cfg = self.payload.as_ref().ok_or_else(|| anyhow!("[payload] section is required when target is 'payload'"))?;
            if payload_cfg.config_entry.trim().is_empty() {
                return Err(anyhow!("payload.config_entry is required"));
            }
            if payload_cfg.module_system.trim().is_empty() {
                return Err(anyhow!("payload.module_system is required (esm or cjs)"));
            }

            let ts_cfg = self.typescript.as_ref().ok_or_else(|| anyhow!("[typescript] section is required when target is 'payload'"))?;
            if ts_cfg.tsconfig_path.trim().is_empty() {
                return Err(anyhow!("typescript.tsconfig_path is required"));
            }

            let io_cfg = self.io.as_ref().ok_or_else(|| anyhow!("[io] section is required when target is 'payload'"))?;
            if io_cfg.seeds_dir.trim().is_empty() {
                return Err(anyhow!("io.seeds_dir is required"));
            }
        }
        Ok(())
    }

    /// Displays the current configuration
    pub fn display(&self) {
        println!("{}", "📋 Current Configuration".cyan().bold());
        println!("{}", "─".repeat(50));
        println!("Source: {}", self.source.blue());
        println!("Target: {}", self.target.blue());
        println!("Output: {}", self.output.blue());
        println!("Collections: {}", self.collections.len().to_string().blue());
        
        for (i, collection) in self.collections.iter().enumerate() {
            println!();
            println!("  {} Collection {}: {}", "📁".cyan(), (i + 1).to_string().yellow(), collection.name.blue());
            println!("    Source Data: {}", collection.source_data.blue());
            if let Some(path) = &collection.collection_path {
                println!("    Collection Path: {}", path.blue());
            }
            if let Some(locale) = &collection.locale {
                println!("    Locale: {}", locale.blue());
            }
            if let Some(related) = &collection.related_collections {
                println!("    Related Collections: {}", related.join(", ").blue());
            }
        }
        
        if let Some(payload) = &self.payload {
            println!();
            println!("{}", "[payload]".cyan().bold());
            println!("  config_entry: {}", payload.config_entry.blue());
            println!("  module_system: {}", payload.module_system.blue());
        }
        if let Some(ts) = &self.typescript {
            println!();
            println!("{}", "[typescript]".cyan().bold());
            println!("  tsconfig_path: {}", ts.tsconfig_path.blue());
            if let Some(aliases) = &ts.path_aliases {
                println!("  path_aliases: {}", aliases.join(", ").blue());
            }
        }
        if let Some(io) = &self.io {
            println!();
            println!("{}", "[io]".cyan().bold());
            println!("  graphs_dir: {}", io.graphs_dir.blue());
            println!("  templates_dir: {}", io.templates_dir.blue());
            println!("  seeds_dir: {}", io.seeds_dir.blue());
        }
        if let Some(media) = &self.media {
            println!();
            println!("{}", "[media]".cyan().bold());
            println!("  policy: {}", media.policy.blue());
        }

        println!();
        println!("Interactive: {}", if self.interactive { "✓".green() } else { "✗".red() });
        println!("Verbose: {}", if self.verbose { "✓".green() } else { "✗".red() });
        println!("Dry Run: {}", if self.dry_run { "✓".yellow() } else { "✗".red() });
        println!("{}", "─".repeat(50));
    }
}

/// Finds and loads configuration from common locations
pub fn find_and_load_config() -> Result<Option<PorterConfig>> {
    let config_paths = [
        "porter.config.toml",
        "porter.config.json",
        ".porter.toml",
        ".porter.json",
    ];

    for path in &config_paths {
        if Path::new(path).exists() {
            println!("{}", format!("📁 Loading configuration from {}", path).cyan());
            return Ok(Some(PorterConfig::load_from_file(path)?));
        }
    }

    Ok(None)
}

/// Creates a new configuration file interactively
pub async fn create_config_interactively() -> Result<PorterConfig> {
    use crate::util::interact;
    
    println!("{}", "🚀 Porter Configuration Setup".cyan().bold());
    println!("{}", "Let's create a new configuration file...".white());
    println!();

    let mut config = PorterConfig::default();

    // Source selection
    let source_options = vec!["umbraco", "wordpress", "drupal (not implemented)", "custom"];
    let source_index = interact::select_with_arrows(
        "Select source system:",
        &source_options,
        "source"
    )?;
    config.source = source_options[source_index].to_string();
    println!("{}", format!("✓ Selected source: {}", config.source).green());

    // WordPress-specific configuration
    if config.source == "wordpress" {
        println!();
        println!("{}", "🔧 WordPress Configuration".cyan().bold());
        
        // Source input type selection
        let input_type_options = vec!["api", "file (wxr export)"];
        let input_type_index = interact::select_with_arrows(
            "Select WordPress source input type:",
            &input_type_options,
            "input_type"
        )?;
        let input_type = input_type_options[input_type_index].to_string();
        println!("{}", format!("✓ Selected input type: {}", input_type).green());
        
        // Store WordPress configuration in metadata
        if config.metadata.is_none() {
            config.metadata = Some(HashMap::new());
        }
        config.metadata.as_mut().unwrap().insert("wordpress_input_type".to_string(), input_type.clone());
        
        // If API is selected, ask for API URL
        if input_type.as_str() == "api" {
            let api_url = interact::prompt("Enter WordPress API URL (e.g., https://example.com/wp-json/wp/v2):")?;
            if api_url.is_empty() {
                return Err(anyhow!("API URL is required for WordPress API source"));
            }
            config.metadata.as_mut().unwrap().insert("wordpress_api_url".to_string(), api_url);
            println!("{}", format!("✓ API URL: {}", config.metadata.as_ref().unwrap().get("wordpress_api_url").unwrap()).green());

            // Optional authentication configuration
            println!("\n{}", "🔐 WordPress API Authentication (optional)".cyan().bold());
            let auth_options = vec!["none", "bearer", "basic", "apikey"];
            let auth_choice = interact::select_with_arrows(
                "Select authentication type:",
                &auth_options,
                "auth_type"
            )?;
            let auth_type = auth_options[auth_choice].to_string();
            config.metadata.as_mut().unwrap().insert("wordpress_auth_type".to_string(), auth_type.clone());

            match auth_type.as_str() {
                "bearer" => {
                    let token = interact::prompt("Enter Bearer token (will be stored in config):")?;
                    if !token.is_empty() {
                        config.metadata.as_mut().unwrap().insert("wordpress_bearer_token".to_string(), token);
                        println!("{}", "✓ Bearer token saved".green());
                    }
                }
                "basic" => {
                    let user = interact::prompt("Enter Basic auth username:")?;
                    let pass = interact::prompt("Enter Basic auth password:")?;
                    if !user.is_empty() && !pass.is_empty() {
                        config.metadata.as_mut().unwrap().insert("wordpress_basic_username".to_string(), user);
                        config.metadata.as_mut().unwrap().insert("wordpress_basic_password".to_string(), pass);
                        println!("{}", "✓ Basic credentials saved".green());
                    }
                }
                "apikey" => {
                    let header = interact::prompt_with_default("Enter API key header name:", "X-API-Key")?;
                    let key = interact::prompt("Enter API key value:")?;
                    if !key.is_empty() {
                        config.metadata.as_mut().unwrap().insert("wordpress_api_key_header".to_string(), header);
                        config.metadata.as_mut().unwrap().insert("wordpress_api_key".to_string(), key);
                        println!("{}", "✓ API key saved".green());
                    }
                }
                _ => {
                    // none
                }
            }
            // Endpoint discovery from API root
            let mut discovered_endpoints: Vec<String> = Vec::new();
            if let Some(api_url) = config.metadata.as_ref().and_then(|m| m.get("wordpress_api_url")).cloned() {
                // Build auth_config from metadata
                let auth_config = config.metadata.as_ref().and_then(|m| build_auth_config_from_metadata(m));

                // Discover endpoints asynchronously (avoid nested runtimes)
                let connector = WordPressApiConnector::new(api_url, WordPressConfig::default(), auth_config);
                if let Ok(conn) = connector {
                    match conn.get_available_content_types().await {
                        Ok(mut eps) => {
                            eps.sort(); eps.dedup();
                            discovered_endpoints = eps;
                            if !discovered_endpoints.is_empty() {
                                println!("{} {:?}", "✓ Discovered endpoints:".green(), discovered_endpoints);
                            }
                        }
                        Err(e) => println!("{} {}", "⚠ Failed to discover endpoints:".yellow(), e),
                    }
                }
            }

            // Persist discovered endpoints temporarily in metadata for use in collection wizard
            if !discovered_endpoints.is_empty() {
                // Store as comma-separated for simple handoff; not part of long-term schema
                config.metadata.as_mut().unwrap().insert(
                    "_wp_discovered_endpoints".to_string(),
                    discovered_endpoints.join(",")
                );
            }
        }
    }

    // Target selection
    let target_options = vec!["payload", "strapi (not implemented)", "contentful (not implemented)", "custom"];
    let target_index = interact::select_with_arrows(
        "Select target system:",
        &target_options,
        "target"
    )?;
    config.target = target_options[target_index].to_string();
    println!("{}", format!("✓ Selected target: {}", config.target).green());

    // Payload-specific config (collect now for future commands)
    if config.target == "payload" {
        println!();
        println!("{}", "🔧 Payload Configuration".cyan().bold());

        let default_payload_entry = "./src/payload.config.ts";
        let payload_entry = interact::prompt_with_default(
            "Enter Payload config entry path (e.g., ./src/payload.config.ts):",
            default_payload_entry,
        )?;
        let module_system_options = vec!["esm", "cjs"];
        let module_idx = interact::select_with_arrows(
            "Select module system:",
            &module_system_options,
            "module_system",
        )?;
        let module_system = module_system_options[module_idx].to_string();
        config.payload = Some(PayloadSection {
            config_entry: payload_entry,
            module_system,
        });

        println!();
        println!("{}", "🧩 TypeScript Configuration".cyan().bold());
        let tsconfig_path = interact::prompt_with_default(
            "Enter tsconfig path:",
            "./tsconfig.json",
        )?;
        let use_aliases = interact::confirm_with_default("Use path aliases (e.g., @)?", true)?;
        let path_aliases = if use_aliases {
            let aliases_csv = interact::prompt_with_default("Enter alias prefixes (comma-separated)", "@")?;
            let list = aliases_csv
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            if list.is_empty() { None } else { Some(list) }
        } else { None };
        config.typescript = Some(TypescriptSection { tsconfig_path, path_aliases });

        println!();
        println!("{}", "📦 Output Directories".cyan().bold());
    }

    // Output directory (legacy top-level; keep for compatibility)
    let default_output = if config.source == "wordpress" {
        "./test-data/ya/seed"
    } else {
        "./seed"
    };
    let output = interact::prompt_with_default("Enter output directory:", default_output)?;
    config.output = output;
    println!("{}", format!("✓ Output: {}", config.output).green());

    // IO directories (graphs/templates/seeds)
    let default_graphs = "./mapping/graphs";
    let default_templates = "./mapping/templates";
    let default_seeds = if !config.output.is_empty() { config.output.clone() } else { "./seeds".to_string() };
    let graphs_dir = interact::prompt_with_default("Enter graphs directory:", default_graphs)?;
    let templates_dir = interact::prompt_with_default("Enter templates directory:", default_templates)?;
    let seeds_dir = interact::prompt_with_default("Enter seeds directory:", &default_seeds)?;
    config.io = Some(IoSection { graphs_dir, templates_dir, seeds_dir: seeds_dir.clone() });

    // Media policy (MVP: ignore)
    config.media = Some(MediaSection { policy: "ignore".to_string() });

    // Add collections
    println!();
    println!("{}", "📁 Collection Configuration".cyan().bold());
    println!("{}", "Now let's configure your collections...".white());
    
    let mut collections = Vec::new();
    let mut add_more = true;

    // If we have discovered endpoints, walk the wizard over them first
    let discovered: Vec<String> = config.metadata.as_ref()
        .and_then(|m| m.get("_wp_discovered_endpoints").cloned())
        .map(|s| s.split(',').map(|v| v.to_string()).collect())
        .unwrap_or_else(|| Vec::new());

    if !discovered.is_empty() && config.source == "wordpress" &&
        config.metadata.as_ref().and_then(|m| m.get("wordpress_input_type")).map(|s| s.as_str()) == Some("api") {
        println!("{}", "🔎 Found endpoints from API – let’s align them to collections".cyan().bold());
        let total = discovered.len();
        for (i, endpoint) in discovered.clone().into_iter().enumerate() {
            println!("\n{}", format!("=== Endpoint {}/{}: {} ===", i + 1, total, endpoint).yellow().bold());
            // Suggest collection name from endpoint
            let suggested = endpoint.clone();
            let collection_name = interact::prompt_with_default("What would you like to name this collection?", &suggested)?;
            println!("{}", format!("✓ Collection name: {}", collection_name).green());

            // Confirm endpoint (single selection)
            println!("{}", format!("Using source endpoint: {}", endpoint).green());

            // Target schema path
            let default_path = format!("./src/collections/{}.ts", collection_name);
            let path = interact::prompt_with_default(&format!("Enter collection schema path for '{}':", collection_name), &default_path)?;
            if path.is_empty() {
                return Err(anyhow!("Collection schema path is required for Payload CMS"));
            }
            let collection_path = Some(path);

            let collection_config = CollectionConfig {
                name: collection_name,
                source_data: endpoint,
                collection_path,
                locale: None,
                related_collections: None,
            };
            collections.push(collection_config);
        }
        // Clean up temporary metadata key
        if let Some(meta) = &mut config.metadata {
            meta.remove("_wp_discovered_endpoints");
        }
        // Ask if they want to add additional collections manually
        add_more = interact::confirm_with_default("Add another collection (manual)?", false)?;
    }
    
    while add_more {
        println!();
        println!("{}", format!("=== Collection {} ===", collections.len() + 1).yellow().bold());
        
        // Collection name
        let collection_name = interact::prompt("Enter collection name (e.g., hotels, pages):")?;
        if collection_name.is_empty() {
            return Err(anyhow!("Collection name is required"));
        }
        println!("{}", format!("✓ Collection name: {}", collection_name).green());
        
        // Source data path or API endpoint
        let source_data = if config.source == "wordpress" && 
            config.metadata.as_ref().and_then(|m| m.get("wordpress_input_type")).map(|s| s.as_str()) == Some("api") {
            // For WordPress API, ask for endpoint
            let endpoint = interact::prompt(&format!("Enter WordPress API endpoint for '{}' (e.g., posts, pages, media):", collection_name))?;
            if endpoint.is_empty() {
                return Err(anyhow!("API endpoint is required for WordPress API source"));
            }
            println!("{}", format!("✓ API endpoint: {}", endpoint).green());
            endpoint
        } else {
            // For file-based sources
            let file_path = interact::prompt(&format!("Enter source data file path for '{}' (e.g., ./test-data/{}-umbraco.json):", collection_name, collection_name))?;
            if file_path.is_empty() {
                return Err(anyhow!("Source data path is required"));
            }
            println!("{}", format!("✓ Source data: {}", file_path).green());
            file_path
        };
        
        // Collection path for Payload
        let mut collection_path = None;
        if config.target == "payload" {
            let default_path = if config.source == "wordpress" {
                format!("./test-data/ya/collections/{}.ts", collection_name)
            } else {
                format!("./test-data/{}.ts", collection_name)
            };
            let path = interact::prompt_with_default(&format!("Enter collection schema path for '{}':", collection_name), &default_path)?;
            if path.is_empty() {
                return Err(anyhow!("Collection schema path is required for Payload CMS"));
            }
            collection_path = Some(path);
            println!("{}", format!("✓ Collection schema: {}", collection_path.as_ref().unwrap()).green());
        }
        
        // Locale (optional)
        let locale = if interact::confirm("Does this collection have a specific locale?")? {
            let locale_value = interact::prompt("Enter locale (e.g., en, fr, es):")?;
            if !locale_value.is_empty() {
                Some(locale_value)
            } else {
                None
            }
        } else {
            None
        };
        
        // Related collections (optional)
        let related_collections = if interact::confirm("Does this collection have related collections?")? {
            let related_input = interact::prompt("Enter related collection names (comma-separated):")?;
            if !related_input.is_empty() {
                Some(related_input.split(',').map(|s| s.trim().to_string()).collect())
            } else {
                None
            }
        } else {
            None
        };
        
        // Create collection config
        let collection_config = CollectionConfig {
            name: collection_name,
            source_data,
            collection_path,
            locale,
            related_collections,
        };
        
        collections.push(collection_config);
        
        // Ask if user wants to add more collections
        add_more = interact::confirm_with_default("Add another collection?", false)?;
    }
    
    config.collections = collections;

    // Interactive mode
    config.interactive = interact::confirm_with_default("Enable interactive field mapping?", true)?;
    println!("{}", if config.interactive { "✓ Interactive mode enabled".green() } else { "✗ Interactive mode disabled".red() });

    // Verbose logging
    config.verbose = interact::confirm_with_default("Enable verbose logging?", true)?;
    println!("{}", if config.verbose { "✓ Verbose logging enabled".green() } else { "✗ Verbose logging disabled".red() });

    // Save configuration
    let save_config = interact::confirm_with_default("Save this configuration to porter.config.toml?", true)?;
    if save_config {
        config.save_to_file("porter.config.toml")?;
    }

    println!();
    println!("{}", "🎯 Configuration Complete!".cyan().bold());
    config.display();

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    #[test]
    fn test_porter_config_default() {
        let config = PorterConfig::default();
        assert_eq!(config.source, "");
        assert_eq!(config.target, "");
        assert_eq!(config.output, "./seed");
        assert_eq!(config.interactive, true);
        assert_eq!(config.verbose, true);
        assert_eq!(config.dry_run, false);
        assert_eq!(config.debug, false);
        assert_eq!(config.fixtures, false);
        assert!(config.collections.is_empty());
    }

    #[test]
    fn test_collection_config_default() {
        let config = CollectionConfig::default();
        assert_eq!(config.name, "");
        assert_eq!(config.source_data, "");
        assert_eq!(config.collection_path, None);
        assert!(config.related_collections.is_none());
    }

    #[test]
    fn test_porter_config_merge() {
        let mut base = PorterConfig::default();
        base.source = "umbraco".to_string();
        base.target = "payload".to_string();
        base.interactive = true;

        let override_config = PorterConfig {
            source: "wordpress".to_string(),
            target: "strapi".to_string(),
            interactive: false,
            verbose: true,
            ..Default::default()
        };

        base.merge(&override_config);

        assert_eq!(base.source, "wordpress");
        assert_eq!(base.target, "strapi");
        assert_eq!(base.interactive, false);
        assert_eq!(base.verbose, true);
    }

    #[test]
    fn test_porter_config_validate_success() {
        let config = PorterConfig {
            source: "umbraco".to_string(),
            target: "payload".to_string(),
            collections: vec![
                CollectionConfig {
                    name: "hotels".to_string(),
                    source_data: "./test-data/hotels.json".to_string(),
                    collection_path: Some("./test-data/hotels.ts".to_string()),
                    locale: None,
                    related_collections: None,
                }
            ],
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_porter_config_validate_missing_source() {
        let config = PorterConfig {
            target: "payload".to_string(),
            collections: vec![
                CollectionConfig {
                    name: "hotels".to_string(),
                    source_data: "./test-data/hotels.json".to_string(),
                    collection_path: Some("./test-data/hotels.ts".to_string()),
                    locale: None,
                    related_collections: None,
                }
            ],
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Source is required"));
    }

    #[test]
    fn test_porter_config_validate_missing_target() {
        let config = PorterConfig {
            source: "umbraco".to_string(),
            collections: vec![
                CollectionConfig {
                    name: "hotels".to_string(),
                    source_data: "./test-data/hotels.json".to_string(),
                    collection_path: Some("./test-data/hotels.ts".to_string()),
                    locale: None,
                    related_collections: None,
                }
            ],
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Target is required"));
    }

    #[test]
    fn test_porter_config_validate_empty_collections() {
        let config = PorterConfig {
            source: "umbraco".to_string(),
            target: "payload".to_string(),
            collections: vec![],
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("At least one collection is required"));
    }

    #[test]
    fn test_porter_config_validate_invalid_collection() {
        let config = PorterConfig {
            source: "umbraco".to_string(),
            target: "payload".to_string(),
            collections: vec![
                CollectionConfig {
                    name: "".to_string(), // Invalid: empty name
                    source_data: "./test-data/hotels.json".to_string(),
                    collection_path: Some("./test-data/hotels.ts".to_string()),
                    locale: None,
                    related_collections: None,
                }
            ],
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Collection name is required"));
    }

    #[test]
    fn test_save_and_load_config() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let config_path = temp_file.path().to_str().unwrap().to_string() + ".toml";

        let original_config = PorterConfig {
            source: "umbraco".to_string(),
            target: "payload".to_string(),
            interactive: true,
            verbose: true,
            collections: vec![
                CollectionConfig {
                    name: "hotels".to_string(),
                    source_data: "./test-data/hotels.json".to_string(),
                    collection_path: Some("./test-data/hotels.ts".to_string()),
                    locale: None,
                    related_collections: Some(vec!["countries".to_string(), "regions".to_string()]),
                }
            ],
            ..Default::default()
        };

        // Save config
        original_config.save_to_file(&config_path)?;

        // Load config
        let loaded_config = PorterConfig::load_from_file(&config_path)?;

        // Verify they match
        assert_eq!(original_config.source, loaded_config.source);
        assert_eq!(original_config.target, loaded_config.target);
        assert_eq!(original_config.interactive, loaded_config.interactive);
        assert_eq!(original_config.verbose, loaded_config.verbose);
        assert_eq!(original_config.collections.len(), loaded_config.collections.len());
        assert_eq!(original_config.collections[0].name, loaded_config.collections[0].name);
        assert_eq!(original_config.collections[0].source_data, loaded_config.collections[0].source_data);
        assert_eq!(original_config.collections[0].collection_path, loaded_config.collections[0].collection_path);
        assert_eq!(original_config.collections[0].related_collections, loaded_config.collections[0].related_collections);

        Ok(())
    }

    #[test]
    fn test_load_nonexistent_config() {
        let result = PorterConfig::load_from_file("nonexistent.toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_and_load_config() -> Result<()> {
        // Create a temporary config file in current directory
        let temp_file = NamedTempFile::new()?.into_temp_path();
        let config_path = temp_file.to_str().unwrap().to_string() + ".toml";
        
        let config = PorterConfig {
            source: "umbraco".to_string(),
            target: "payload".to_string(),
            ..Default::default()
        };
        config.save_to_file(&config_path)?;

        // Test finding the config
        let found_config = PorterConfig::load_from_file(&config_path)?;
        assert_eq!(found_config.source, "umbraco");
        assert_eq!(found_config.target, "payload");

        // Clean up
        fs::remove_file(config_path)?;
        Ok(())
    }
}
