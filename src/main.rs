mod cli;

use porter::adapter::TargetOptions;
use porter::sources::umbraco::UmbracoSource;
use porter::targets::payload::PayloadTarget;
use porter::plugin::{PluginManager, get_default_plugin_dir};
use porter::mapping;
use porter::batch::BatchConfig;
use porter::performance::{PerformanceProcessor, PerformanceConfig, OptimizedBatchProcessor};
use porter::config::{PorterConfig, CollectionConfig, find_and_load_config, create_config_interactively};
use crate::cli::{Cli, Commands};
use clap::Parser;
use anyhow::{Result, anyhow};
use log::{info, debug, warn};
use std::path::Path;
use colored::Colorize;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle subcommands first
    if let Some(command) = cli.command {
        match command {
            Commands::Init { output } => {
                println!("{}", "🚀 Initializing Porter Configuration".cyan().bold());
                let config = create_config_interactively()?;
                config.save_to_file(&output)?;
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
        }
    }

    // Load configuration from file if available
    let mut config = if let Some(file_config) = find_and_load_config()? {
        println!("{}", "📁 Using configuration from file".cyan());
        println!("DEBUG: File config - interactive: {}, verbose: {}", file_config.interactive, file_config.verbose);
        file_config
    } else {
        println!("{}", "📝 No configuration file found, using command line arguments".yellow());
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
    if cli.output != "./seed" { // Only override if not default
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

    // Debug: Print configuration values
    println!("DEBUG: Configuration loaded - interactive: {}, verbose: {}, debug: {}", 
             config.interactive, config.verbose, config.debug);

    // Set log level based on configuration
    if config.verbose {
        unsafe { std::env::set_var("RUST_LOG", "info"); }
    }
    if config.debug {
        unsafe { std::env::set_var("RUST_LOG", "debug"); }
    }
    env_logger::init();

    // Extract values from config
    let source = &config.source;
    let target = &config.target;

    // Initialize plugin manager and register built-in adapters
    let mut plugin_manager = PluginManager::new();

    // Register built-in source adapters
    plugin_manager.register_source("umbraco", Box::new(UmbracoSource::new()));

    // Register built-in target adapters
    plugin_manager.register_target("payload", Box::new(PayloadTarget::new()));

    // Load plugins from directory (custom or default)
    let plugin_dir = match &config.plugin_dir {
        Some(dir) => Path::new(dir).to_path_buf(),
        None => get_default_plugin_dir(),
    };
    info!("Loading plugins from {:?}", plugin_dir);
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
        println!("{}", format!("🔄 Processing Collection: {}/{} - {}", current_collection, total_collections, collection_config.name).cyan().bold());
        println!("{}", "─".repeat(80));
        
        if current_collection < total_collections {
            let remaining_collections = total_collections - current_collection;
            println!("{}", format!("📋 Collections remaining: {}", remaining_collections).yellow());
            println!();
        }

        // 1) Read source docs for this collection
        info!("Reading source documents from {:?}", collection_config.source_data);
        let source_adapter = plugin_manager.get_source(source)
            .ok_or_else(|| anyhow!("Unsupported source: {}", source))?;
        let docs = source_adapter.read_documents(&[collection_config.source_data.clone()])?;
        info!("Read {} documents for collection '{}'", docs.len(), collection_config.name);

        // 2) Load/create mapping for this collection
        info!("Loading or creating mapping for collection '{}'", collection_config.name);
        let mapping = mapping::load_or_create_mapping(
            &collection_config.name,
            source,
            target,
            &docs,
            collection_config.collection_path.as_deref(),
            config.interactive,
            Some((current_collection, total_collections))
        )?;
        info!("Mapping loaded with {} field mappings", mapping.field_mappings.len());

        // 3) Transform docs using mapping (with batch processing for large datasets)
        let target_adapter = plugin_manager.get_target(target)
            .ok_or_else(|| anyhow!("Unsupported target: {}", target))?;

        let opts = TargetOptions {
            collection: Some(collection_config.name.clone()),
            locale: collection_config.locale.clone(),
            related_collections: collection_config.related_collections.clone(),
            ..Default::default()
        };

        if config.dry_run {
            info!("Dry run - not writing output files for collection '{}'", collection_config.name);
            // For dry run, just validate the mapping
            let validation_result = mapping::validate_mapping_with_documents(
                &mapping,
                &docs,
                collection_config.collection_path.as_deref(),
            )?;
            
            if validation_result.is_valid {
                println!("{}", format!("✓ Mapping validation passed for '{}'", collection_config.name).green());
            } else {
                println!("{}", format!("⚠ Mapping validation failed for '{}'", collection_config.name).yellow());
                for error in &validation_result.errors {
                    println!("  Error: {} - {}", error.field, error.message);
                }
            }
        } else {
            // Use optimized processing based on dataset size
            if docs.len() > 1000 {
                info!("Large dataset detected ({} documents), using optimized batch processing", docs.len());
                
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
                
                let mut optimized_processor = OptimizedBatchProcessor::new(batch_config, perf_config);
                let transformed_docs = optimized_processor.process_documents_optimized(&docs, &mapping)?;
                
                let output_path = format!("{}/{}", config.output, collection_config.name);
                info!("Writing seed file to {}", output_path);
                target_adapter.emit_seed(&transformed_docs, &output_path, &opts)?;
                
                println!("{}", format!("✓ Optimized processing completed for '{}'", collection_config.name).green());
                println!("  Processed: {} documents", transformed_docs.len());
                println!("  Time: {:.1}s", optimized_processor.performance_processor.get_performance_stats().processing_time_seconds);
            } else if docs.len() > 100 {
                info!("Medium dataset ({} documents), using parallel processing", docs.len());
                
                let perf_config = PerformanceConfig {
                    num_threads: num_cpus::get(),
                    memory_limit_mb: 512,
                    chunk_size: 25,
                    enable_memory_monitoring: false,
                    memory_monitor_interval: 5,
                    enable_adaptive_chunking: true,
                };
                
                let mut perf_processor = PerformanceProcessor::new(perf_config);
                let transformed_docs = perf_processor.process_documents_parallel(&docs, &mapping)?;
                
                let output_path = format!("{}/{}", config.output, collection_config.name);
                info!("Writing seed file to {}", output_path);
                target_adapter.emit_seed(&transformed_docs, &output_path, &opts)?;
                
                println!("{}", format!("✓ Parallel processing completed for '{}'", collection_config.name).green());
                println!("  Processed: {} documents", transformed_docs.len());
                println!("  Time: {:.1}s", perf_processor.get_performance_stats().processing_time_seconds);
            } else {
                // Use simple processing for small datasets
                info!("Small dataset ({} documents), using simple processing", docs.len());
                let mut transformed_docs = Vec::new();
                for doc in &docs {
                    let transformed = mapping::apply_mapping(doc, &mapping)?;
                    transformed_docs.push(transformed);
                }
                
                let output_path = format!("{}/{}", config.output, collection_config.name);
                info!("Writing seed file to {}", output_path);
                target_adapter.emit_seed(&transformed_docs, &output_path, &opts)?;
                info!("Migration completed successfully for collection '{}'", collection_config.name);
            }
        }
    }

    println!();
    println!("{}", "🎉 All Collections Processed Successfully!".green().bold());
    println!("{}", format!("Processed {} collections", config.collections.len()).cyan());

    Ok(())
}
