use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata about a source adapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceMetadata {
    /// Human-readable name of the source
    pub name: String,

    /// Version of the source adapter
    pub version: String,

    /// Description of what this source adapter does
    pub description: String,

    /// Supported data formats (JSON, XML, CSV, etc.)
    pub supported_formats: Vec<String>,

    /// Author/maintainer information
    pub author: Option<String>,

    /// License information
    pub license: Option<String>,

    /// Homepage or documentation URL
    pub homepage: Option<String>,

    /// Repository URL
    pub repository: Option<String>,

    /// Configuration schema (JSON Schema)
    pub config_schema: Option<serde_json::Value>,

    /// Example configuration
    pub example_config: Option<serde_json::Value>,

    /// Additional metadata as key-value pairs
    pub additional_metadata: HashMap<String, String>,
}

/// Metadata about a target adapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetMetadata {
    /// Human-readable name of the target
    pub name: String,

    /// Version of the target adapter
    pub version: String,

    /// Description of what this target adapter does
    pub description: String,

    /// Supported output formats (TypeScript, JSON, etc.)
    pub supported_formats: Vec<String>,

    /// Author/maintainer information
    pub author: Option<String>,

    /// License information
    pub license: Option<String>,

    /// Homepage or documentation URL
    pub homepage: Option<String>,

    /// Repository URL
    pub repository: Option<String>,

    /// Configuration schema (JSON Schema)
    pub config_schema: Option<serde_json::Value>,

    /// Example configuration
    pub example_config: Option<serde_json::Value>,

    /// Additional metadata as key-value pairs
    pub additional_metadata: HashMap<String, String>,
}

impl Default for SourceMetadata {
    fn default() -> Self {
        Self {
            name: "Unknown Source".to_string(),
            version: "0.1.0".to_string(),
            description: "No description provided".to_string(),
            supported_formats: Vec::new(),
            author: None,
            license: None,
            homepage: None,
            repository: None,
            config_schema: None,
            example_config: None,
            additional_metadata: HashMap::new(),
        }
    }
}

impl Default for TargetMetadata {
    fn default() -> Self {
        Self {
            name: "Unknown Target".to_string(),
            version: "0.1.0".to_string(),
            description: "No description provided".to_string(),
            supported_formats: Vec::new(),
            author: None,
            license: None,
            homepage: None,
            repository: None,
            config_schema: None,
            example_config: None,
            additional_metadata: HashMap::new(),
        }
    }
}

impl SourceMetadata {
    pub fn new(name: &str, version: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            description: description.to_string(),
            ..Default::default()
        }
    }

    pub fn with_formats(mut self, formats: Vec<String>) -> Self {
        self.supported_formats = formats;
        self
    }

    pub fn with_author(mut self, author: String) -> Self {
        self.author = Some(author);
        self
    }

    pub fn with_license(mut self, license: String) -> Self {
        self.license = Some(license);
        self
    }

    pub fn with_homepage(mut self, homepage: String) -> Self {
        self.homepage = Some(homepage);
        self
    }

    pub fn with_repository(mut self, repository: String) -> Self {
        self.repository = Some(repository);
        self
    }

    pub fn with_config_schema(mut self, schema: serde_json::Value) -> Self {
        self.config_schema = Some(schema);
        self
    }

    pub fn with_example_config(mut self, config: serde_json::Value) -> Self {
        self.example_config = Some(config);
        self
    }

    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.additional_metadata.insert(key, value);
        self
    }
}

impl TargetMetadata {
    pub fn new(name: &str, version: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            description: description.to_string(),
            ..Default::default()
        }
    }

    pub fn with_formats(mut self, formats: Vec<String>) -> Self {
        self.supported_formats = formats;
        self
    }

    pub fn with_author(mut self, author: String) -> Self {
        self.author = Some(author);
        self
    }

    pub fn with_license(mut self, license: String) -> Self {
        self.license = Some(license);
        self
    }

    pub fn with_homepage(mut self, homepage: String) -> Self {
        self.homepage = Some(homepage);
        self
    }

    pub fn with_repository(mut self, repository: String) -> Self {
        self.repository = Some(repository);
        self
    }

    pub fn with_config_schema(mut self, schema: serde_json::Value) -> Self {
        self.config_schema = Some(schema);
        self
    }

    pub fn with_example_config(mut self, config: serde_json::Value) -> Self {
        self.example_config = Some(config);
        self
    }

    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.additional_metadata.insert(key, value);
        self
    }
}
