# Porter API Reference

This document provides a comprehensive reference for the Porter library API, including all public functions, structs, traits, and modules.

## Table of Contents

1. [Core Library](#core-library)
2. [Configuration](#configuration)
3. [Mapping System](#mapping-system)
4. [Adapter System](#adapter-system)
5. [Batch Processing](#batch-processing)
6. [Performance](#performance)
7. [Utilities](#utilities)

## Core Library

### `porter::lib`

The main library module that re-exports all public components.

```rust
pub mod adapter;
pub mod batch;
pub mod cli;
pub mod config;
pub mod mapping;
pub mod performance;
pub mod plugin;
pub mod sources;
pub mod targets;
pub mod util;
```

## Configuration

### `porter::config`

Configuration management for Porter migrations.

#### `PorterConfig`

Main configuration struct for Porter migrations.

```rust
pub struct PorterConfig {
    pub source: String,
    pub target: String,
    pub output: String,
    pub collections: Vec<CollectionConfig>,
    pub interactive: bool,
    pub verbose: bool,
    pub dry_run: bool,
    pub plugin_dir: Option<String>,
}
```

**Methods:**

- `default() -> Self` - Creates default configuration
- `load_from_file(path: &str) -> Result<Self>` - Loads configuration from file
- `save_to_file(&self, path: &str) -> Result<()>` - Saves configuration to file
- `merge(&mut self, cli_config: &PorterConfig) -> Result<()>` - Merges CLI config
- `validate(&self) -> Result<()>` - Validates configuration
- `display(&self)` - Displays configuration

#### `CollectionConfig`

Configuration for individual collections.

```rust
pub struct CollectionConfig {
    pub name: String,
    pub source_data: String,
    pub collection_path: Option<String>,
    pub locale: Option<String>,
    pub related_collections: Option<Vec<String>>,
}
```

**Methods:**

- `default() -> Self` - Creates default collection config

#### Functions

```rust
pub fn find_and_load_config() -> Result<Option<PorterConfig>>
```
Finds and loads configuration from common locations.

```rust
pub fn create_config_interactively() -> Result<PorterConfig>
```
Creates configuration through interactive prompts.

## Mapping System

### `porter::mapping`

Core mapping functionality for data transformation.

#### `Mapping`

Main mapping configuration struct.

```rust
pub struct Mapping {
    pub source: String,
    pub target: String,
    pub collection: String,
    pub field_mappings: Vec<FieldMapping>,
    pub block_mappings: Option<Vec<BlockMapping>>,
}
```

#### `FieldMapping`

Individual field mapping configuration.

```rust
pub struct FieldMapping {
    pub to: String,
    pub from: Value,
    pub transforms: Vec<String>,
    pub fallback: Option<String>,
}
```

#### Functions

```rust
pub fn load_or_create_mapping(
    collection_name: &str,
    source: &str,
    target: &str,
    documents: &[Value],
    schema_path: Option<&str>,
    interactive: bool,
) -> Result<Mapping>
```
Loads existing mapping or creates new one interactively.

```rust
pub fn apply_mapping(doc: &Value, mapping: &Mapping) -> Result<Value>
```
Applies mapping to transform a document.

```rust
pub fn extract_source_fields(documents: &[Value]) -> HashSet<String>
```
Extracts unique field names from source documents.

```rust
pub fn get_mapping_path(collection_name: &str, source: &str, target: &str) -> String
```
Generates mapping file path.

### `porter::mapping::nested`

Nested mapping support for complex data structures.

#### `NestedPath`

Represents a nested field path.

```rust
pub struct NestedPath {
    pub segments: Vec<PathSegment>,
}
```

#### `PathSegment`

Individual path segment types.

```rust
pub enum PathSegment {
    Field(String),
    Index(usize),
    Wildcard,
    Slice(Option<usize>, Option<usize>),
}
```

#### `NestedMapping`

Configuration for nested mappings.

```rust
pub struct NestedMapping {
    pub source_path: NestedPath,
    pub target_path: NestedPath,
    pub transform: Option<String>,
    pub flatten: bool,
    pub separator: Option<String>,
}
```

#### Functions

```rust
pub fn extract_nested_value(doc: &Value, path: &NestedPath) -> Result<Vec<Value>>
```
Extracts values from nested paths in documents.

```rust
pub fn set_nested_value(doc: &mut Value, path: &NestedPath, values: &[Value]) -> Result<()>
```
Sets values at nested paths in documents.

```rust
pub fn apply_nested_mapping(doc: &Value, mapping: &NestedMapping) -> Result<Value>
```
Applies nested mapping to a document.

### `porter::mapping::validation`

Mapping validation functionality.

#### `ValidationResult`

Result of mapping validation.

```rust
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}
```

#### `ValidationError`

Validation error information.

```rust
pub struct ValidationError {
    pub field: String,
    pub message: String,
    pub severity: ErrorSeverity,
}
```

#### Functions

```rust
pub fn validate_mapping(
    mapping: &Mapping,
    source_schema: &SchemaInfo,
    target_schema: &SchemaInfo,
) -> ValidationResult
```
Validates mapping against source and target schemas.

```rust
pub fn create_schema_from_documents(documents: &[Value]) -> SchemaInfo
```
Creates schema information from document samples.

## Adapter System

### `porter::adapter`

Adapter traits for source and target systems.

#### `SourceReader`

Trait for reading from source systems.

```rust
pub trait SourceReader {
    fn read_documents(&self, source_files: &[String]) -> Result<Vec<Value>>;
}
```

#### `TargetWriter`

Trait for writing to target systems.

```rust
pub trait TargetWriter {
    fn emit_seed(
        &self,
        documents: &[Value],
        output_path: &str,
        options: &TargetOptions,
    ) -> Result<()>;
}
```

#### `TargetOptions`

Options for target writing.

```rust
pub struct TargetOptions {
    pub collection: Option<String>,
    pub locale: Option<String>,
    pub related_collections: Option<Vec<String>>,
}
```

### `porter::plugin`

Plugin system for dynamic adapter loading.

#### `PluginManager`

Manages plugin loading and registration.

```rust
pub struct PluginManager {
    sources: HashMap<String, Box<dyn SourceReader>>,
    targets: HashMap<String, Box<dyn TargetWriter>>,
}
```

**Methods:**

- `new() -> Self` - Creates new plugin manager
- `register_source(&mut self, name: &str, adapter: Box<dyn SourceReader>)` - Registers source adapter
- `register_target(&mut self, name: &str, adapter: Box<dyn TargetWriter>)` - Registers target adapter
- `get_source(&self, name: &str) -> Option<&dyn SourceReader>` - Gets source adapter
- `get_target(&self, name: &str) -> Option<&dyn TargetWriter>` - Gets target adapter
- `list_sources(&self) -> Vec<String>` - Lists available source adapters
- `list_targets(&self) -> Vec<String>` - Lists available target adapters
- `load_plugins_from_directory(&mut self, dir: &Path) -> Result<()>` - Loads plugins from directory

#### Functions

```rust
pub fn get_default_plugin_dir() -> PathBuf
```
Gets default plugin directory path.

## Batch Processing

### `porter::batch`

Batch processing for large datasets.

#### `BatchConfig`

Configuration for batch processing.

```rust
pub struct BatchConfig {
    pub batch_size: usize,
    pub max_memory_mb: usize,
    pub enable_resume: bool,
    pub progress_interval: usize,
    pub temp_dir: String,
}
```

#### `BatchProcessor`

Main batch processing engine.

```rust
pub struct BatchProcessor {
    config: BatchConfig,
    stats: BatchStats,
    errors: Vec<BatchError>,
    start_time: Instant,
}
```

**Methods:**

- `new(config: BatchConfig) -> Self` - Creates new batch processor
- `process_documents(&mut self, source: &dyn SourceReader, target: &dyn TargetWriter, mapping: &Mapping, source_files: &[String], output_path: &str, options: &TargetOptions) -> Result<BatchResult>` - Processes documents in batches

#### `BatchStats`

Statistics for batch processing.

```rust
pub struct BatchStats {
    pub total_documents: usize,
    pub processed_documents: usize,
    pub failed_documents: usize,
    pub current_batch: usize,
    pub total_batches: usize,
    pub memory_usage_mb: f64,
    pub processing_time_seconds: f64,
}
```

#### `BatchResult`

Result of batch processing.

```rust
pub struct BatchResult {
    pub success: bool,
    pub stats: BatchStats,
    pub errors: Vec<BatchError>,
    pub checkpoint_file: Option<String>,
}
```

#### Functions

```rust
pub fn create_batch_processor() -> BatchProcessor
```
Creates batch processor with default configuration.

```rust
pub fn create_batch_processor_with_config(config: BatchConfig) -> BatchProcessor
```
Creates batch processor with custom configuration.

## Performance

### `porter::performance`

Performance optimization and parallel processing.

#### `PerformanceConfig`

Configuration for performance optimizations.

```rust
pub struct PerformanceConfig {
    pub num_threads: usize,
    pub memory_limit_mb: usize,
    pub chunk_size: usize,
    pub enable_memory_monitoring: bool,
    pub memory_monitor_interval: u64,
    pub enable_adaptive_chunking: bool,
}
```

#### `PerformanceProcessor`

High-performance document processor.

```rust
pub struct PerformanceProcessor {
    config: PerformanceConfig,
    memory_stats: Arc<Mutex<MemoryStats>>,
    start_time: Instant,
    memory_monitor_handle: Option<JoinHandle<()>>,
    stop_monitoring: Arc<Mutex<bool>>,
}
```

**Methods:**

- `new(config: PerformanceConfig) -> Self` - Creates new performance processor
- `process_documents_parallel(&mut self, documents: &[Value], mapping: &Mapping) -> Result<Vec<Value>>` - Processes documents in parallel
- `process_documents_memory_aware(&mut self, documents: &[Value], mapping: &Mapping) -> Result<Vec<Value>>` - Processes documents with memory awareness
- `get_memory_stats(&self) -> Result<MemoryStats>` - Gets current memory statistics
- `get_performance_stats(&self) -> PerformanceStats` - Gets performance statistics

#### `MemoryStats`

Memory usage statistics.

```rust
pub struct MemoryStats {
    pub current_mb: f64,
    pub peak_mb: f64,
    pub available_mb: f64,
    pub total_mb: f64,
    pub usage_percentage: f64,
}
```

#### `PerformanceStats`

Performance statistics.

```rust
pub struct PerformanceStats {
    pub processing_time_seconds: f64,
    pub memory_stats: MemoryStats,
    pub num_threads: usize,
    pub chunk_size: usize,
}
```

#### `OptimizedBatchProcessor`

Enhanced batch processor with performance optimizations.

```rust
pub struct OptimizedBatchProcessor {
    _batch_processor: BatchProcessor,
    pub performance_processor: PerformanceProcessor,
}
```

**Methods:**

- `new(batch_config: BatchConfig, perf_config: PerformanceConfig) -> Self` - Creates optimized batch processor
- `process_documents_optimized(&mut self, documents: &[Value], mapping: &Mapping) -> Result<Vec<Value>>` - Processes documents with optimizations

## Utilities

### `porter::util`

Utility functions and helpers.

#### `porter::util::fs`

File system utilities.

```rust
pub fn read_json_file(path: &str) -> Result<Value>
```
Reads JSON file and returns parsed value.

```rust
pub fn write_json_file(path: &str, data: &Value) -> Result<()>
```
Writes JSON data to file.

```rust
pub fn ensure_directory_exists(path: &str) -> Result<()>
```
Ensures directory exists, creating if necessary.

```rust
pub fn delete_file(path: &str) -> Result<()>
```
Deletes file if it exists.

#### `porter::util::interact`

Interactive user prompts.

```rust
pub fn prompt(message: &str) -> Result<String>
```
Prompts user for input.

```rust
pub fn confirm(message: &str) -> Result<bool>
```
Prompts user for confirmation.

```rust
pub fn select(message: &str, options: &[String]) -> Result<usize>
```
Prompts user to select from options.

```rust
pub fn select_with_arrows(message: &str, options: &[String], used_fields: &HashSet<String>) -> Result<usize>
```
Interactive selection with arrow keys.

```rust
pub fn confirm_with_default(message: &str, default: bool) -> Result<bool>
```
Confirmation with default value.

```rust
pub fn select_with_default(message: &str, options: &[String], default: usize) -> Result<usize>
```
Selection with default value.

```rust
pub fn display_mapping_operation(operation: &str, field: &str)
```
Displays mapping operation information.

```rust
pub fn display_mapping_summary(mappings: &[(&str, &str)])
```
Displays mapping summary.

```rust
pub fn confirm_mappings(mappings: &[(&str, &str)]) -> Result<bool>
```
Confirms mapping choices.

```rust
pub fn clear_screen()
```
Clears terminal screen.

## Error Handling

### Common Error Types

```rust
// Configuration errors
pub enum ConfigError {
    MissingField(String),
    InvalidValue(String),
    FileNotFound(String),
}

// Mapping errors
pub enum MappingError {
    FieldNotFound(String),
    InvalidTransform(String),
    TypeMismatch(String, String),
}

// Processing errors
pub enum ProcessingError {
    DocumentFailed(usize, String),
    BatchFailed(usize, String),
    MemoryExceeded(usize),
}
```

### Error Handling Patterns

```rust
// Using anyhow for error propagation
use anyhow::{Result, anyhow};

fn process_document(doc: &Value) -> Result<Value> {
    if doc.is_null() {
        return Err(anyhow!("Document is null"));
    }
    // Process document...
    Ok(result)
}

// Using thiserror for custom error types
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PorterError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Mapping error: {0}")]
    Mapping(String),
    #[error("Processing error: {0}")]
    Processing(String),
}
```

## Examples

### Basic Usage

```rust
use porter::{PorterConfig, Mapping, apply_mapping};
use serde_json::json;

// Load configuration
let config = PorterConfig::load_from_file("porter.config.toml")?;

// Create mapping
let mapping = Mapping {
    source: "umbraco".to_string(),
    target: "payload".to_string(),
    collection: "hotels".to_string(),
    field_mappings: vec![
        FieldMapping {
            to: "title".to_string(),
            from: json!("nodeName"),
            transforms: Vec::new(),
            fallback: None,
        }
    ],
    block_mappings: None,
};

// Apply mapping
let doc = json!({"nodeName": "Hotel Example"});
let result = apply_mapping(&doc, &mapping)?;
```

### Batch Processing

```rust
use porter::batch::{BatchConfig, create_batch_processor};

let config = BatchConfig {
    batch_size: 100,
    max_memory_mb: 512,
    enable_resume: true,
    progress_interval: 10,
    temp_dir: "./temp".to_string(),
};

let mut processor = create_batch_processor_with_config(config);
let result = processor.process_documents(
    &source_adapter,
    &target_adapter,
    &mapping,
    &source_files,
    &output_path,
    &options,
)?;
```

### Performance Optimization

```rust
use porter::performance::{PerformanceConfig, PerformanceProcessor};

let config = PerformanceConfig {
    num_threads: 8,
    memory_limit_mb: 1024,
    chunk_size: 50,
    enable_memory_monitoring: true,
    memory_monitor_interval: 5,
    enable_adaptive_chunking: true,
};

let mut processor = PerformanceProcessor::new(config);
let results = processor.process_documents_parallel(&documents, &mapping)?;
```

## Version Compatibility

### Rust Version

Porter requires Rust 1.70 or later.

### Dependencies

Key dependencies and their versions:

- `anyhow = "1"` - Error handling
- `serde = "1"` - Serialization
- `serde_json = "1"` - JSON handling
- `rayon = "1.10"` - Parallel processing
- `clap = "4"` - CLI argument parsing
- `dialoguer = "0.11"` - Interactive prompts
- `colored = "2"` - Terminal colors
- `log = "0.4"` - Logging
- `chrono = "0.4"` - Date/time handling

### Breaking Changes

- **v0.2.0**: Added nested mapping support, breaking changes to mapping API
- **v0.1.0**: Initial release

## Support

For API questions and issues:

- **Documentation**: This API reference
- **Examples**: Check the [examples directory](../examples/)
- **Issues**: [GitHub Issues](https://github.com/your-org/porter/issues)
- **Discussions**: [GitHub Discussions](https://github.com/your-org/porter/discussions)
