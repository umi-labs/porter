mod cli;

use crate::cli::{Cli, Commands};
use anyhow::{Result, anyhow};
use clap::Parser;
use colored::Colorize;
use log::{debug, info, warn};
 use porter::dlog;
use porter::util::debug as dbgutil;
use porter::adapter::TargetOptions;
use porter::batch::BatchConfig;
use porter::config::{
    CollectionConfig, PorterConfig, create_config_interactively, find_and_load_config,
};
use porter::mapping;
use porter::mapping::initialize_mappings_base;
use porter::mapping::generator::MappingGenerator;
use porter::performance::{OptimizedBatchProcessor, PerformanceConfig, PerformanceProcessor};
use porter::plugin::{PluginManager, get_default_plugin_dir};
use porter::sources::umbraco::UmbracoSource;
use porter::targets::payload::PayloadTarget;
use porter::adapters::{SourceAdapter, ApiSourceAdapter, ConnectionMethod};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize debug early from CLI so subcommands can emit debug before config is loaded
    dbgutil::init(cli.debug);

    // Handle subcommands first
    if let Some(command) = cli.command {
        match command {
            Commands::Init { output } => {
                println!("{}", "🚀 Initialising Porter Configuration".cyan().bold());
                dlog!("Init command invoked; output file will be {}", output);
                let config = create_config_interactively().await?;
                dlog!(
                    "Init finished; saving config with {} collections to {}",
                    config.collections.len(),
                    output
                );
                config.save_to_file(&output)?;
                dlog!("Config saved to {}", output);
                return Ok(());
            }
            Commands::Clean { config: config_file, dir, full } => {
                use std::fs;
                use std::path::PathBuf;
                println!("{}", "🧹 Cleaning migrations".cyan().bold());
                dlog!("Clean command invoked with config={:?} dir={:?} full={}", config_file, dir, full);

                // Load config if provided/available to infer output and migrations dir
                let config_opt = if let Some(config_path) = config_file {
                    dlog!("Loading config from explicit path: {}", config_path);
                    let cfg = PorterConfig::load_from_file(&config_path)?;
                    // If debug was not enabled via CLI, enable if config requests it
                    dbgutil::init(cli.debug || cfg.debug);
                    Some(cfg)
                } else {
                    dlog!("Attempting to find and load config from default locations");
                    let cfg_opt = find_and_load_config()?;
                    if let Some(cfg) = &cfg_opt { dbgutil::init(cli.debug || cfg.debug); }
                    cfg_opt
                };

                // Determine migrations directory
                let migrations_dir = if let Some(p) = dir {
                    dlog!("Using migrations dir provided via --dir: {}", p);
                    PathBuf::from(p)
                } else if let Some(cfg) = &config_opt {
                    // Assume migrations dir is parent of output if named like <migrations>/output
                    let out = PathBuf::from(&cfg.output);
                    let inferred = out.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("./migrations"));
                    dlog!("Inferred migrations dir from config.output ({}): {:?}", cfg.output, inferred);
                    inferred
                } else {
                    dlog!("No config available; defaulting migrations dir to ./migrations");
                    PathBuf::from("./migrations")
                };

                if migrations_dir.exists() {
                    dlog!("Deleting migrations directory {:?}", migrations_dir);
                    println!("Deleting {:?}", migrations_dir);
                    fs::remove_dir_all(&migrations_dir).map_err(|e| anyhow!("Failed to delete {:?}: {}", migrations_dir, e))?;
                } else {
                    println!("{} {:?}", "No migrations directory found at".yellow(), migrations_dir);
                }

                if full {
                    // Delete porter config file if present
                    let candidates = [
                        "porter.config.toml",
                        "porter.config.json",
                        ".porter.toml",
                        ".porter.json",
                    ];
                    let mut removed_any = false;
                    for c in &candidates {
                        if Path::new(c).exists() {
                            dlog!("Removing config file {}", c);
                            println!("Deleting {}", c);
                            fs::remove_file(c).map_err(|e| anyhow!("Failed to delete {}: {}", c, e))?;
                            removed_any = true;
                        }
                    }
                    if !removed_any { println!("{}", "No porter config file found".yellow()); }
                }

                println!("{}", "✓ Clean complete".green());
                return Ok(());
            }
            Commands::Config { file: _ } => {
                if let Some(config) = find_and_load_config()? {
                    config.display();
                } else {
                    println!("{}", "No configuration file found".yellow());
                    println!("Run 'porter init' to create a new configuration");
                }
                return Ok(());
            }
            Commands::Generate { config: config_file, collection } => {
                println!("{}", "🔄 Generating Mappings".cyan().bold());
                dlog!("Generate command invoked with config={:?} collection={:?}", config_file, collection);
                let config = if let Some(config_path) = config_file {
                    dlog!("Loading config from explicit path: {}", config_path);
                    let cfg = PorterConfig::load_from_file(&config_path)?;
                    dbgutil::init(cli.debug || cfg.debug);
                    cfg
                } else if let Some(file_config) = find_and_load_config()? {
                    dlog!("Loaded config from default locations");
                    dbgutil::init(cli.debug || file_config.debug);
                    file_config
                } else {
                    return Err(anyhow!("No configuration file found. Run 'porter init' first."));
                };
                dlog!("Config loaded: collections={} output={}", config.collections.len(), config.output);
                // Create mapping generator and generate mappings
                let generator = MappingGenerator::new(config);
                dlog!("MappingGenerator created; starting generate_mappings");
                generator.generate_mappings(collection.as_deref()).await?;
                dlog!("Generate completed successfully");
                return Ok(());
            }
            Commands::Template { config: config_file, collection } => {
                println!("{}", "🧩 Generating Templates".cyan().bold());
                dlog!("Template command invoked with config={:?} collection={:?}", config_file, collection);
                let config = if let Some(config_path) = config_file {
                    dlog!("Loading config from explicit path: {}", config_path);
                    let cfg = PorterConfig::load_from_file(&config_path)?;
                    dbgutil::init(cli.debug || cfg.debug);
                    cfg
                } else if let Some(file_config) = find_and_load_config()? {
                    dlog!("Loaded config from default locations");
                    dbgutil::init(cli.debug || file_config.debug);
                    file_config
                } else {
                    return Err(anyhow!("No configuration file found. Run 'porter init' first."));
                };
                dlog!("Config loaded: collections={} output={}", config.collections.len(), config.output);
                let generator = MappingGenerator::new(config);
                dlog!("MappingGenerator created; starting generate_templates");
                generator.generate_templates(collection.as_deref())?;
                return Ok(());
            }
            Commands::Migrate { config: config_file, collection: _collection } => {
                println!("{}", "🚀 Starting Migration".cyan().bold());

                let config = if let Some(config_path) = config_file {
                    PorterConfig::load_from_file(&config_path)?
                } else if let Some(file_config) = find_and_load_config()? {
                    file_config
                } else {
                    return Err(anyhow!("No configuration file found. Run 'porter init' first."));
                };

                // Initialize plugin manager
                let mut plugin_manager = PluginManager::new();
                plugin_manager.register_source("umbraco", Box::new(UmbracoSource::new()));
                plugin_manager.register_source(
                    "wordpress",
                    Box::new(porter::sources::wordpress::WordPressSource::new()),
                );
                plugin_manager.register_target("payload", Box::new(PayloadTarget::new()));

                // Iterate collections
                let total_collections = config.collections.len();
                for (index, collection) in config.collections.iter().enumerate() {
                    let current = index + 1;
                    println!("\n{}", format!("🔄 Migrating Collection: {}/{} - {}", current, total_collections, collection.name).cyan().bold());
                    println!("{}", "─".repeat(80));

                    // Read porter-format docs
                    let porter_dir = config.metadata.as_ref()
                        .and_then(|m| m.get("porter_format_dir")).cloned()
                        .unwrap_or_else(|| "./porter-format".to_string());
                    let porter_path = format!("{}/{}.json", porter_dir, collection.name);
                    if !std::path::Path::new(&porter_path).exists() {
                        return Err(anyhow!(
                            "Porter-format file not found for collection '{}': {}. Run 'porter generate' first.",
                            collection.name, porter_path
                        ));
                    }
                    let porter_content = std::fs::read_to_string(&porter_path)?;
                    let docs: Vec<serde_json::Value> = serde_json::from_str(&porter_content)
                        .map_err(|e| anyhow!("Failed to parse porter-format JSON for '{}': {}", collection.name, e))?;

                    // Load existing mapping (do not create during migrate)
                    let mapping = mapping::load_mapping_only(
                        &collection.name,
                        &config.source,
                        &config.target,
                    )?;

                    // Transform docs
                    let mut transformed_docs = Vec::new();
                    for doc in &docs {
                        let transformed = mapping::apply_mapping(doc, &mapping)?;
                        transformed_docs.push(transformed);
                    }

                    // Emit seed files (Payload MVP)
                    let target_adapter = plugin_manager
                        .get_target(&config.target)
                        .ok_or_else(|| anyhow!("Unsupported target: {}", config.target))?;

                    let opts = TargetOptions {
                        collection: Some(collection.name.clone()),
                        locale: collection.locale.clone(),
                        related_collections: collection.related_collections.clone(),
                        ..Default::default()
                    };

                    // Use seeds_dir from [io] if present, fall back to top-level output
                    let seeds_base = config.io.as_ref().map(|io| io.seeds_dir.clone()).unwrap_or_else(|| config.output.clone());
                    let output_path = format!("{}/{}", seeds_base, collection.name);
                    info!("Writing seed file to {}", output_path);
                    target_adapter.emit_seed(&transformed_docs, &output_path, &opts)?;

                    println!("{}", format!("✓ Migration completed for '{}'", collection.name).green());
                    println!("  Documents: {}", transformed_docs.len());
                    println!("  Output: {}", output_path);
                }

                println!("\n{}", "🎉 Migration Finished".green().bold());
                println!("{}", format!("Processed {} collections", config.collections.len()).cyan());
                return Ok(());
            }
            Commands::Explain { config: config_file } => {
                println!("{}", "📚 Porter Migration Process Explanation".cyan().bold());
                println!();
                
                let config = if let Some(config_path) = config_file {
                    PorterConfig::load_from_file(&config_path)?
                } else if let Some(file_config) = find_and_load_config()? {
                    file_config
                } else {
                    println!("{}", "No configuration file found. Here's the general migration process:".yellow());
                    explain_general_migration_process();
                    return Ok(());
                };
                
                explain_migration_process(&config);
                return Ok(());
            }
            Commands::Upgrade { force } => {
                println!("{}", "⬆️  Upgrading Porter".cyan().bold());
                dlog!("Upgrade command invoked with force={}", force);
                upgrade_porter(force).await?;
                return Ok(());
            }
        }
    }

    // Load configuration from file if available
    let mut config = if let Some(file_config) = find_and_load_config()? {
        println!("{}", "📁 Using configuration from file".cyan());
        println!(
            "DEBUG: File config - interactive: {}, verbose: {}",
            file_config.interactive, file_config.verbose
        );
        file_config
    } else {
        println!(
            "{}",
            "📝 No configuration file found, using command line arguments".yellow()
        );
        println!("Run 'porter init' to create a new configuration");
        PorterConfig::default()
    };

    // If fixtures mode is enabled, use default test data
    if cli.fixtures {
        info!("Using fixtures mode with default test data");
        config.source = "umbraco".to_string();
        config.target = "payload".to_string();
        config.output = "./seed".to_string();

        // Create default collection for fixtures
        let default_collection = CollectionConfig {
            name: "hotels".to_string(),
            source_data: "./test-data/umbraco.json".to_string(),
            collection_path: Some("./test-data/payload-collection.ts".to_string()),
            locale: None,
            related_collections: None,
        };
        config.collections = vec![default_collection];
        debug!("Fixtures configuration: {:?}", config);
    }

    // Only override config with CLI arguments if they are explicitly provided
    if cli.source.is_some() {
        config.source = cli.source.unwrap();
    }
    if cli.target.is_some() {
        config.target = cli.target.unwrap();
    }
    if cli.output != "./seed" {
        // Only override if not default
        config.output = cli.output;
    }
    if cli.plugin_dir.is_some() {
        config.plugin_dir = cli.plugin_dir;
    }
    // Only override boolean flags if they are explicitly set via CLI
    if cli.interactive {
        config.interactive = true;
    }
    if cli.verbose {
        config.verbose = true;
    }
    if cli.dry_run {
        config.dry_run = true;
    }
    if cli.debug {
        config.debug = true;
    }
    if cli.fixtures {
        config.fixtures = true;
    }

    // Validate configuration
    config.validate()?;

    // Initialize simple debug printer
    dbgutil::init(config.debug);
    dlog!(
        "Configuration loaded - interactive: {}, verbose: {}, debug: {}",
        config.interactive,
        config.verbose,
        config.debug
    );

    // Set log level based on configuration
    if config.verbose {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
    if config.debug {
        unsafe {
            std::env::set_var("RUST_LOG", "debug");
        }
    }
    env_logger::init();

    // Extract values from config
    let source = &config.source;
    let target = &config.target;

    // Initialize mappings base directory to <output>/mappings
    initialize_mappings_base(&format!("{}/mappings", config.output));

    // Initialize plugin manager and register built-in adapters
    let mut plugin_manager = PluginManager::new();

    // Register built-in source adapters
    plugin_manager.register_source("umbraco", Box::new(UmbracoSource::new()));
    plugin_manager.register_source(
        "wordpress",
        Box::new(porter::sources::wordpress::WordPressSource::new()),
    );

    // Register built-in target adapters
    plugin_manager.register_target("payload", Box::new(PayloadTarget::new()));

    // Load plugins from directory (custom or default)
    let plugin_dir = match &config.plugin_dir {
        Some(dir) => Path::new(dir).to_path_buf(),
        None => get_default_plugin_dir(),
    };
    info!("Loading plugins from {:?}", plugin_dir);
    dlog!("Plugin directory resolved to {:?}", plugin_dir);
    if let Err(e) = plugin_manager.load_plugins_from_directory(&plugin_dir) {
        warn!("Error loading plugins: {}", e);
    }

    // List available adapters
    let source_adapters = plugin_manager.list_sources();
    let target_adapters = plugin_manager.list_targets();
    info!("Available source adapters: {:?}", source_adapters);
    info!("Available target adapters: {:?}", target_adapters);

    // If list_adapters flag is set, just print the adapters and exit
    if cli.list_adapters {
        println!("Available source adapters:");
        for adapter in &source_adapters {
            println!("  - {}", adapter);
        }
        println!("\nAvailable target adapters:");
        for adapter in &target_adapters {
            println!("  - {}", adapter);
        }
        return Ok(());
    }

    // Process each collection
    let total_collections = config.collections.len();
    for (collection_index, collection_config) in config.collections.iter().enumerate() {
        let current_collection = collection_index + 1;

        println!();
        println!(
            "{}",
            format!(
                "🔄 Processing Collection: {}/{} - {}",
                current_collection, total_collections, collection_config.name
            )
            .cyan()
            .bold()
        );
        println!("{}", "─".repeat(80));

        if current_collection < total_collections {
            let remaining_collections = total_collections - current_collection;
            println!(
                "{}",
                format!("📋 Collections remaining: {}", remaining_collections).yellow()
            );
            println!();
        }

        // 1) Read source docs for this collection
        info!(
            "Reading source documents from {:?}",
            collection_config.source_data
        );
        let source_adapter = plugin_manager
            .get_source(source)
            .ok_or_else(|| anyhow!("Unsupported source: {}", source))?;
        
        let docs = if source == "wordpress" && 
            config.metadata.as_ref().and_then(|m| m.get("wordpress_input_type")).map(|s| s.as_str()) == Some("api") {
            // For WordPress API, we need to fetch from the API
            info!("Fetching data from WordPress API endpoint: {}", collection_config.source_data);
            
            // Get WordPress API URL from config
            let api_url = config.metadata.as_ref()
                .and_then(|m| m.get("wordpress_api_url"))
                .ok_or_else(|| anyhow!("WordPress API URL not found in configuration. Run 'porter init' to configure."))?;
            
            // Initialize WordPress source with API configuration
            let mut wp_source = porter::sources::wordpress::WordPressSource::new();
            let wp_config = porter::sources::wordpress::WordPressConfig {
                format: porter::sources::wordpress::WordPressFormat::Api,
                content_types: vec![collection_config.source_data.clone()],
                include_drafts: false,
                include_private: false,
                include_media: true,
                include_comments: false,
                include_users: true,
                include_taxonomies: true,
                include_acf_fields: true,
                field_mappings: std::collections::HashMap::new(),
                custom_field_processors: std::collections::HashMap::new(),
                shortcode_processing: true,
                media_processing_config: porter::sources::wordpress::MediaProcessingConfig::default(),
            };
            wp_source.set_wp_config(wp_config);
            
            // Initialize the source adapter
            // Build auth_config from metadata if present
            let auth_config = config.metadata.as_ref().and_then(|m| porter::util::auth::build_auth_config_from_metadata(m));

            let source_config = porter::adapters::SourceConfig {
                adapter_type: "wordpress".to_string(),
                connection_method: ConnectionMethod::Api {
                    endpoint: api_url.clone(),
                    auth_config,
                },
                adapter_config: serde_json::Value::Object(serde_json::Map::new()),
            };
            wp_source.init(&source_config)?;
            
            // Fetch documents from API
            let query = porter::adapters::SourceQuery {
                query: Some(collection_config.source_data.clone()),
                parameters: std::collections::HashMap::new(),
                limit: None,
                offset: None,
                filters: std::collections::HashMap::new(),
            };
            
            // Use the async fetch_documents method directly (already in #[tokio::main])
            wp_source.fetch_documents(&query).await?
        } else {
            // For file-based sources, read from files
            source_adapter.read_documents(&[collection_config.source_data.clone()])?
        };
        info!(
            "Read {} documents for collection '{}'",
            docs.len(),
            collection_config.name
        );

        // 2) Optionally write porter-format normalized files (future: move to generate path)
        // Skipping write here to avoid side-effects; generator now writes porter-format.

        // 3) Load/create mapping for this collection
        info!(
            "Loading or creating mapping for collection '{}'",
            collection_config.name
        );
        // Migrate requires a pre-generated mapping
        let mapping = mapping::load_mapping_only(
            &collection_config.name,
            source,
            target,
        )?;
        info!(
            "Mapping loaded with {} field mappings",
            mapping.field_mappings.len()
        );

        // 4) Transform docs using mapping (with batch processing for large datasets)
        let target_adapter = plugin_manager
            .get_target(target)
            .ok_or_else(|| anyhow!("Unsupported target: {}", target))?;

        let opts = TargetOptions {
            collection: Some(collection_config.name.clone()),
            locale: collection_config.locale.clone(),
            related_collections: collection_config.related_collections.clone(),
            ..Default::default()
        };

        if config.dry_run {
            info!(
                "Dry run - not writing output files for collection '{}'",
                collection_config.name
            );
            // For dry run, just validate the mapping
            let validation_result = mapping::validate_mapping_with_documents(
                &mapping,
                &docs,
                collection_config.collection_path.as_deref(),
            )?;

            if validation_result.is_valid {
                println!(
                    "{}",
                    format!(
                        "✓ Mapping validation passed for '{}'",
                        collection_config.name
                    )
                    .green()
                );
            } else {
                println!(
                    "{}",
                    format!(
                        "⚠ Mapping validation failed for '{}'",
                        collection_config.name
                    )
                    .yellow()
                );
                for error in &validation_result.errors {
                    println!("  Error: {} - {}", error.field, error.message);
                }
            }
        } else {
            // Use optimized processing based on dataset size
            if docs.len() > 1000 {
                info!(
                    "Large dataset detected ({} documents), using optimized batch processing",
                    docs.len()
                );

                let batch_config = BatchConfig {
                    batch_size: 100,
                    max_memory_mb: 512,
                    enable_resume: true,
                    progress_interval: 5,
                    temp_dir: format!("./temp/{}", collection_config.name),
                };

                let perf_config = PerformanceConfig {
                    num_threads: num_cpus::get(),
                    memory_limit_mb: 1024,
                    chunk_size: 50,
                    enable_memory_monitoring: true,
                    memory_monitor_interval: 5,
                    enable_adaptive_chunking: true,
                };

                let mut optimized_processor =
                    OptimizedBatchProcessor::new(batch_config, perf_config);
                let transformed_docs =
                    optimized_processor.process_documents_optimized(&docs, &mapping)?;

                let seeds_base = config.io.as_ref().map(|io| io.seeds_dir.clone()).unwrap_or_else(|| config.output.clone());
                let output_path = format!("{}/{}", seeds_base, collection_config.name);
                info!("Writing seed file to {}", output_path);
                target_adapter.emit_seed(&transformed_docs, &output_path, &opts)?;

                println!(
                    "{}",
                    format!(
                        "✓ Optimized processing completed for '{}'",
                        collection_config.name
                    )
                    .green()
                );
                println!("  Processed: {} documents", transformed_docs.len());
                println!(
                    "  Time: {:.1}s",
                    optimized_processor
                        .performance_processor
                        .get_performance_stats()
                        .processing_time_seconds
                );
            } else if docs.len() > 100 {
                info!(
                    "Medium dataset ({} documents), using parallel processing",
                    docs.len()
                );

                let perf_config = PerformanceConfig {
                    num_threads: num_cpus::get(),
                    memory_limit_mb: 512,
                    chunk_size: 25,
                    enable_memory_monitoring: false,
                    memory_monitor_interval: 5,
                    enable_adaptive_chunking: true,
                };

                let mut perf_processor = PerformanceProcessor::new(perf_config);
                let transformed_docs =
                    perf_processor.process_documents_parallel(&docs, &mapping)?;

                let seeds_base = config.io.as_ref().map(|io| io.seeds_dir.clone()).unwrap_or_else(|| config.output.clone());
                let output_path = format!("{}/{}", seeds_base, collection_config.name);
                info!("Writing seed file to {}", output_path);
                target_adapter.emit_seed(&transformed_docs, &output_path, &opts)?;

                println!(
                    "{}",
                    format!(
                        "✓ Parallel processing completed for '{}'",
                        collection_config.name
                    )
                    .green()
                );
                println!("  Processed: {} documents", transformed_docs.len());
                println!(
                    "  Time: {:.1}s",
                    perf_processor
                        .get_performance_stats()
                        .processing_time_seconds
                );
            } else {
                // Use simple processing for small datasets
                info!(
                    "Small dataset ({} documents), using simple processing",
                    docs.len()
                );
                let mut transformed_docs = Vec::new();
                for doc in &docs {
                    let transformed = mapping::apply_mapping(doc, &mapping)?;
                    transformed_docs.push(transformed);
                }

                let seeds_base = config.io.as_ref().map(|io| io.seeds_dir.clone()).unwrap_or_else(|| config.output.clone());
                let output_path = format!("{}/{}", seeds_base, collection_config.name);
                info!("Writing seed file to {}", output_path);
                target_adapter.emit_seed(&transformed_docs, &output_path, &opts)?;
                info!(
                    "Migration completed successfully for collection '{}'",
                    collection_config.name
                );
            }
        }
    }

    println!();
    println!(
        "{}",
        "🎉 All Collections Processed Successfully!".green().bold()
    );
    println!(
        "{}",
        format!("Processed {} collections", config.collections.len()).cyan()
    );

    Ok(())
}

/// Explain the general migration process
fn explain_general_migration_process() {
    use colored::Colorize;
    
    println!("{}", "🔄 General Migration Process".cyan().bold());
    println!("{}", "─".repeat(50));
    println!();
    
    println!("{}", "1. Initialise Configuration".yellow().bold());
    println!("   Run: {}", "porter init".green());
    println!("   • Select your source system (Umbraco, WordPress, etc.)");
    println!("   • Select your target system (Payload, Strapi, etc.)");
    println!("   • Configure collections and their data sources");
    println!();
    
    println!("{}", "2. Generate Mappings".yellow().bold());
    println!("   Run: {}", "porter generate".green());
    println!("   • Analyses source data structure");
    println!("   • Analyses target collection schemas");
    println!("   • Creates field mapping files");
    println!("   • Allows interactive field mapping");
    println!();
    
    println!("{}", "3. Migrate Data".yellow().bold());
    println!("   Run: {}", "porter migrate".green());
    println!("   • Reads source data");
    println!("   • Applies field mappings");
    println!("   • Transforms data to target format");
    println!("   • Generates seed files for target system");
    println!();
    
    println!("{}", "📋 Available Commands".cyan().bold());
    println!("{}", "─".repeat(30));
    println!("{} - Initialise configuration", "porter init".green());
    println!("{} - Generate field mappings", "porter generate".green());
    println!("{} - Migrate data", "porter migrate".green());
    println!("{} - Show current configuration", "porter config".green());
    println!("{} - Explain migration process", "porter explain".green());
    println!();
}

/// Explain the migration process for a specific configuration
fn explain_migration_process(config: &PorterConfig) {
    use colored::Colorize;
    
    println!("{}", "📋 Your Migration Configuration".cyan().bold());
    println!("{}", "─".repeat(50));
    println!("Source: {}", config.source.blue());
    println!("Target: {}", config.target.blue());
    println!("Output: {}", config.output.blue());
    println!("Collections: {}", config.collections.len().to_string().blue());
    println!();
    
    // Show collection details
    for (i, collection) in config.collections.iter().enumerate() {
        println!("{} Collection {}: {}", "📁".cyan(), (i + 1).to_string().yellow(), collection.name.blue());
        println!("   Source: {}", collection.source_data.blue());
        if let Some(path) = &collection.collection_path {
            println!("   Schema: {}", path.blue());
        }
        println!();
    }
    
    // Explain the process based on source type
    match config.source.as_str() {
        "wordpress" => {
            if let Some(input_type) = config.metadata.as_ref().and_then(|m| m.get("wordpress_input_type")) {
                if input_type == "api" {
                    explain_wordpress_api_migration(config);
                } else {
                    explain_wordpress_file_migration(config);
                }
            } else {
                explain_wordpress_file_migration(config);
            }
        }
        "umbraco" => explain_umbraco_migration(config),
        _ => explain_general_migration_process(),
    }
}

/// Explain WordPress API migration process
fn explain_wordpress_api_migration(config: &PorterConfig) {
    use colored::Colorize;
    
    println!("{}", "🌐 WordPress API Migration Process".cyan().bold());
    println!("{}", "─".repeat(50));
    println!();
    
    if let Some(api_url) = config.metadata.as_ref().and_then(|m| m.get("wordpress_api_url")) {
        println!("WordPress API URL: {}", api_url.blue());
        println!();
    }

    println!("{}", "Step 0: Initialise Templates".yellow().bold());
    println!("   Run: {}", "porter template".green());
    println!("   • Creates templates for each collection");
    println!("   • Creates TypeScript templates for each collection");
    println!();
    
    println!("{}", "Step 1: Generate Mappings".yellow().bold());
    println!("   Run: {}", "porter generate".green());
    println!("   • Connects to WordPress API at the configured URL");
    println!("   • Discovers available endpoints (posts, pages, media, etc.)");
    println!("   • Fetches sample data from each configured endpoint");
    println!("   • Analyses WordPress field structure");
    println!("   • Analyses Payload collection schemas");
    println!("   • Creates field mapping files in ./mappings/ directory");
    println!("   • Allows interactive field mapping for complex fields");
    println!();
    
    println!("{}", "Step 2: Review and Adjust Mappings".yellow().bold());
    println!("   • Check generated mapping files in ./mappings/");
    println!("   • Edit mappings if needed for custom field transformations");
    println!("   • Test mappings with a small dataset if desired");
    println!();
    
    println!("{}", "Step 3: Migrate Data".yellow().bold());
    println!("   Run: {}", "porter migrate".green());
    println!("   • Connects to WordPress API");
    println!("   • Fetches all data from configured endpoints");
    println!("   • Applies field mappings to transform data");
    println!("   • Generates Payload seed files in {}", config.output.blue());
    println!("   • Creates TypeScript seed files for each collection");
    println!();
    
    println!("{}", "Step 4: Import to Payload".yellow().bold());
    println!("   • Copy generated seed files to your Payload project");
    println!("   • Run Payload's seed command to import data");
    println!("   • Verify data in Payload admin interface");
    println!();
    
    println!("{}", "🔧 Collection Details".cyan().bold());
    for collection in &config.collections {
        println!("• {} → {}", collection.source_data.blue(), collection.name.blue());
        if let Some(schema_path) = &collection.collection_path {
            println!("  Schema: {}", schema_path.blue());
        }
    }
    println!();
}

/// Explain WordPress file migration process
fn explain_wordpress_file_migration(_config: &PorterConfig) {
    use colored::Colorize;
    
    println!("{}", "📄 WordPress File Migration Process".cyan().bold());
    println!("{}", "─".repeat(50));
    println!();
    
    println!("{}", "Step 1: Generate Mappings".yellow().bold());
    println!("   Run: {}", "porter generate".green());
    println!("   • Reads WordPress WXR export files");
    println!("   • Parses WordPress XML structure");
    println!("   • Analyses WordPress field structure");
    println!("   • Analyses Payload collection schemas");
    println!("   • Creates field mapping files");
    println!();
    
    println!("{}", "Step 2: Migrate Data".yellow().bold());
    println!("   Run: {}", "porter migrate".green());
    println!("   • Reads WordPress export files");
    println!("   • Applies field mappings");
    println!("   • Generates Payload seed files");
    println!();
}

/// Explain Umbraco migration process
fn explain_umbraco_migration(_config: &PorterConfig) {
    use colored::Colorize;
    
    println!("{}", "🏢 Umbraco Migration Process".cyan().bold());
    println!("{}", "─".repeat(50));
    println!();
    
    println!("{}", "Step 1: Generate Mappings".yellow().bold());
    println!("   Run: {}", "porter generate".green());
    println!("   • Reads Umbraco JSON export files");
    println!("   • Analyses Umbraco content structure");
    println!("   • Analyses Payload collection schemas");
    println!("   • Creates field mapping files");
    println!();
    
    println!("{}", "Step 2: Migrate Data".yellow().bold());
    println!("   Run: {}", "porter migrate".green());
    println!("   • Reads Umbraco export files");
    println!("   • Applies field mappings");
    println!("   • Generates Payload seed files");
    println!();
}

async fn upgrade_porter(force: bool) -> Result<()> {
    use std::process::Command;
    
    println!("{}", "Checking for Porter updates...".cyan());
    
    // First, update Homebrew
    println!("{}", "Updating Homebrew...".yellow());
    let update_result = Command::new("brew")
        .arg("update")
        .output();
    
    match update_result {
        Ok(output) => {
            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!("Failed to update Homebrew: {}", error));
            }
            println!("{}", "✅ Homebrew updated successfully".green());
        }
        Err(e) => {
            return Err(anyhow!("Failed to run 'brew update': {}. Make sure Homebrew is installed.", e));
        }
    }
    
    // Check current version
    let current_version = env!("CARGO_PKG_VERSION");
    println!("{}", format!("Current version: {}", current_version).cyan());
    
    // Check if there's a newer version available
    let outdated_result = Command::new("brew")
        .arg("outdated")
        .arg("porter")
        .output();
    
    let has_updates = match outdated_result {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                !stdout.trim().is_empty()
            } else {
                // If brew outdated fails, assume no updates (might be a network issue)
                false
            }
        }
        Err(_) => false,
    };
    
    if !has_updates && !force {
        println!("{}", "✅ Porter is already up to date!".green());
        println!("{}", "Use --force to upgrade anyway".yellow());
        return Ok(());
    }
    
    if has_updates {
        println!("{}", "🔄 New version available! Upgrading...".yellow());
    } else if force {
        println!("{}", "🔄 Force upgrading...".yellow());
    }
    
    // Perform the upgrade
    let upgrade_result = Command::new("brew")
        .arg("upgrade")
        .arg("porter")
        .output();
    
    match upgrade_result {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                println!("{}", "✅ Porter upgraded successfully!".green());
                
                if !stdout.trim().is_empty() {
                    println!();
                    println!("{}", "Upgrade output:".cyan());
                    println!("{}", stdout);
                }
                
                // Show new version
                let version_result = Command::new("porter")
                    .arg("--version")
                    .output();
                
                if let Ok(version_output) = version_result {
                    if version_output.status.success() {
                        let version = String::from_utf8_lossy(&version_output.stdout);
                        println!("{}", format!("New version: {}", version.trim()).green());
                    }
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!("Failed to upgrade Porter: {}", stderr));
            }
        }
        Err(e) => {
            return Err(anyhow!("Failed to run 'brew upgrade porter': {}. Make sure Porter is installed via Homebrew.", e));
        }
    }
    
    Ok(())
}
