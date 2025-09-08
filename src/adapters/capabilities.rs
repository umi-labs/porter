use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Capabilities of a source adapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCapabilities {
    /// Whether the adapter supports streaming large datasets
    pub supports_streaming: bool,

    /// Whether the adapter supports pagination
    pub supports_pagination: bool,

    /// Whether the adapter supports incremental/delta updates
    pub supports_incremental: bool,

    /// Maximum recommended batch size (None = unlimited)
    pub max_batch_size: Option<usize>,

    /// Supported content types (posts, pages, users, etc.)
    pub supported_content_types: Vec<String>,

    /// Supported file formats (if file-based)
    pub supported_file_formats: Vec<String>,

    /// Whether the adapter requires authentication
    pub requires_authentication: bool,

    /// Whether the adapter supports schema introspection
    pub supports_schema_introspection: bool,

    /// Additional capabilities as key-value pairs
    pub additional_capabilities: HashMap<String, CapabilityValue>,
}

/// Capabilities of a target adapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetCapabilities {
    /// Whether the adapter supports batch writing
    pub supports_batch_writing: bool,

    /// Whether the adapter supports transactions
    pub supports_transactions: bool,

    /// Whether the adapter supports upsert operations
    pub supports_upsert: bool,

    /// Whether the adapter supports schema validation
    pub supports_schema_validation: bool,

    /// Supported output formats
    pub supported_output_formats: Vec<String>,

    /// Supported write strategies
    pub supported_write_strategies: Vec<String>,

    /// Maximum recommended batch size (None = unlimited)
    pub max_batch_size: Option<usize>,

    /// Whether the adapter requires authentication
    pub requires_authentication: bool,

    /// Whether the adapter supports preview mode
    pub supports_preview: bool,

    /// Additional capabilities as key-value pairs
    pub additional_capabilities: HashMap<String, CapabilityValue>,
}

/// Value types for additional capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CapabilityValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Array(Vec<String>),
    Object(HashMap<String, String>),
}

impl Default for SourceCapabilities {
    fn default() -> Self {
        Self {
            supports_streaming: false,
            supports_pagination: false,
            supports_incremental: false,
            max_batch_size: Some(1000),
            supported_content_types: Vec::new(),
            supported_file_formats: Vec::new(),
            requires_authentication: false,
            supports_schema_introspection: false,
            additional_capabilities: HashMap::new(),
        }
    }
}

impl Default for TargetCapabilities {
    fn default() -> Self {
        Self {
            supports_batch_writing: false,
            supports_transactions: false,
            supports_upsert: false,
            supports_schema_validation: false,
            supported_output_formats: Vec::new(),
            supported_write_strategies: vec!["insert".to_string()],
            max_batch_size: Some(1000),
            requires_authentication: false,
            supports_preview: false,
            additional_capabilities: HashMap::new(),
        }
    }
}

impl SourceCapabilities {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_streaming(mut self, enabled: bool) -> Self {
        self.supports_streaming = enabled;
        self
    }

    pub fn with_pagination(mut self, enabled: bool) -> Self {
        self.supports_pagination = enabled;
        self
    }

    pub fn with_incremental(mut self, enabled: bool) -> Self {
        self.supports_incremental = enabled;
        self
    }

    pub fn with_batch_size(mut self, size: Option<usize>) -> Self {
        self.max_batch_size = size;
        self
    }

    pub fn with_content_types(mut self, types: Vec<String>) -> Self {
        self.supported_content_types = types;
        self
    }

    pub fn with_file_formats(mut self, formats: Vec<String>) -> Self {
        self.supported_file_formats = formats;
        self
    }

    pub fn with_authentication(mut self, required: bool) -> Self {
        self.requires_authentication = required;
        self
    }

    pub fn with_schema_introspection(mut self, enabled: bool) -> Self {
        self.supports_schema_introspection = enabled;
        self
    }

    pub fn with_capability(mut self, key: String, value: CapabilityValue) -> Self {
        self.additional_capabilities.insert(key, value);
        self
    }
}

impl TargetCapabilities {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_batch_writing(mut self, enabled: bool) -> Self {
        self.supports_batch_writing = enabled;
        self
    }

    pub fn with_transactions(mut self, enabled: bool) -> Self {
        self.supports_transactions = enabled;
        self
    }

    pub fn with_upsert(mut self, enabled: bool) -> Self {
        self.supports_upsert = enabled;
        self
    }

    pub fn with_schema_validation(mut self, enabled: bool) -> Self {
        self.supports_schema_validation = enabled;
        self
    }

    pub fn with_output_formats(mut self, formats: Vec<String>) -> Self {
        self.supported_output_formats = formats;
        self
    }

    pub fn with_write_strategies(mut self, strategies: Vec<String>) -> Self {
        self.supported_write_strategies = strategies;
        self
    }

    pub fn with_batch_size(mut self, size: Option<usize>) -> Self {
        self.max_batch_size = size;
        self
    }

    pub fn with_authentication(mut self, required: bool) -> Self {
        self.requires_authentication = required;
        self
    }

    pub fn with_preview(mut self, enabled: bool) -> Self {
        self.supports_preview = enabled;
        self
    }

    pub fn with_capability(mut self, key: String, value: CapabilityValue) -> Self {
        self.additional_capabilities.insert(key, value);
        self
    }
}
