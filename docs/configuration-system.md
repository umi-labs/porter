# Porter Configuration System

## Overview

The Porter configuration system provides flexible, hierarchical configuration management with adapter-specific schemas, runtime validation, and support for multiple connection methods. This system enables users to configure complex migration scenarios with type safety and validation.

## Configuration Architecture

### Hierarchical Structure

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PorterConfig {
    pub version: String,
    pub source: SourceAdapterConfig,
    pub target: TargetAdapterConfig,
    pub collections: Vec<CollectionConfig>,
    pub global_settings: GlobalSettings,
    pub plugin_configs: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAdapterConfig {
    pub adapter_type: String,  // "wordpress", "umbraco", "strapi"
    pub connection_method: ConnectionMethod,
    pub adapter_config: Value, // adapter-specific configuration
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetAdapterConfig {
    pub adapter_type: String,  // "payload", "strapi", "contentful"
    pub output_method: OutputMethod,
    pub adapter_config: Value, // adapter-specific configuration
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    pub name: String,
    pub source_query: Option<SourceQuery>,
    pub target_collection: Option<String>,
    pub field_mappings: Option<PathBuf>,
    pub transforms: Vec<TransformConfig>,
    pub validation_rules: Vec<ValidationRule>,
    pub locale: Option<String>,
    pub related_collections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalSettings {
    pub batch_size: Option<usize>,
    pub parallel_processing: bool,
    pub max_workers: Option<usize>,
    pub retry_attempts: u32,
    pub retry_delay: Duration,
    pub progress_reporting: ProgressConfig,
    pub logging: LoggingConfig,
    pub temp_directory: Option<PathBuf>,
    pub plugin_directory: Option<PathBuf>,
}
```

### Connection Methods

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ConnectionMethod {
    File { 
        paths: Vec<PathBuf>,
        format: FileFormat,
        encoding: Option<String>,
        compression: Option<CompressionType>,
        watch_for_changes: bool,
    },
    Database { 
        connection_string: String,
        query_config: QueryConfig,
        pool_config: PoolConfig,
        ssl_config: Option<SslConfig>,
        transaction_config: TransactionConfig,
    },
    Api { 
        endpoint: String,
        auth_config: AuthConfig,
        request_config: RequestConfig,
        rate_limit_config: RateLimitConfig,
        retry_config: RetryConfig,
    },
    Stream {
        stream_config: StreamConfig,
        buffer_config: BufferConfig,
        checkpoint_config: CheckpointConfig,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum OutputMethod {
    SeedFiles { 
        output_dir: PathBuf,
        format: SeedFormat,
        template_config: TemplateConfig,
        file_organization: FileOrganization,
        cleanup_existing: bool,
    },
    Database { 
        connection_string: String,
        write_strategy: WriteStrategy,
        batch_config: BatchConfig,
        transaction_config: TransactionConfig,
        conflict_resolution: ConflictResolution,
    },
    Api {
        endpoint: String,
        auth_config: AuthConfig,
        push_strategy: PushStrategy,
        request_config: RequestConfig,
        validation_config: ValidationConfig,
    },
    Stream {
        stream_config: StreamConfig,
        serialization_config: SerializationConfig,
        delivery_guarantee: DeliveryGuarantee,
    },
}
```

### Detailed Configuration Types

#### File Configuration
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileFormat {
    JSON,
    XML,
    CSV { delimiter: char, quote: char, escape: Option<char> },
    YAML,
    TOML,
    Parquet,
    Avro,
    Custom { format_name: String, config: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Gzip,
    Bzip2,
    Lz4,
    Zstd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOrganization {
    pub strategy: OrganizationStrategy,
    pub max_files_per_directory: Option<usize>,
    pub filename_template: String,
    pub directory_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrganizationStrategy {
    Flat,           // All files in output directory
    ByCollection,   // Files organized by collection
    ByDate,         // Files organized by date
    BySize,         // Files organized by size limits
    Custom(String), // Custom organization pattern
}
```

#### Database Configuration
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryConfig {
    pub query: String,
    pub parameters: HashMap<String, Value>,
    pub pagination: Option<PaginationConfig>,
    pub filters: Option<FilterConfig>,
    pub ordering: Option<OrderingConfig>,
    pub joins: Vec<JoinConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub min_connections: u32,
    pub max_connections: u32,
    pub connection_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
    pub acquire_timeout: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SslConfig {
    pub mode: SslMode,
    pub ca_cert_path: Option<PathBuf>,
    pub client_cert_path: Option<PathBuf>,
    pub client_key_path: Option<PathBuf>,
    pub verify_hostname: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SslMode {
    Disable,
    Allow,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionConfig {
    pub isolation_level: IsolationLevel,
    pub timeout: Duration,
    pub read_only: bool,
    pub deferrable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WriteStrategy {
    Insert,        // Insert new documents only
    Upsert,        // Insert or update existing documents
    Replace,       // Replace existing documents completely
    Merge,         // Merge with existing data
    BulkInsert,    // Optimized bulk insert
    Stream,        // Streaming insert
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    pub size: usize,
    pub timeout: Duration,
    pub flush_interval: Duration,
    pub max_retries: u32,
    pub parallel_batches: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictResolution {
    Error,         // Throw error on conflict
    Skip,          // Skip conflicting records
    Overwrite,     // Overwrite existing records
    Merge,         // Merge conflicting records
    Custom(String), // Custom conflict resolution
}
```

#### API Configuration
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub auth_type: AuthType,
    pub credentials: HashMap<String, String>,
    pub token_refresh: Option<TokenRefreshConfig>,
    pub scopes: Vec<String>,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    None,
    Bearer { token: String },
    ApiKey { key: String, header: String },
    Basic { username: String, password: String },
    OAuth2 { config: OAuth2Config },
    JWT { config: JWTConfig },
    Custom { config: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Config {
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestConfig {
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub max_redirects: u32,
    pub user_agent: String,
    pub default_headers: HashMap<String, String>,
    pub proxy: Option<ProxyConfig>,
    pub tls_config: Option<TlsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub requests_per_second: f64,
    pub burst_size: u32,
    pub backoff_strategy: BackoffStrategy,
    pub respect_retry_after: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackoffStrategy {
    Fixed { delay: Duration },
    Linear { initial_delay: Duration, increment: Duration },
    Exponential { initial_delay: Duration, multiplier: f64, max_delay: Duration },
    Custom { strategy: String, config: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PushStrategy {
    SingleRequests,    // One request per document
    Batch { size: usize }, // Batch multiple documents
    Streaming,         // Streaming push
    BulkApi,          // Use bulk API if available
}
```

#### Stream Configuration
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    pub stream_type: StreamType,
    pub topic: String,
    pub partition_config: PartitionConfig,
    pub serialization: SerializationFormat,
    pub compression: CompressionType,
    pub connection_config: StreamConnectionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamType {
    Kafka { config: KafkaConfig },
    Redis { config: RedisStreamConfig },
    RabbitMQ { config: RabbitMQConfig },
    AmazonKinesis { config: KinesisConfig },
    GooglePubSub { config: PubSubConfig },
    Custom { config: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeliveryGuarantee {
    AtMostOnce,    // Fire and forget
    AtLeastOnce,   // Guaranteed delivery with possible duplicates
    ExactlyOnce,   // Guaranteed delivery without duplicates
}
```

### Transform Configuration
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformConfig {
    pub name: String,
    pub transform_type: TransformType,
    pub field_path: String,
    pub target_path: Option<String>,
    pub config: Value,
    pub condition: Option<ConditionConfig>,
    pub error_handling: ErrorHandling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransformType {
    // Data type transforms
    StringTransform(StringTransformType),
    NumberTransform(NumberTransformType),
    DateTransform(DateTransformType),
    BooleanTransform(BooleanTransformType),
    
    // Structure transforms
    ArrayTransform(ArrayTransformType),
    ObjectTransform(ObjectTransformType),
    
    // Content transforms
    RichTextTransform(RichTextTransformType),
    MediaTransform(MediaTransformType),
    
    // Relationship transforms
    RelationshipTransform(RelationshipTransformType),
    
    // Custom transforms
    CustomTransform { name: String, config: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StringTransformType {
    Trim,
    Uppercase,
    Lowercase,
    TitleCase,
    Slugify,
    Sanitize,
    Regex { pattern: String, replacement: String },
    Template { template: String },
    Split { delimiter: String, index: Option<usize> },
    Join { delimiter: String },
    Truncate { max_length: usize, suffix: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionConfig {
    pub condition_type: ConditionType,
    pub field_path: Option<String>,
    pub value: Value,
    pub operator: ComparisonOperator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConditionType {
    FieldValue,
    FieldExists,
    FieldType,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComparisonOperator {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Contains,
    StartsWith,
    EndsWith,
    Regex(String),
    In(Vec<Value>),
    NotIn(Vec<Value>),
}
```

## Configuration Management

### Configuration Loader

```rust
pub struct ConfigurationManager {
    config_parsers: HashMap<String, Box<dyn ConfigParser>>,
    schema_registry: SchemaRegistry,
    validator: ConfigValidator,
    environment_resolver: EnvironmentResolver,
    secret_manager: SecretManager,
}

impl ConfigurationManager {
    pub fn load_config<P: AsRef<Path>>(&self, config_path: P) -> Result<PorterConfig> {
        let config_path = config_path.as_ref();
        
        // Determine config format from extension
        let format = self.detect_config_format(config_path)?;
        
        // Parse raw configuration
        let parser = self.config_parsers.get(&format)
            .ok_or_else(|| anyhow!("Unsupported config format: {}", format))?;
        let mut raw_config = parser.parse_file(config_path)?;
        
        // Resolve environment variables and secrets
        self.resolve_environment_variables(&mut raw_config)?;
        self.resolve_secrets(&mut raw_config)?;
        
        // Validate configuration
        self.validate_configuration(&raw_config)?;
        
        // Convert to typed configuration
        let config: PorterConfig = serde_json::from_value(raw_config)?;
        
        // Additional validation
        self.validate_typed_configuration(&config)?;
        
        Ok(config)
    }
    
    pub fn save_config<P: AsRef<Path>>(&self, config: &PorterConfig, config_path: P) -> Result<()> {
        let config_path = config_path.as_ref();
        let format = self.detect_config_format(config_path)?;
        
        // Convert to raw value
        let mut raw_config = serde_json::to_value(config)?;
        
        // Mask sensitive values
        self.mask_sensitive_values(&mut raw_config)?;
        
        // Format and save
        let parser = self.config_parsers.get(&format)
            .ok_or_else(|| anyhow!("Unsupported config format: {}", format))?;
        parser.save_file(&raw_config, config_path)?;
        
        Ok(())
    }
    
    pub fn validate_configuration(&self, config: &Value) -> Result<ValidationResult> {
        // Validate against schema
        let schema = self.schema_registry.get_root_schema()?;
        let validation_result = self.validator.validate(config, &schema)?;
        
        if !validation_result.is_valid() {
            return Err(anyhow!("Configuration validation failed: {:?}", validation_result.errors));
        }
        
        Ok(validation_result)
    }
    
    fn resolve_environment_variables(&self, config: &mut Value) -> Result<()> {
        self.environment_resolver.resolve_recursive(config)
    }
    
    fn resolve_secrets(&self, config: &mut Value) -> Result<()> {
        self.secret_manager.resolve_recursive(config)
    }
}
```

### Environment Variable Resolution

```rust
pub struct EnvironmentResolver {
    variable_pattern: Regex,
    default_value_pattern: Regex,
}

impl EnvironmentResolver {
    pub fn new() -> Result<Self> {
        Ok(Self {
            variable_pattern: Regex::new(r"\$\{([A-Z_][A-Z0-9_]*)\}")?,
            default_value_pattern: Regex::new(r"\$\{([A-Z_][A-Z0-9_]*):([^}]+)\}")?,
        })
    }
    
    pub fn resolve_recursive(&self, value: &mut Value) -> Result<()> {
        match value {
            Value::String(s) => {
                *s = self.resolve_string(s)?;
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.resolve_recursive(item)?;
                }
            }
            Value::Object(obj) => {
                for (_, v) in obj.iter_mut() {
                    self.resolve_recursive(v)?;
                }
            }
            _ => {}
        }
        
        Ok(())
    }
    
    fn resolve_string(&self, s: &str) -> Result<String> {
        let mut result = s.to_string();
        
        // Handle variables with default values
        for caps in self.default_value_pattern.captures_iter(s) {
            let full_match = caps.get(0).unwrap().as_str();
            let var_name = caps.get(1).unwrap().as_str();
            let default_value = caps.get(2).unwrap().as_str();
            
            let value = env::var(var_name).unwrap_or_else(|_| default_value.to_string());
            result = result.replace(full_match, &value);
        }
        
        // Handle variables without default values
        for caps in self.variable_pattern.captures_iter(s) {
            let full_match = caps.get(0).unwrap().as_str();
            let var_name = caps.get(1).unwrap().as_str();
            
            if !self.default_value_pattern.is_match(full_match) {
                let value = env::var(var_name)
                    .map_err(|_| anyhow!("Environment variable '{}' not found", var_name))?;
                result = result.replace(full_match, &value);
            }
        }
        
        Ok(result)
    }
}
```

### Secret Management

```rust
pub struct SecretManager {
    secret_pattern: Regex,
    vault_client: Option<VaultClient>,
    aws_client: Option<AwsSecretsClient>,
    azure_client: Option<AzureKeyVaultClient>,
}

impl SecretManager {
    pub fn resolve_recursive(&self, value: &mut Value) -> Result<()> {
        match value {
            Value::String(s) => {
                if self.secret_pattern.is_match(s) {
                    *s = self.resolve_secret(s)?;
                }
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.resolve_recursive(item)?;
                }
            }
            Value::Object(obj) => {
                for (_, v) in obj.iter_mut() {
                    self.resolve_recursive(v)?;
                }
            }
            _ => {}
        }
        
        Ok(())
    }
    
    fn resolve_secret(&self, secret_ref: &str) -> Result<String> {
        if secret_ref.starts_with("vault://") {
            self.resolve_vault_secret(secret_ref)
        } else if secret_ref.starts_with("aws://") {
            self.resolve_aws_secret(secret_ref)
        } else if secret_ref.starts_with("azure://") {
            self.resolve_azure_secret(secret_ref)
        } else if secret_ref.starts_with("file://") {
            self.resolve_file_secret(secret_ref)
        } else {
            Err(anyhow!("Unsupported secret reference format: {}", secret_ref))
        }
    }
    
    fn resolve_vault_secret(&self, secret_ref: &str) -> Result<String> {
        let vault_client = self.vault_client.as_ref()
            .ok_or_else(|| anyhow!("Vault client not configured"))?;
            
        // Parse vault://path/to/secret:field
        let uri = secret_ref.strip_prefix("vault://").unwrap();
        let (path, field) = uri.split_once(':')
            .ok_or_else(|| anyhow!("Invalid vault secret format"))?;
            
        vault_client.read_secret(path, field)
    }
}
```

## Schema-Based Configuration

### Dynamic Schema Registry

```rust
pub struct SchemaRegistry {
    root_schema: JsonSchema,
    adapter_schemas: HashMap<String, JsonSchema>,
    plugin_schemas: HashMap<String, JsonSchema>,
    validation_cache: HashMap<String, ValidationResult>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        let root_schema = Self::build_root_schema();
        
        Self {
            root_schema,
            adapter_schemas: HashMap::new(),
            plugin_schemas: HashMap::new(),
            validation_cache: HashMap::new(),
        }
    }
    
    pub fn register_adapter_schema(&mut self, adapter_name: &str, schema: JsonSchema) -> Result<()> {
        // Validate schema format
        self.validate_schema_format(&schema)?;
        
        // Register schema
        self.adapter_schemas.insert(adapter_name.to_string(), schema);
        
        // Clear validation cache
        self.validation_cache.clear();
        
        Ok(())
    }
    
    pub fn get_adapter_schema(&self, adapter_name: &str) -> Option<&JsonSchema> {
        self.adapter_schemas.get(adapter_name)
    }
    
    pub fn get_combined_schema(&self, adapter_type: &str) -> Result<JsonSchema> {
        let base_schema = self.root_schema.clone();
        
        if let Some(adapter_schema) = self.adapter_schemas.get(adapter_type) {
            // Merge schemas
            self.merge_schemas(base_schema, adapter_schema.clone())
        } else {
            Ok(base_schema)
        }
    }
    
    fn build_root_schema() -> JsonSchema {
        json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {
                "version": {
                    "type": "string",
                    "pattern": "^\\d+\\.\\d+\\.\\d+$"
                },
                "source": {
                    "$ref": "#/definitions/SourceAdapterConfig"
                },
                "target": {
                    "$ref": "#/definitions/TargetAdapterConfig"
                },
                "collections": {
                    "type": "array",
                    "items": {
                        "$ref": "#/definitions/CollectionConfig"
                    }
                },
                "global_settings": {
                    "$ref": "#/definitions/GlobalSettings"
                },
                "plugin_configs": {
                    "type": "object",
                    "additionalProperties": true
                }
            },
            "required": ["version", "source", "target"],
            "definitions": {
                "SourceAdapterConfig": {
                    "type": "object",
                    "properties": {
                        "adapter_type": {
                            "type": "string"
                        },
                        "connection_method": {
                            "$ref": "#/definitions/ConnectionMethod"
                        },
                        "adapter_config": {
                            "type": "object",
                            "additionalProperties": true
                        }
                    },
                    "required": ["adapter_type", "connection_method"]
                },
                "ConnectionMethod": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "file" },
                                "config": {
                                    "type": "object",
                                    "properties": {
                                        "paths": {
                                            "type": "array",
                                            "items": { "type": "string" }
                                        },
                                        "format": { "type": "string" },
                                        "encoding": { "type": "string" },
                                        "compression": { "type": "string" }
                                    },
                                    "required": ["paths", "format"]
                                }
                            },
                            "required": ["type", "config"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "database" },
                                "config": {
                                    "type": "object",
                                    "properties": {
                                        "connection_string": { "type": "string" },
                                        "query_config": { "type": "object" },
                                        "pool_config": { "type": "object" }
                                    },
                                    "required": ["connection_string"]
                                }
                            },
                            "required": ["type", "config"]
                        }
                    ]
                }
            }
        }).into()
    }
}
```

### Configuration Validation

```rust
pub struct ConfigValidator {
    json_validator: JSONSchemaValidator,
    custom_validators: HashMap<String, Box<dyn CustomValidator>>,
}

impl ConfigValidator {
    pub fn validate(&self, config: &Value, schema: &JsonSchema) -> Result<ValidationResult> {
        let mut result = ValidationResult::new();
        
        // JSON Schema validation
        let json_result = self.json_validator.validate(config, schema)?;
        result.merge(json_result);
        
        // Custom validations
        for (name, validator) in &self.custom_validators {
            let custom_result = validator.validate(config)?;
            result.add_custom_result(name, custom_result);
        }
        
        Ok(result)
    }
    
    pub fn register_custom_validator<T>(&mut self, name: &str, validator: T) 
    where 
        T: CustomValidator + 'static 
    {
        self.custom_validators.insert(name.to_string(), Box::new(validator));
    }
}

pub trait CustomValidator: Send + Sync {
    fn validate(&self, config: &Value) -> Result<CustomValidationResult>;
}

// Example custom validators
pub struct ConnectionStringValidator;

impl CustomValidator for ConnectionStringValidator {
    fn validate(&self, config: &Value) -> Result<CustomValidationResult> {
        let mut result = CustomValidationResult::new();
        
        // Validate database connection strings
        if let Some(source) = config.get("source") {
            if let Some(connection_method) = source.get("connection_method") {
                if connection_method.get("type").and_then(|t| t.as_str()) == Some("database") {
                    if let Some(connection_string) = connection_method
                        .get("config")
                        .and_then(|c| c.get("connection_string"))
                        .and_then(|cs| cs.as_str()) 
                    {
                        if !self.is_valid_connection_string(connection_string) {
                            result.add_error("Invalid database connection string format");
                        }
                    }
                }
            }
        }
        
        Ok(result)
    }
}

pub struct PathValidator;

impl CustomValidator for PathValidator {
    fn validate(&self, config: &Value) -> Result<CustomValidationResult> {
        let mut result = CustomValidationResult::new();
        
        // Validate file paths exist and are accessible
        self.validate_paths_recursive(config, &mut result, "");
        
        Ok(result)
    }
}
```

## Configuration Examples

### Complete WordPress to Payload Configuration

```toml
version = "2.0.0"

[source]
adapter_type = "wordpress"

[source.connection_method]
type = "file"

[source.connection_method.config]
paths = ["./data/wordpress-export.xml"]
format = "wxr"
encoding = "utf-8"
watch_for_changes = false

[source.adapter_config]
content_types = ["post", "page", "product", "event"]
include_drafts = false
include_private = false
include_media = true
include_comments = false
include_users = true
include_taxonomies = true
include_acf_fields = true
shortcode_processing = true

[source.adapter_config.field_mappings]
"_yoast_wpseo_title" = "seo_title"
"_yoast_wpseo_metadesc" = "seo_description"
"_featured_image" = "featured_media"

[source.adapter_config.media_processing_config]
download_media = true
media_base_path = "./media"
resize_images = true
max_image_width = 1920
max_image_height = 1080

[target]
adapter_type = "payload"

[target.output_method]
type = "database"

[target.output_method.config]
connection_string = "${DATABASE_URL}"
write_strategy = "upsert"
conflict_resolution = "merge"

[target.output_method.config.batch_config]
size = 50
timeout = "30s"
flush_interval = "5s"
max_retries = 3
parallel_batches = true

[target.adapter_config]
locale_handling = "separate_collections"
media_processing = { resize_images = true, generate_thumbnails = true }
relationship_handling = "bidirectional"
validate_before_push = true

[global_settings]
batch_size = 100
parallel_processing = true
max_workers = 4
retry_attempts = 3
retry_delay = "2s"

[global_settings.progress_reporting]
enabled = true
update_interval = "1s"
show_collection_progress = true
show_field_progress = false

[global_settings.logging]
level = "info"
output = "file"
file_path = "./porter.log"
rotation = "daily"
max_files = 7

[[collections]]
name = "posts"
target_collection = "posts"
locale = "en"

[[collections.transforms]]
name = "featured_image"
transform_type = { MediaTransform = "convert_to_upload" }
field_path = "featured_media"
target_path = "featuredImage"

[[collections.transforms]]
name = "content_cleanup"
transform_type = { RichTextTransform = "clean_html" }
field_path = "content"
target_path = "content"

[collections.transforms.config]
allowed_tags = ["p", "br", "strong", "em", "ul", "ol", "li", "a", "img"]
remove_empty_paragraphs = true
convert_shortcodes = true

[[collections]]
name = "pages"
target_collection = "pages"
locale = "en"

[collections.source_query]
post_type = "page"
status = "publish"
limit = 1000

# Plugin-specific configurations
[plugin_configs.wordpress_seo]
import_meta_descriptions = true
import_canonical_urls = true
import_social_metadata = true

[plugin_configs.acf_processor]
field_group_mapping = { "hero_section" = "hero", "page_builder" = "content_blocks" }
convert_repeaters_to_arrays = true
```

### Multi-Environment Configuration

```toml
# Base configuration
version = "2.0.0"

# Environment-specific overrides
[environments.development]
[environments.development.global_settings]
batch_size = 10
parallel_processing = false
max_workers = 1

[environments.development.global_settings.logging]
level = "debug"
output = "console"

[environments.production]
[environments.production.global_settings]
batch_size = 1000
parallel_processing = true
max_workers = 8

[environments.production.global_settings.logging]
level = "warn"
output = "file"
file_path = "/var/log/porter/porter.log"

[environments.production.target.output_method.config]
connection_string = "${PRODUCTION_DATABASE_URL}"

[environments.production.target.output_method.config.batch_config]
size = 200
parallel_batches = true

# Secrets configuration
[secrets]
database_credentials = "vault://secret/porter/database"
api_tokens = "aws://secretsmanager/porter/api-tokens"
encryption_keys = "azure://keyvault/porter/encryption"
```

### API-Based Configuration

```toml
[source]
adapter_type = "wordpress"

[source.connection_method]
type = "api"

[source.connection_method.config]
endpoint = "https://wordpress-site.com/wp-json/wp/v2"

[source.connection_method.config.auth_config]
auth_type = { Bearer = { token = "${WP_API_TOKEN}" } }
scopes = ["read"]

[source.connection_method.config.request_config]
timeout = "30s"
connect_timeout = "10s"
max_redirects = 3
user_agent = "Porter/2.0.0"

[source.connection_method.config.rate_limit_config]
requests_per_second = 10.0
burst_size = 20
respect_retry_after = true

[source.connection_method.config.rate_limit_config.backoff_strategy]
type = "exponential"
initial_delay = "1s"
multiplier = 2.0
max_delay = "60s"

[target]
adapter_type = "payload"

[target.output_method]
type = "api"

[target.output_method.config]
endpoint = "https://payload-cms.com/api"

[target.output_method.config.auth_config]
auth_type = { Bearer = { token = "${PAYLOAD_API_TOKEN}" } }

[target.output_method.config.push_strategy]
type = "batch"
size = 25

[target.output_method.config.validation_config]
validate_schema = true
validate_relationships = true
fail_on_validation_error = true
```

This comprehensive configuration system provides the flexibility and type safety needed to handle complex migration scenarios while maintaining ease of use and clear validation.