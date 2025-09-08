# Porter Multi-Source/Target Architecture Plan

## Overview

This document outlines the comprehensive plan for transforming Porter into a truly extensible, DRY-compliant system supporting multiple sources and targets with flexible connection methods. The plan addresses current architectural limitations while maintaining backward compatibility and performance.

## Current Architecture Analysis

### Existing Strengths
- **Clean Trait Design**: Simple, focused traits for extensibility
- **Comprehensive Processing**: Both source and target implementations handle complex scenarios
- **Configuration Management**: Robust configuration system with validation
- **Batch Processing**: Sophisticated batch processing for large datasets
- **Performance Optimization**: Parallel processing and memory management
- **Interactive Experience**: Rich interactive mapping creation
- **Error Handling**: Comprehensive error handling with detailed logging

### Architectural Issues to Address

1. **Hardcoded Registration**: Built-in adapters are hardcoded in main.rs
2. **Memory Safety**: Unsafe code with potential memory leaks in plugin system
3. **Limited Configuration**: No mechanism for source/target-specific configuration
4. **Monolithic Processing**: Complex processing logic mixed with source reading
5. **Missing Async Support**: No streaming support for database connections
6. **Plugin System Issues**: Incomplete plugin registration with memory safety concerns

## Design Principles

### DRY Compliance
- **Reusable Components**: Shared field processors across adapters
- **Common Interfaces**: Consistent trait definitions with minimal duplication
- **Configuration Inheritance**: Hierarchical configuration with sensible defaults
- **Modular Architecture**: Clear separation of concerns

### Extensibility Requirements
- **Plugin Architecture**: Safe dynamic loading of external adapters
- **Multiple Connection Methods**: File, database, and API connectivity
- **Flexible Output Options**: Seed files or direct database operations
- **Schema Validation**: Runtime configuration validation

### Performance Considerations
- **Async/Await Support**: Non-blocking I/O operations
- **Streaming Capabilities**: Memory-efficient processing of large datasets
- **Parallel Processing**: Maintained and enhanced parallel execution
- **Resource Management**: Proper cleanup and connection pooling

## Enhanced Architecture Design

### Core Trait Hierarchy

#### Source Adapters
```rust
// Base trait for all source adapters
pub trait SourceAdapter: Send + Sync {
    fn metadata(&self) -> SourceMetadata;
    fn capabilities(&self) -> SourceCapabilities;
    fn init(&mut self, config: &SourceConfig) -> Result<()>;
    fn validate_config(&self, config: &SourceConfig) -> Result<()>;
    fn cleanup(&mut self) -> Result<()>;
}

// File-based source adapters
pub trait FileSourceAdapter: SourceAdapter {
    fn read_documents(&self, file_paths: &[String]) -> Result<Vec<Value>>;
    fn stream_documents(&self, file_paths: &[String]) -> Result<Box<dyn Iterator<Item = Result<Value>>>>;
}

// Database source adapters  
pub trait DatabaseSourceAdapter: SourceAdapter {
    async fn connect(&mut self, connection_string: &str) -> Result<()>;
    async fn read_documents(&self, query: &SourceQuery) -> Result<Vec<Value>>;
    async fn stream_documents(&self, query: &SourceQuery) -> Result<Pin<Box<dyn Stream<Item = Result<Value>>>>>;
}

// API-based source adapters
pub trait ApiSourceAdapter: SourceAdapter {
    async fn authenticate(&mut self, auth_config: &AuthConfig) -> Result<()>;
    async fn fetch_documents(&self, request: &ApiRequest) -> Result<Vec<Value>>;
    async fn stream_documents(&self, request: &ApiRequest) -> Result<Pin<Box<dyn Stream<Item = Result<Value>>>>>;
}
```

#### Target Adapters
```rust
// Base trait for all target adapters
pub trait TargetAdapter: Send + Sync {
    fn metadata(&self) -> TargetMetadata;
    fn capabilities(&self) -> TargetCapabilities;
    fn init(&mut self, config: &TargetConfig) -> Result<()>;
    fn validate_config(&self, config: &TargetConfig) -> Result<()>;
    fn cleanup(&mut self) -> Result<()>;
}

// File-based target adapters
pub trait FileTargetAdapter: TargetAdapter {
    fn write_documents(&self, docs: &[Value], output_config: &OutputConfig) -> Result<()>;
    fn validate_output(&self, output_path: &str) -> Result<()>;
}

// Database target adapters
pub trait DatabaseTargetAdapter: TargetAdapter {
    async fn connect(&mut self, connection_string: &str) -> Result<()>;
    async fn write_documents(&self, docs: &[Value], write_config: &WriteConfig) -> Result<()>;
    async fn upsert_documents(&self, docs: &[Value], write_config: &WriteConfig) -> Result<()>;
    async fn validate_schema(&self, collection: &str) -> Result<()>;
}

// API-based target adapters
pub trait ApiTargetAdapter: TargetAdapter {
    async fn authenticate(&mut self, auth_config: &AuthConfig) -> Result<()>;
    async fn push_documents(&self, docs: &[Value], api_config: &ApiConfig) -> Result<()>;
    async fn batch_push_documents(&self, docs: &[Value], api_config: &ApiConfig) -> Result<()>;
}
```

### Configuration System

#### Hierarchical Configuration Structure
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PorterConfig {
    pub source: SourceAdapterConfig,
    pub target: TargetAdapterConfig,
    pub collections: Vec<CollectionConfig>,
    pub global_settings: GlobalSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAdapterConfig {
    pub adapter_type: String,  // "wordpress", "umbraco", "strapi"
    pub connection_method: ConnectionMethod,
    pub adapter_config: Value, // adapter-specific configuration
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectionMethod {
    File { 
        paths: Vec<String>,
        format: FileFormat,
        encoding: Option<String>,
        compression: Option<CompressionType>,
    },
    Database { 
        connection_string: String,
        query_config: QueryConfig,
        pool_config: PoolConfig,
    },
    Api { 
        endpoint: String,
        auth_config: AuthConfig,
        rate_limit_config: RateLimitConfig,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetAdapterConfig {
    pub adapter_type: String,  // "payload", "strapi", "contentful"
    pub output_method: OutputMethod,
    pub adapter_config: Value, // adapter-specific configuration
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputMethod {
    SeedFiles { 
        output_dir: String,
        format: SeedFormat,
        template_config: TemplateConfig,
    },
    Database { 
        connection_string: String,
        write_strategy: WriteStrategy,
        batch_config: BatchConfig,
    },
    Api {
        endpoint: String,
        auth_config: AuthConfig,
        push_strategy: PushStrategy,
    },
}
```

#### Connection Method Details
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileFormat {
    JSON,
    XML,
    CSV,
    YAML,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WriteStrategy {
    Insert,        // Insert new documents only
    Upsert,        // Insert or update existing
    Replace,       // Replace existing documents
    Merge,         // Merge with existing data
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryConfig {
    pub query: String,
    pub parameters: HashMap<String, Value>,
    pub pagination: Option<PaginationConfig>,
    pub filters: Option<FilterConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub auth_type: AuthType,
    pub credentials: HashMap<String, String>,
    pub token_refresh: Option<TokenRefreshConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    Bearer,
    ApiKey,
    OAuth2,
    Basic,
    Custom(String),
}
```

### Field Processor System

#### Reusable Field Processors
```rust
pub trait FieldProcessor: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn supported_types(&self) -> Vec<FieldType>;
    fn process_field(&self, field_value: &Value, context: &ProcessingContext) -> Result<Value>;
    fn validate_field(&self, field_value: &Value) -> Result<()>;
}

// Common field processors shared across adapters
pub struct MediaFieldProcessor;
pub struct RichTextFieldProcessor;
pub struct RelationshipFieldProcessor;
pub struct CoordinateFieldProcessor;
pub struct DateTimeFieldProcessor;
pub struct LocalizedFieldProcessor;

// Adapter-specific processors
pub struct UmbracoContentFieldProcessor;
pub struct WordPressShortcodeProcessor;
pub struct PayloadUploadFieldProcessor;
```

## Implementation Phases

### Phase 1: Core Architecture Refactoring

#### Enhanced Trait System
- Implement new trait hierarchy with connection method support
- Add async/await support for database and API operations
- Create streaming interfaces for large dataset processing
- Implement proper lifecycle management (init/cleanup)

#### Configuration System Overhaul
- Design hierarchical configuration structure
- Add adapter-specific configuration schemas
- Implement runtime configuration validation
- Create configuration migration utilities

#### Plugin System Foundation
- Design safe plugin loading mechanism
- Implement plugin manifest system
- Create plugin registry with dependency management
- Establish plugin development SDK

### Phase 2: WordPress Source Implementation

#### WordPress Data Structure Support
```rust
// WordPress-specific structures
pub struct WordPressPost {
    pub id: u64,
    pub title: String,
    pub content: String,
    pub excerpt: String,
    pub status: PostStatus,
    pub post_type: String,
    pub author: u64,
    pub date: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub featured_media: Option<u64>,
    pub categories: Vec<u64>,
    pub tags: Vec<u64>,
    pub meta: HashMap<String, Value>,
    pub acf_fields: Option<Value>,
}

pub struct WordPressUser {
    pub id: u64,
    pub username: String,
    pub email: String,
    pub display_name: String,
    pub roles: Vec<String>,
    pub meta: HashMap<String, Value>,
}

pub struct WordPressTerm {
    pub id: u64,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub taxonomy: String,
    pub parent: Option<u64>,
    pub meta: HashMap<String, Value>,
}
```

#### WXR Parser Implementation
- XML parsing with proper namespace handling
- Support for WordPress Extended RSS format
- Custom field extraction and processing
- Media attachment handling
- Taxonomy relationship processing
- User and author information extraction

#### Alternative Input Methods
- JSON dump parser for cleaned WordPress exports
- MySQL database connector for direct WordPress database access
- REST API connector for WordPress.com and WordPress.org sites
- WooCommerce extension support

#### WordPress Field Processors
```rust
pub struct WordPressShortcodeProcessor;  // Process WordPress shortcodes
pub struct WordPressGalleryProcessor;    // Handle gallery shortcodes
pub struct WordPressEmbedProcessor;      // Process oEmbed content
pub struct WordPressACFProcessor;        // Advanced Custom Fields
pub struct WordPressMetaProcessor;       // Post meta fields
```

### Phase 3: Enhanced Payload Target

#### Multiple Output Methods
```rust
impl PayloadTarget {
    // Seed file generation (existing functionality)
    fn generate_seed_files(&self, docs: &[Value], config: &SeedConfig) -> Result<()> {
        // Enhanced seed generation with templates
        // Support for multiple output formats (TypeScript, JavaScript, JSON)
        // Custom template support
    }
    
    // Direct database insertion (new functionality)
    async fn insert_to_database(&self, docs: &[Value], config: &DatabaseConfig) -> Result<()> {
        // MongoDB or PostgreSQL direct insertion
        // Batch processing with transaction support
        // Schema validation before insertion
        // Relationship handling and reference resolution
    }
    
    // API-based insertion (new functionality) 
    async fn push_via_api(&self, docs: &[Value], config: &ApiConfig) -> Result<()> {
        // REST API calls to Payload CMS
        // Authentication handling
        // Rate limiting and retry logic
        // Progress tracking and error recovery
    }
}
```

#### Enhanced Field Processing
```rust
// Payload-specific field processors
pub struct PayloadUploadFieldProcessor {
    // Handle file uploads and media processing
    // Support for multiple storage providers
    // Image optimization and thumbnail generation
}

pub struct PayloadRelationshipProcessor {
    // Process relationships between collections
    // Handle bidirectional relationships
    // Reference validation and integrity checks
}

pub struct PayloadRichTextProcessor {
    // Convert from various rich text formats
    // Handle embedded media and links
    // Support for Slate.js and Lexical formats
}

pub struct PayloadLocalizationProcessor {
    // Handle multi-language content
    // Locale-specific field processing
    // Translation workflow support
}
```

### Phase 4: Plugin System Implementation

#### Safe Plugin Loading
```rust
pub struct PluginManager {
    sources: HashMap<String, Box<dyn SourceAdapter>>,
    targets: HashMap<String, Box<dyn TargetAdapter>>,
    processors: HashMap<String, Box<dyn FieldProcessor>>,
    loaded_plugins: Vec<LoadedPlugin>,
    plugin_registry: PluginRegistry,
}

impl PluginManager {
    pub fn load_plugin(&mut self, plugin_path: &Path) -> Result<()> {
        // 1. Load and validate plugin manifest
        let manifest = self.load_manifest(plugin_path)?;
        self.validate_compatibility(&manifest)?;
        
        // 2. Safely load dynamic library
        let lib = unsafe { Library::new(plugin_path)? };
        
        // 3. Get plugin entry point
        let registrar: Symbol<PluginRegistrar> = unsafe {
            lib.get(b"porter_plugin_registrar")?
        };
        
        // 4. Register plugin components
        let mut registry = AdapterRegistry::new();
        registrar(&mut registry);
        
        // 5. Validate and install components
        self.install_components(registry)?;
        
        // 6. Keep plugin loaded with proper cleanup
        self.loaded_plugins.push(LoadedPlugin::new(manifest, lib));
        
        Ok(())
    }
    
    pub fn unload_plugin(&mut self, plugin_name: &str) -> Result<()> {
        // Safe plugin unloading with resource cleanup
        // Remove adapters and processors
        // Update registry
    }
}
```

#### Plugin Development SDK
```rust
// Plugin development macro
#[macro_export]
macro_rules! porter_plugin {
    ($registrar:expr) => {
        #[no_mangle]
        pub extern "C" fn porter_plugin_registrar(registry: &mut porter::AdapterRegistry) {
            $registrar(registry);
        }
        
        #[no_mangle]
        pub extern "C" fn porter_plugin_metadata() -> porter::PluginManifest {
            porter::PluginManifest::from_cargo_toml()
        }
    };
}

// Example plugin implementation
porter_plugin!(|registry: &mut AdapterRegistry| {
    registry.register_source("my_cms", Box::new(MyCmsSource::new()));
    registry.register_target("my_target", Box::new(MyTarget::new()));
    registry.register_processor("my_processor", Box::new(MyFieldProcessor::new()));
});
```

#### Plugin Manifest System
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub license: String,
    pub porter_version_requirement: String,
    pub adapters: Vec<AdapterInfo>,
    pub processors: Vec<ProcessorInfo>,
    pub dependencies: Vec<PluginDependency>,
    pub configuration_schema: Option<JsonSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterInfo {
    pub name: String,
    pub adapter_type: AdapterType,
    pub description: String,
    pub supported_formats: Vec<String>,
    pub capabilities: Vec<String>,
}
```

### Phase 5: Configuration Schema System

#### Dynamic Configuration Validation
```rust
pub trait ConfigSchema {
    fn get_schema(&self) -> JsonSchema;
    fn validate(&self, config: &Value) -> Result<()>;
    fn get_default_config(&self) -> Value;
    fn get_config_documentation(&self) -> ConfigDocumentation;
}

// Auto-generation of configuration schemas
impl ConfigSchema for WordPressSource {
    fn get_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "format": {
                    "type": "string",
                    "enum": ["wxr", "json", "database"],
                    "description": "WordPress export format"
                },
                "custom_field_mapping": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "description": "Mapping for custom fields"
                },
                "content_types": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "WordPress post types to include"
                },
                "include_media": {
                    "type": "boolean",
                    "default": true,
                    "description": "Include media attachments"
                }
            },
            "required": ["format"]
        }).into()
    }
}
```

#### Configuration UI Generation
```rust
pub struct ConfigurationUI {
    schema_registry: HashMap<String, Box<dyn ConfigSchema>>,
}

impl ConfigurationUI {
    pub fn generate_interactive_config(&self, adapter_type: &str) -> Result<Value> {
        let schema = self.schema_registry.get(adapter_type)
            .ok_or_else(|| anyhow!("Unknown adapter type: {}", adapter_type))?;
            
        // Generate interactive prompts based on schema
        self.prompt_for_configuration(schema.get_schema())
    }
}
```

## Migration Strategy

### Backward Compatibility
- Existing configurations continue to work
- Legacy adapters remain functional during transition
- Gradual migration path for users
- Clear deprecation warnings and migration guides

### Migration Tools
```rust
pub struct ConfigurationMigrator {
    migrations: Vec<Box<dyn ConfigMigration>>,
}

pub trait ConfigMigration {
    fn from_version(&self) -> String;
    fn to_version(&self) -> String;
    fn migrate(&self, old_config: Value) -> Result<Value>;
}
```

### Testing Strategy
- Comprehensive unit tests for all components
- Integration tests with real data sources
- Plugin compatibility testing
- Performance regression testing
- Memory safety validation

## Example Configurations

### WordPress WXR to Payload Seeds
```toml
[source]
adapter_type = "wordpress"
connection_method = { type = "file", paths = ["./wp-export.xml"], format = "wxr" }

[source.adapter_config]
format = "wxr"
content_types = ["posts", "pages", "media"]
include_acf_fields = true
custom_field_mapping = { "_featured_image" = "featured_media" }

[target]
adapter_type = "payload"
output_method = { type = "seed_files", output_dir = "./seed", format = "typescript" }

[target.adapter_config]
collection_prefix = ""
locale_handling = "separate_files"
media_processing = { resize_images = true, generate_thumbnails = true }

[[collections]]
name = "posts"
source_query = { post_type = "post", status = "publish" }
```

### WordPress Database to Payload API
```toml
[source]
adapter_type = "wordpress"
connection_method = { type = "database", connection_string = "mysql://user:pass@localhost/wordpress" }

[source.adapter_config]
query_config = { 
    batch_size = 100,
    offset = 0,
    custom_query = "SELECT * FROM wp_posts WHERE post_status = 'publish'"
}

[target]
adapter_type = "payload"
output_method = { 
    type = "api", 
    endpoint = "http://localhost:3000/api",
    auth_config = { type = "bearer", token = "${PAYLOAD_TOKEN}" }
}

[target.adapter_config]
write_strategy = "upsert"
batch_size = 50
validate_before_push = true
```

### Umbraco to Multiple Targets
```toml
[source]
adapter_type = "umbraco"
connection_method = { type = "file", paths = ["./umbraco-export.json"], format = "json" }

[[targets]]
adapter_type = "payload"
output_method = { type = "seed_files", output_dir = "./payload-seeds" }

[[targets]]
adapter_type = "strapi"  # Future implementation
output_method = { type = "api", endpoint = "http://localhost:1337/api" }
```

## Performance Considerations

### Streaming and Memory Management
- Stream processing for large datasets
- Configurable batch sizes
- Memory usage monitoring
- Garbage collection optimization

### Async Processing
- Non-blocking I/O operations
- Connection pooling for databases
- Parallel processing with configurable concurrency
- Progress tracking and cancellation support

### Caching Strategy
- Schema caching to avoid repeated parsing
- Field processor caching
- Connection pooling
- Intelligent configuration caching

## Security Considerations

### Plugin Security
- Plugin signature verification
- Sandboxed plugin execution
- Resource usage limits
- Security scanning for plugins

### Data Protection
- Secure credential storage
- Encryption for sensitive configuration
- Audit logging for data access
- GDPR compliance features

### Network Security
- TLS/SSL for all network communications
- Certificate validation
- Rate limiting and DDoS protection
- API key management

## Testing and Quality Assurance

### Test Coverage Requirements
- Unit tests for all adapters and processors
- Integration tests with real data sources
- Plugin compatibility testing
- Performance benchmarking
- Security vulnerability scanning

### Continuous Integration
- Automated testing on multiple platforms
- Plugin compatibility validation
- Performance regression detection
- Security scanning in CI/CD pipeline

This architecture plan provides a comprehensive foundation for making Porter truly extensible while maintaining its current strengths and ensuring a smooth migration path for existing users.