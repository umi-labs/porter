use crate::adapters::{
    ConnectionMethod, FileSourceAdapter, SourceAdapter, SourceCapabilities, SourceConfig,
    SourceMetadata,
};
use anyhow::{Context, Result};
use log::{debug, info, warn};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;

/// Umbraco source adapter for reading Umbraco JSON exports
pub struct UmbracoSource {
    /// Content type mappings for special handling
    content_type_mappings: HashMap<String, String>,
    /// Configuration for this adapter instance
    config: Option<SourceConfig>,
    /// Whether the adapter is initialized
    initialized: bool,
}

impl Default for UmbracoSource {
    fn default() -> Self {
        let mut mappings = HashMap::new();
        // Add common Umbraco content type mappings
        mappings.insert("Image".to_string(), "media".to_string());
        mappings.insert("File".to_string(), "media".to_string());
        mappings.insert("Folder".to_string(), "media".to_string());
        mappings.insert("Link".to_string(), "link".to_string());

        Self {
            content_type_mappings: mappings,
            config: None,
            initialized: false,
        }
    }
}

impl UmbracoSource {
    /// Create a new UmbracoSource with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a custom content type mapping
    pub fn with_content_type_mapping(mut self, content_type: &str, mapping: &str) -> Self {
        self.content_type_mappings
            .insert(content_type.to_string(), mapping.to_string());
        self
    }

    /// Process nested content blocks within a document
    fn process_nested_content(&self, doc: &mut Value) -> Result<()> {
        if let Some(obj) = doc.as_object_mut() {
            // Process each field in the document
            for (key, value) in obj.clone().iter() {
                // Check if this is a nested content field
                if let Some(nested_arr) = value.get("contentData").and_then(|v| v.as_array()) {
                    if !nested_arr.is_empty() {
                        debug!("Found nested content in field: {}", key);

                        // Process each nested content item
                        let mut processed_items = Vec::new();
                        for item in nested_arr {
                            let mut item_clone = item.clone();
                            self.process_nested_content(&mut item_clone)?;
                            processed_items.push(item_clone);
                        }

                        // Replace the original nested content with processed items
                        obj[key] = json!({
                            "contentData": processed_items,
                            "contentTypeAlias": value.get("contentTypeAlias").cloned().unwrap_or(Value::Null)
                        });
                    }
                }

                // Recursively process nested objects
                if let Some(_nested_obj) = value.as_object() {
                    let mut nested_value = value.clone();
                    self.process_nested_content(&mut nested_value)?;
                    obj[key] = nested_value;
                }

                // Process arrays that might contain objects
                if let Some(arr) = value.as_array() {
                    let mut processed_arr = Vec::new();
                    for item in arr {
                        if item.is_object() {
                            let mut item_clone = item.clone();
                            self.process_nested_content(&mut item_clone)?;
                            processed_arr.push(item_clone);
                        } else {
                            processed_arr.push(item.clone());
                        }
                    }
                    obj[key] = json!(processed_arr);
                }
            }
        }

        Ok(())
    }

    /// Process media references in a document
    fn process_media(&self, doc: &mut Value) -> Result<()> {
        if let Some(obj) = doc.as_object_mut() {
            // Process each field in the document
            for (key, value) in obj.clone().iter() {
                // Check if this is a media field
                if let Some(content_type) = value.get("contentType").and_then(|v| v.as_str()) {
                    if self.content_type_mappings.get(content_type) == Some(&"media".to_string()) {
                        debug!("Found media reference in field: {}", key);

                        // Extract media information
                        let media_url =
                            value.get("mediaUrl").and_then(|v| v.as_str()).unwrap_or("");
                        let media_id = value.get("mediaId").and_then(|v| v.as_str()).unwrap_or("");
                        let media_name = value
                            .get("mediaName")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        // Create a structured media object
                        obj[key] = json!({
                            "type": "media",
                            "url": media_url,
                            "id": media_id,
                            "name": media_name,
                            "contentType": content_type
                        });
                    }
                }

                // Recursively process nested objects
                if let Some(_nested_obj) = value.as_object() {
                    let mut nested_value = value.clone();
                    self.process_media(&mut nested_value)?;
                    obj[key] = nested_value;
                }

                // Process arrays that might contain objects
                if let Some(arr) = value.as_array() {
                    let mut processed_arr = Vec::new();
                    for item in arr {
                        if item.is_object() {
                            let mut item_clone = item.clone();
                            self.process_media(&mut item_clone)?;
                            processed_arr.push(item_clone);
                        } else {
                            processed_arr.push(item.clone());
                        }
                    }
                    obj[key] = json!(processed_arr);
                }
            }
        }

        Ok(())
    }

    /// Process links in a document
    fn process_links(&self, doc: &mut Value) -> Result<()> {
        if let Some(obj) = doc.as_object_mut() {
            // Process each field in the document
            for (key, value) in obj.clone().iter() {
                // Check if this is a link field
                if let Some(content_type) = value.get("contentType").and_then(|v| v.as_str()) {
                    if self.content_type_mappings.get(content_type) == Some(&"link".to_string()) {
                        debug!("Found link in field: {}", key);

                        // Extract link information
                        let url = value.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        let name = value.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let target = value.get("target").and_then(|v| v.as_str()).unwrap_or("");

                        // Create a structured link object
                        obj[key] = json!({
                            "type": "link",
                            "url": url,
                            "name": name,
                            "target": target,
                            "contentType": content_type
                        });
                    }
                }

                // Recursively process nested objects
                if let Some(_nested_obj) = value.as_object() {
                    let mut nested_value = value.clone();
                    self.process_links(&mut nested_value)?;
                    obj[key] = nested_value;
                }

                // Process arrays that might contain objects
                if let Some(arr) = value.as_array() {
                    let mut processed_arr = Vec::new();
                    for item in arr {
                        if item.is_object() {
                            let mut item_clone = item.clone();
                            self.process_links(&mut item_clone)?;
                            processed_arr.push(item_clone);
                        } else {
                            processed_arr.push(item.clone());
                        }
                    }
                    obj[key] = json!(processed_arr);
                }
            }
        }

        Ok(())
    }

    /// Process content variants (language/culture-specific content)
    fn process_variants(&self, doc: &mut Value) -> Result<()> {
        if let Some(obj) = doc.as_object_mut() {
            // Check if this document has variants
            if let Some(variants_value) = obj.get("variants") {
                if let Some(variants) = variants_value.as_array() {
                    debug!("Found content variants");

                    // Clone the variants array to avoid borrowing issues
                    let variants_clone = variants.clone();

                    // Extract the default variant (usually the first one)
                    if let Some(default_variant) = variants_clone.first() {
                        if let Some(variant_obj) = default_variant.as_object() {
                            // Collect properties to insert
                            let properties_to_insert: Vec<(String, Value)> = variant_obj
                                .iter()
                                .filter(|(key, _)| {
                                    !obj.contains_key(*key)
                                        || *key == "language"
                                        || *key == "segment"
                                })
                                .map(|(key, value)| (key.clone(), value.clone()))
                                .collect();

                            // Now insert them
                            for (key, value) in properties_to_insert {
                                obj.insert(key, value);
                            }
                        }
                    }

                    // Add a structured variants array with language information
                    let mut processed_variants = Vec::new();
                    for variant in variants_clone {
                        if let Some(variant_obj) = variant.as_object() {
                            let language = variant_obj
                                .get("language")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let segment = variant_obj
                                .get("segment")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            processed_variants.push(json!({
                                "language": language,
                                "segment": segment,
                                "properties": variant.clone()
                            }));
                        }
                    }

                    obj.insert("_variants".to_string(), json!(processed_variants));
                }
            }
        }

        Ok(())
    }

    /// Process a single document, applying all enhancements
    fn process_document(&self, doc: &Value) -> Result<Value> {
        let mut processed_doc = doc.clone();

        // Apply all processing steps
        self.process_nested_content(&mut processed_doc)
            .context("Error processing nested content")?;

        self.process_media(&mut processed_doc)
            .context("Error processing media")?;

        self.process_links(&mut processed_doc)
            .context("Error processing links")?;

        self.process_variants(&mut processed_doc)
            .context("Error processing content variants")?;

        Ok(processed_doc)
    }

    /// Extract documents from an Umbraco JSON export
    fn extract_documents(&self, json: &Value) -> Result<Vec<Value>> {
        let mut documents = Vec::new();

        // Try to find content in common Umbraco export structures
        let content_arrays = [
            "accommodationPages",
            "contentPages",
            "content",
            "items",
            "nodes",
            "pages",
        ];

        let mut found_content = false;

        // Check each possible content array
        for array_name in content_arrays {
            if let Some(arr) = json.get(array_name).and_then(|x| x.as_array()) {
                info!(
                    "Found content in '{}' array with {} items",
                    array_name,
                    arr.len()
                );
                for item in arr {
                    documents.push(item.clone());
                }
                found_content = true;
            }
        }

        // If no content arrays were found, check if the root is an array
        if !found_content {
            if let Some(arr) = json.as_array() {
                info!("Using root array with {} items", arr.len());
                for item in arr {
                    documents.push(item.clone());
                }
                found_content = true;
            }
        }

        // If still no content, use the root object as a single document
        if !found_content {
            info!("No content arrays found, using root as a single document");
            documents.push(json.clone());
        }

        Ok(documents)
    }
}

// Temporary: implement both old and new traits during transition
use crate::adapter::SourceReader;

impl SourceReader for UmbracoSource {
    fn read_documents(&self, inputs: &[String]) -> Result<Vec<Value>> {
        let mut out = Vec::new();

        for path in inputs {
            info!("Reading Umbraco data from {}", path);

            // Read and parse the file
            let s = fs::read_to_string(path)
                .with_context(|| format!("Failed to read file: {}", path))?;

            let v: Value = serde_json::from_str(&s)
                .with_context(|| format!("Failed to parse JSON from file: {}", path))?;

            // Extract documents from the JSON
            let documents = self
                .extract_documents(&v)
                .with_context(|| format!("Failed to extract documents from file: {}", path))?;

            info!("Extracted {} documents from {}", documents.len(), path);

            // Process each document
            for doc in documents {
                match self.process_document(&doc) {
                    Ok(processed_doc) => {
                        out.push(processed_doc);
                    }
                    Err(e) => {
                        // Log the error but continue processing other documents
                        warn!("Error processing document: {}. Skipping.", e);
                    }
                }
            }
        }

        info!("Total documents processed: {}", out.len());
        Ok(out)
    }
}

// Implement new trait system
impl SourceAdapter for UmbracoSource {
    fn metadata(&self) -> SourceMetadata {
        SourceMetadata::new(
            "Umbraco CMS",
            "1.0.0",
            "Reads and processes Umbraco JSON export files with support for nested content, media, and multi-language variants"
        )
        .with_formats(vec!["json".to_string()])
        .with_author("Porter Team".to_string())
        .with_homepage("https://github.com/umi-labs/porter".to_string())
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::new()
            .with_streaming(false)
            .with_pagination(false)
            .with_content_types(vec![
                "pages".to_string(),
                "content".to_string(),
                "media".to_string(),
                "nested_content".to_string(),
            ])
            .with_file_formats(vec!["json".to_string()])
            .with_batch_size(Some(1000))
    }

    fn init(&mut self, config: &SourceConfig) -> Result<()> {
        // Validate that this is a file-based configuration
        match &config.connection_method {
            ConnectionMethod::File { .. } => {
                self.config = Some(config.clone());
                self.initialized = true;
                Ok(())
            }
            _ => Err(anyhow::anyhow!(
                "Umbraco source only supports file-based connections"
            )),
        }
    }

    fn validate_config(&self, config: &SourceConfig) -> Result<()> {
        match &config.connection_method {
            ConnectionMethod::File { paths, format, .. } => {
                if paths.is_empty() {
                    return Err(anyhow::anyhow!("At least one file path must be provided"));
                }

                if format != "json" {
                    return Err(anyhow::anyhow!("Umbraco source only supports JSON format"));
                }

                // Check if files exist and are readable
                for path in paths {
                    if !std::path::Path::new(path).exists() {
                        return Err(anyhow::anyhow!("File does not exist: {}", path));
                    }
                }

                Ok(())
            }
            _ => Err(anyhow::anyhow!(
                "Umbraco source only supports file-based connections"
            )),
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        self.config = None;
        self.initialized = false;
        Ok(())
    }
}

impl FileSourceAdapter for UmbracoSource {
    fn read_documents(&self, file_paths: &[String]) -> Result<Vec<Value>> {
        if !self.initialized {
            return Err(anyhow::anyhow!("Umbraco source not initialized"));
        }

        // Use the existing implementation (explicitly call the old trait method)
        SourceReader::read_documents(self, file_paths)
    }

    fn stream_documents(
        &self,
        _file_paths: &[String],
    ) -> Result<Box<dyn Iterator<Item = Result<Value>>>> {
        // For now, return an error as streaming is not implemented
        Err(anyhow::anyhow!(
            "Streaming not yet implemented for Umbraco source"
        ))
    }

    fn supported_formats(&self) -> Vec<String> {
        vec!["json".to_string()]
    }

    fn validate_files(&self, file_paths: &[String]) -> Result<()> {
        for path in file_paths {
            let path_obj = std::path::Path::new(path);

            if !path_obj.exists() {
                return Err(anyhow::anyhow!("File does not exist: {}", path));
            }

            if !path_obj.is_file() {
                return Err(anyhow::anyhow!("Path is not a file: {}", path));
            }

            // Check if we can read the file
            match std::fs::File::open(path) {
                Ok(_) => {}
                Err(e) => return Err(anyhow::anyhow!("Cannot read file {}: {}", path, e)),
            }

            // Basic JSON validation - try to read first few bytes
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    // Check if it's valid JSON by parsing just the first few characters
                    let trimmed = content.trim();
                    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
                        return Err(anyhow::anyhow!("File does not appear to be JSON: {}", path));
                    }
                }
                Err(e) => return Err(anyhow::anyhow!("Cannot read file content {}: {}", path, e)),
            }
        }

        Ok(())
    }
}
