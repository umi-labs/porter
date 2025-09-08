use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub mod capabilities;
pub mod metadata;
pub mod source;
pub mod target;

pub use capabilities::*;
pub use metadata::*;
pub use source::*;
pub use target::*;

/// Configuration for a source adapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub adapter_type: String,
    pub connection_method: ConnectionMethod,
    pub adapter_config: Value,
}

/// Configuration for a target adapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    pub adapter_type: String,
    pub output_method: OutputMethod,
    pub adapter_config: Value,
}

/// Different methods for connecting to source data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ConnectionMethod {
    File {
        paths: Vec<String>,
        format: String,
        encoding: Option<String>,
    },
    Database {
        connection_string: String,
        query: Option<String>,
    },
    Api {
        endpoint: String,
        auth_config: Option<AuthConfig>,
    },
}

/// Different methods for outputting target data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum OutputMethod {
    SeedFiles {
        output_dir: String,
        format: String,
    },
    Database {
        connection_string: String,
        write_strategy: WriteStrategy,
    },
    Api {
        endpoint: String,
        auth_config: Option<AuthConfig>,
    },
}

/// Authentication configuration for API connections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub auth_type: AuthType,
    pub credentials: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    Bearer,
    ApiKey,
    Basic,
    OAuth2,
}

/// Strategy for writing data to targets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WriteStrategy {
    Insert,
    Upsert,
    Replace,
    Merge,
}

/// Context provided to field processors during processing
#[derive(Debug, Clone)]
pub struct ProcessingContext {
    pub source_type: String,
    pub target_type: String,
    pub item_id: String,
    pub field_name: String,
    pub collection_name: Option<String>,
    pub additional_data: HashMap<String, Value>,
}

/// Query configuration for source data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceQuery {
    pub query: Option<String>,
    pub parameters: HashMap<String, Value>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub filters: HashMap<String, Value>,
}

/// Configuration for batch operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    pub size: usize,
    pub parallel: bool,
    pub max_retries: u32,
}

/// Options for target output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetOptions {
    pub output_dir: String,
    pub collection_name: String,
    pub locale: Option<String>,
    pub batch_config: Option<BatchConfig>,
    pub validation: bool,
}
