use anyhow::Result;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::adapters::capabilities::TargetCapabilities;
use crate::adapters::metadata::TargetMetadata;
use crate::adapters::{BatchConfig, TargetConfig, TargetOptions, WriteStrategy};

/// Base trait for all target adapters
///
/// All target adapters must implement this trait to provide metadata,
/// initialization, validation, and cleanup capabilities.
pub trait TargetAdapter: Send + Sync {
    /// Get metadata about this target adapter
    fn metadata(&self) -> TargetMetadata;

    /// Get capabilities of this target adapter
    fn capabilities(&self) -> TargetCapabilities;

    /// Initialize the target adapter with configuration
    fn init(&mut self, config: &TargetConfig) -> Result<()>;

    /// Validate the provided configuration
    fn validate_config(&self, config: &TargetConfig) -> Result<()>;

    /// Cleanup resources when done
    fn cleanup(&mut self) -> Result<()>;

    /// Validate documents before writing
    fn validate_documents(&self, docs: &[Value]) -> Result<()>;
}

/// Trait for target adapters that write to files (seed files, exports)
pub trait FileTargetAdapter: TargetAdapter {
    /// Write documents to files
    fn write_documents(&self, docs: &[Value], options: &TargetOptions) -> Result<()>;

    /// Write documents in batches for large datasets
    fn write_documents_batch(
        &self,
        docs: &[Value],
        options: &TargetOptions,
        batch_config: &BatchConfig,
    ) -> Result<()>;

    /// Get supported output formats
    fn supported_formats(&self) -> Vec<String>;

    /// Validate output directory and permissions
    fn validate_output_path(&self, output_path: &str) -> Result<()>;

    /// Generate preview of output without writing
    fn preview_output(&self, docs: &[Value], options: &TargetOptions) -> Result<String>;
}

/// Trait for target adapters that write to databases
pub trait DatabaseTargetAdapter: TargetAdapter {
    /// Connect to the target database
    fn connect(
        &mut self,
        connection_string: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Write documents to database
    fn write_documents(
        &self,
        docs: &[Value],
        collection: &str,
        strategy: WriteStrategy,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Write documents in batches with transaction support
    fn write_documents_batch(
        &self,
        docs: &[Value],
        collection: &str,
        batch_config: &BatchConfig,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Upsert documents (insert or update)
    fn upsert_documents(
        &self,
        docs: &[Value],
        collection: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Test database connection and permissions
    fn test_connection(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Validate schema compatibility
    fn validate_schema(
        &self,
        collection: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Get database schema information
    fn get_schema(
        &self,
        collection: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;
}

/// Trait for target adapters that write via APIs
pub trait ApiTargetAdapter: TargetAdapter {
    /// Authenticate with the target API
    fn authenticate(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Push documents to API
    fn push_documents(
        &self,
        docs: &[Value],
        collection: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Push documents in batches
    fn push_documents_batch(
        &self,
        docs: &[Value],
        collection: &str,
        batch_config: &BatchConfig,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Test API connectivity and authentication
    fn test_connection(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Get API schema information
    fn get_api_schema(
        &self,
        collection: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;

    /// Validate documents against API schema
    fn validate_api_documents(
        &self,
        docs: &[Value],
        collection: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}

/// Trait for validating target data
pub trait TargetValidator: Send + Sync {
    /// Get the name of this validator
    fn name(&self) -> &str;

    /// Get a description of what this validator does
    fn description(&self) -> &str;

    /// Validate a single document
    fn validate_document(&self, doc: &Value, schema: Option<&Value>) -> Result<ValidationResult>;

    /// Validate a collection of documents
    fn validate_documents(
        &self,
        docs: &[Value],
        schema: Option<&Value>,
    ) -> Result<Vec<ValidationResult>>;

    /// Get validation schema
    fn get_validation_schema(&self) -> Option<Value> {
        None
    }
}

/// Result of validation
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub field_path: String,
    pub message: String,
    pub error_type: String,
}

#[derive(Debug, Clone)]
pub struct ValidationWarning {
    pub field_path: String,
    pub message: String,
    pub warning_type: String,
}

impl ValidationResult {
    pub fn new() -> Self {
        Self {
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn with_error(mut self, error: ValidationError) -> Self {
        self.valid = false;
        self.errors.push(error);
        self
    }

    pub fn with_warning(mut self, warning: ValidationWarning) -> Self {
        self.warnings.push(warning);
        self
    }

    pub fn is_valid(&self) -> bool {
        self.valid && self.errors.is_empty()
    }
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Registry for managing target validators
pub struct TargetValidatorRegistry {
    validators: std::collections::HashMap<String, Box<dyn TargetValidator>>,
}

impl TargetValidatorRegistry {
    pub fn new() -> Self {
        Self {
            validators: std::collections::HashMap::new(),
        }
    }

    pub fn register<T>(&mut self, name: &str, validator: T)
    where
        T: TargetValidator + 'static,
    {
        self.validators
            .insert(name.to_string(), Box::new(validator));
    }

    pub fn get(&self, name: &str) -> Option<&dyn TargetValidator> {
        self.validators.get(name).map(|v| v.as_ref())
    }

    pub fn list_validators(&self) -> Vec<&str> {
        self.validators.keys().map(|s| s.as_str()).collect()
    }

    pub fn validate_with_all(
        &self,
        docs: &[Value],
        schema: Option<&Value>,
    ) -> Result<Vec<ValidationResult>> {
        let mut results = Vec::new();

        for validator in self.validators.values() {
            let validator_results = validator.validate_documents(docs, schema)?;
            results.extend(validator_results);
        }

        Ok(results)
    }
}

impl Default for TargetValidatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
