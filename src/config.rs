use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use anyhow::{Result, anyhow};
use colored::Colorize;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CollectionConfig {
    pub name: String,
    pub source_data: String,
    pub collection_path: Option<String>,
    pub locale: Option<String>,
    pub related_collections: Option<Vec<String>>,
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
pub fn create_config_interactively() -> Result<PorterConfig> {
    use crate::util::interact;
    
    println!("{}", "🚀 Porter Configuration Setup".cyan().bold());
    println!("{}", "Let's create a new configuration file...".white());
    println!();

    let mut config = PorterConfig::default();

    // Source selection
    let source_options = vec!["umbraco", "wordpress (not implemented)", "drupal (not implemented)", "custom"];
    let source_index = interact::select_with_arrows(
        "Select source system:",
        &source_options,
        "source"
    )?;
    config.source = source_options[source_index].to_string();
    println!("{}", format!("✓ Selected source: {}", config.source).green());

    // Target selection
    let target_options = vec!["payload", "strapi (not implemented)", "contentful (not implemented)", "custom"];
    let target_index = interact::select_with_arrows(
        "Select target system:",
        &target_options,
        "target"
    )?;
    config.target = target_options[target_index].to_string();
    println!("{}", format!("✓ Selected target: {}", config.target).green());

    // Output directory
    let output = interact::prompt_with_default("Enter output directory:", "./seed")?;
    config.output = output;
    println!("{}", format!("✓ Output: {}", config.output).green());

    // Add collections
    println!();
    println!("{}", "📁 Collection Configuration".cyan().bold());
    println!("{}", "Now let's configure your collections...".white());
    
    let mut collections = Vec::new();
    let mut add_more = true;
    
    while add_more {
        println!();
        println!("{}", format!("=== Collection {} ===", collections.len() + 1).yellow().bold());
        
        // Collection name
        let collection_name = interact::prompt("Enter collection name (e.g., hotels, pages):")?;
        if collection_name.is_empty() {
            return Err(anyhow!("Collection name is required"));
        }
        println!("{}", format!("✓ Collection name: {}", collection_name).green());
        
        // Source data path
        let source_data = interact::prompt(&format!("Enter source data file path for '{}' (e.g., ./test-data/{}-umbraco.json):", collection_name, collection_name))?;
        if source_data.is_empty() {
            return Err(anyhow!("Source data path is required"));
        }
        println!("{}", format!("✓ Source data: {}", source_data).green());
        
        // Collection path for Payload
        let mut collection_path = None;
        if config.target == "payload" {
            let path = interact::prompt(&format!("Enter collection schema path for '{}' (e.g., ./test-data/{}.ts):", collection_name, collection_name))?;
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
