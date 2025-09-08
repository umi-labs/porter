use anyhow::Result;
use futures::Stream;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::adapters::capabilities::SourceCapabilities;
use crate::adapters::metadata::SourceMetadata;
use crate::adapters::{ProcessingContext, SourceConfig, SourceQuery};

/// Base trait for all source adapters
///
/// All source adapters must implement this trait to provide metadata,
/// initialization, validation, and cleanup capabilities.
pub trait SourceAdapter: Send + Sync {
    /// Get metadata about this source adapter
    fn metadata(&self) -> SourceMetadata;

    /// Get capabilities of this source adapter
    fn capabilities(&self) -> SourceCapabilities;

    /// Initialize the source adapter with configuration
    fn init(&mut self, config: &SourceConfig) -> Result<()>;

    /// Validate the provided configuration
    fn validate_config(&self, config: &SourceConfig) -> Result<()>;

    /// Cleanup resources when done
    fn cleanup(&mut self) -> Result<()>;
}

/// Trait for source adapters that read from files
pub trait FileSourceAdapter: SourceAdapter {
    /// Read all documents from the provided file paths
    fn read_documents(&self, file_paths: &[String]) -> Result<Vec<Value>>;

    /// Stream documents from the provided file paths for memory efficiency
    fn stream_documents(
        &self,
        file_paths: &[String],
    ) -> Result<Box<dyn Iterator<Item = Result<Value>>>>;

    /// Get supported file formats
    fn supported_formats(&self) -> Vec<String>;

    /// Validate file format and accessibility
    fn validate_files(&self, file_paths: &[String]) -> Result<()>;
}

/// Trait for source adapters that connect to databases
pub trait DatabaseSourceAdapter: SourceAdapter {
    /// Connect to the database
    fn connect(
        &mut self,
        connection_string: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Read documents based on query configuration
    fn read_documents(
        &self,
        query: &SourceQuery,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Value>>> + Send + '_>>;

    /// Stream documents for memory efficiency
    fn stream_documents(
        &self,
        query: &SourceQuery,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Pin<Box<dyn Stream<Item = Result<Value>> + Send>>>>
                + Send
                + '_,
        >,
    >;

    /// Test database connection
    fn test_connection(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Get database schema information
    fn get_schema(&self) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;
}

/// Trait for source adapters that connect to APIs
pub trait ApiSourceAdapter: SourceAdapter {
    /// Authenticate with the API
    fn authenticate(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Fetch documents from the API
    fn fetch_documents(
        &self,
        query: &SourceQuery,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Value>>> + Send + '_>>;

    /// Stream documents from the API
    fn stream_documents(
        &self,
        query: &SourceQuery,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Pin<Box<dyn Stream<Item = Result<Value>> + Send>>>>
                + Send
                + '_,
        >,
    >;

    /// Test API connectivity and authentication
    fn test_connection(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Get API schema or endpoint information
    fn get_api_info(&self) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;
}

/// Trait for processing and transforming fields from source data
pub trait FieldProcessor: Send + Sync {
    /// Get the name of this field processor
    fn name(&self) -> &str;

    /// Get a description of what this processor does
    fn description(&self) -> &str;

    /// Get the field types this processor can handle
    fn supported_types(&self) -> Vec<String>;

    /// Process a field value with the given context
    fn process_field(&self, field_value: &Value, context: &ProcessingContext) -> Result<Value>;

    /// Validate that a field value can be processed
    fn validate_field(&self, field_value: &Value) -> Result<()>;

    /// Get configuration schema for this processor
    fn get_config_schema(&self) -> Option<Value> {
        None
    }
}

/// Registry for managing field processors
pub struct FieldProcessorRegistry {
    processors: std::collections::HashMap<String, Box<dyn FieldProcessor>>,
}

impl FieldProcessorRegistry {
    pub fn new() -> Self {
        Self {
            processors: std::collections::HashMap::new(),
        }
    }

    pub fn register<T>(&mut self, name: &str, processor: T)
    where
        T: FieldProcessor + 'static,
    {
        self.processors
            .insert(name.to_string(), Box::new(processor));
    }

    pub fn get(&self, name: &str) -> Option<&dyn FieldProcessor> {
        self.processors.get(name).map(|p| p.as_ref())
    }

    pub fn list_processors(&self) -> Vec<&str> {
        self.processors.keys().map(|s| s.as_str()).collect()
    }

    pub fn get_processors_for_type(&self, field_type: &str) -> Vec<&dyn FieldProcessor> {
        self.processors
            .values()
            .filter(|p| p.supported_types().contains(&field_type.to_string()))
            .map(|p| p.as_ref())
            .collect()
    }
}

impl Default for FieldProcessorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
