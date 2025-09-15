use crate::adapters::{
    FileTargetAdapter, OutputMethod, TargetAdapter, TargetCapabilities, TargetConfig,
    TargetMetadata, TargetOptions,
};
use anyhow::{Context, Result};
use log::{debug, info, warn};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, create_dir_all};
use std::path::Path;

/// Payload target adapter for generating Payload CMS seed files
pub struct PayloadTarget {
    /// Field type mappings for special handling
    field_type_mappings: HashMap<String, String>,
}

impl Default for PayloadTarget {
    fn default() -> Self {
        let mut mappings = HashMap::new();
        // Add common Payload field type mappings
        mappings.insert("media".to_string(), "upload".to_string());
        mappings.insert("link".to_string(), "relationship".to_string());
        mappings.insert("point".to_string(), "point".to_string());
        mappings.insert("date".to_string(), "date".to_string());
        mappings.insert("richText".to_string(), "richText".to_string());
        mappings.insert("array".to_string(), "array".to_string());
        mappings.insert("blocks".to_string(), "blocks".to_string());
        mappings.insert("relationship".to_string(), "relationship".to_string());

        Self {
            field_type_mappings: mappings,
        }
    }
}

impl PayloadTarget {
    /// Create a new PayloadTarget with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a custom field type mapping
    pub fn with_field_type_mapping(mut self, field_type: &str, mapping: &str) -> Self {
        self.field_type_mappings
            .insert(field_type.to_string(), mapping.to_string());
        self
    }

    /// Process a document to handle special field types
    fn process_document(&self, doc: &Value, opts: &crate::adapter::TargetOptions) -> Result<Value> {
        let mut processed_doc = doc.clone();

        // Process special field types
        self.process_field_types(&mut processed_doc)?;

        // Process relationships - TODO: implement this properly with new config structure
        // if let Some(related_collections) = &opts.related_collections {
        //     self.process_relationships(&mut processed_doc, related_collections)?;
        // }

        // Process localization
        if let Some(locale) = &opts.locale {
            self.process_localization(&mut processed_doc, locale)?;
        }

        Ok(processed_doc)
    }

    /// Process special field types in a document
    fn process_field_types(&self, doc: &mut Value) -> Result<()> {
        if let Some(obj) = doc.as_object_mut() {
            // Process each field in the document
            for (key, value) in obj.clone().iter() {
                // Check if this is a special field type
                if let Some(field_obj) = value.as_object() {
                    if let Some(field_type) = field_obj.get("type").and_then(|v| v.as_str()) {
                        if let Some(payload_type) = self.field_type_mappings.get(field_type) {
                            debug!("Processing field '{}' with type '{}'", key, field_type);

                            match payload_type.as_str() {
                                "upload" => {
                                    // Handle media/upload fields
                                    let url =
                                        field_obj.get("url").and_then(|v| v.as_str()).unwrap_or("");
                                    let filename = Path::new(url)
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("");

                                    obj[key] = json!({
                                        "url": url,
                                        "filename": filename,
                                        "mimeType": guess_mime_type(url),
                                        "filesize": field_obj.get("filesize").cloned().unwrap_or(Value::Null),
                                        "width": field_obj.get("width").cloned().unwrap_or(Value::Null),
                                        "height": field_obj.get("height").cloned().unwrap_or(Value::Null),
                                    });
                                }
                                "relationship" => {
                                    // Basic relationship handling (enhanced in process_relationships)
                                    if let Some(id) = field_obj.get("id") {
                                        obj[key] = id.clone();
                                    }
                                }
                                "point" => {
                                    // Handle point fields
                                    let lat = field_obj.get("lat").cloned().unwrap_or(Value::Null);
                                    let lng = field_obj.get("lng").cloned().unwrap_or(Value::Null);

                                    obj[key] = json!({
                                        "type": "Point",
                                        "coordinates": [lng, lat]
                                    });
                                }
                                "date" => {
                                    // Handle date fields
                                    if let Some(date_str) =
                                        field_obj.get("value").and_then(|v| v.as_str())
                                    {
                                        obj[key] = Value::String(date_str.to_string());
                                    }
                                }
                                "richText" => {
                                    // Handle rich text fields
                                    if let Some(content) = field_obj.get("content") {
                                        obj[key] = content.clone();
                                    }
                                }
                                "array" | "blocks" => {
                                    // Handle array and blocks fields recursively
                                    if let Some(items) =
                                        field_obj.get("items").and_then(|v| v.as_array())
                                    {
                                        let mut processed_items = Vec::new();
                                        for item in items {
                                            let mut item_clone = item.clone();
                                            self.process_field_types(&mut item_clone)?;
                                            processed_items.push(item_clone);
                                        }
                                        obj[key] = json!(processed_items);
                                    }
                                }
                                _ => {
                                    // Unknown type, leave as is
                                    warn!("Unknown field type mapping: {}", payload_type);
                                }
                            }
                        }
                    }
                }

                // Recursively process nested objects
                if let Some(_nested_obj) = value.as_object() {
                    let mut nested_value = value.clone();
                    self.process_field_types(&mut nested_value)?;
                    obj[key] = nested_value;
                }

                // Process arrays that might contain objects
                if let Some(arr) = value.as_array() {
                    let mut processed_arr = Vec::new();
                    for item in arr {
                        if item.is_object() {
                            let mut item_clone = item.clone();
                            self.process_field_types(&mut item_clone)?;
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

    /// Process relationships between collections
    #[allow(dead_code)]
    fn process_relationships(&self, doc: &mut Value, related_collections: &[String]) -> Result<()> {
        if let Some(obj) = doc.as_object_mut() {
            // Process each field in the document
            for (key, value) in obj.clone().iter() {
                // Check if this field is a relationship
                if let Some(field_obj) = value.as_object() {
                    if let Some(field_type) = field_obj.get("type").and_then(|v| v.as_str()) {
                        if field_type == "relationship" {
                            debug!("Processing relationship field: {}", key);

                            // Check if the relationship has a collection specified
                            if let Some(collection) =
                                field_obj.get("collection").and_then(|v| v.as_str())
                            {
                                if related_collections.contains(&collection.to_string()) {
                                    // This is a relationship to a known collection
                                    if let Some(id) = field_obj.get("id") {
                                        obj[key] = json!({
                                            "relationTo": collection,
                                            "value": id
                                        });
                                    }
                                }
                            } else if let Some(id) = field_obj.get("id") {
                                // No collection specified, use simple ID reference
                                obj[key] = id.clone();
                            }
                        }
                    }
                }

                // Recursively process nested objects
                if let Some(_nested_obj) = value.as_object() {
                    let mut nested_value = value.clone();
                    self.process_relationships(&mut nested_value, related_collections)?;
                    obj[key] = nested_value;
                }

                // Process arrays that might contain objects
                if let Some(arr) = value.as_array() {
                    let mut processed_arr = Vec::new();
                    for item in arr {
                        if item.is_object() {
                            let mut item_clone = item.clone();
                            self.process_relationships(&mut item_clone, related_collections)?;
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

    /// Process localization in a document
    fn process_localization(&self, doc: &mut Value, locale: &str) -> Result<()> {
        if let Some(obj) = doc.as_object_mut() {
            // Check if this document has variants
            if let Some(variants_value) = obj.get("_variants") {
                if let Some(variants) = variants_value.as_array() {
                    debug!("Processing localization with locale: {}", locale);

                    // Clone the variants array to avoid borrowing issues
                    let variants_clone = variants.clone();

                    // Properties to insert
                    let mut properties_to_insert = Vec::new();

                    // Find the variant that matches the requested locale
                    for variant in variants_clone {
                        if let Some(variant_obj) = variant.as_object() {
                            if let Some(language) =
                                variant_obj.get("language").and_then(|v| v.as_str())
                            {
                                if language == locale {
                                    debug!("Found matching locale variant: {}", locale);

                                    // Get the properties for this locale
                                    if let Some(properties) = variant_obj.get("properties") {
                                        if let Some(props_obj) = properties.as_object() {
                                            // Collect properties to insert
                                            for (key, value) in props_obj {
                                                properties_to_insert
                                                    .push((key.clone(), value.clone()));
                                            }
                                        }
                                    }

                                    break;
                                }
                            }
                        }
                    }

                    // Now insert the collected properties
                    for (key, value) in properties_to_insert {
                        obj.insert(key, value);
                    }

                    // Remove the variants array as it's no longer needed
                    obj.remove("_variants");
                }
            }
        }

        Ok(())
    }

    /// Validate a document against Payload field requirements
    fn validate_document(&self, doc: &Value) -> Result<()> {
        if let Some(obj) = doc.as_object() {
            // Check for required fields
            if !obj.contains_key("id") {
                warn!("Document is missing required 'id' field");
            }

            // Check for invalid field types
            for (key, value) in obj {
                if let Some(field_obj) = value.as_object() {
                    if let Some(field_type) = field_obj.get("type").and_then(|v| v.as_str()) {
                        if !self.field_type_mappings.contains_key(field_type) {
                            warn!("Field '{}' has unknown type: {}", key, field_type);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Extract a function name from a document, using title field or fallback to index
    fn extract_function_name(&self, doc: &Value, index: usize) -> String {
        // Try to get the title field
        if let Some(title) = doc.get("title").and_then(|v| v.as_str()) {
            // Convert title to lowercase and replace spaces/special chars with underscores
            let mut function_name = title.to_lowercase();
            function_name = function_name
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect::<String>();
            
            // Remove multiple consecutive underscores
            while function_name.contains("__") {
                function_name = function_name.replace("__", "_");
            }
            
            // Remove leading/trailing underscores
            function_name = function_name.trim_matches('_').to_string();
            
            // Ensure it's not empty and starts with a letter
            if !function_name.is_empty() && function_name.chars().next().unwrap().is_alphabetic() {
                return function_name;
            }
        }
        
        // Fallback to index-based name
        format!("item_{}", index)
    }
}

// Temporary: implement both old and new traits during transition
use crate::adapter::TargetWriter;

impl TargetWriter for PayloadTarget {
    fn emit_seed(
        &self,
        docs: &[Value],
        out_dir: &str,
        opts: &crate::adapter::TargetOptions,
    ) -> Result<()> {
        // Create output directory
        create_dir_all(out_dir).context("Failed to create output directory")?;

        // Get collection name
        let collection = opts.collection.as_deref().unwrap_or("seed");

        info!(
            "Generating Payload seed files for collection: {} ({} documents)",
            collection,
            docs.len()
        );

        // Process and validate each document, creating separate files
        for (index, doc) in docs.iter().enumerate() {
            match self.process_document(doc, opts) {
                Ok(processed_doc) => {
                    // Validate the processed document
                    if let Err(e) = self.validate_document(&processed_doc) {
                        warn!("Document validation warning: {}", e);
                    }

                    // Extract title for function name
                    let function_name = self.extract_function_name(&processed_doc, index);
                    let file_name = format!("{}/{}.ts", out_dir, function_name);

                    // Generate the seed file content
                    let mut buf = String::new();
                    buf.push_str("// GENERATED BY PORTER\n");
                    buf.push_str("// Collection: ");
                    buf.push_str(collection);
                    buf.push_str("\n");

                    if let Some(locale) = &opts.locale {
                        buf.push_str("// Locale: ");
                        buf.push_str(locale);
                        buf.push_str("\n");
                    }

                    // Add import statement
                    buf.push_str("\nimport type { RequiredDataFromCollectionSlug } from 'payload'\n\n");

                    // Generate function with proper type annotation
                    buf.push_str("export const ");
                    buf.push_str(&function_name);
                    buf.push_str(": () => RequiredDataFromCollectionSlug<'");
                    buf.push_str(collection);
                    buf.push_str("'> = () => {\nreturn ");
                    buf.push_str("'_status: 'published', ");
                    buf.push_str(
                        &serde_json::to_string_pretty(&processed_doc)
                            .context("Failed to serialize processed document")?,
                    );
                    buf.push_str("\n};\n");

                    // Write the file
                    fs::write(&file_name, buf).context("Failed to write seed file")?;
                    
                    info!("Generated seed file: {}", file_name);
                }
                Err(e) => {
                    // Log the error but continue processing other documents
                    warn!("Error processing document: {}. Skipping.", e);
                }
            }
        }

        Ok(())
    }
}

/// Guess the MIME type of a file based on its extension
fn guess_mime_type(path: &str) -> &'static str {
    let extension = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        "ppt" | "pptx" => "application/vnd.ms-powerpoint",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        _ => "application/octet-stream",
    }
}

/// Payload target adapter state
pub struct PayloadTargetState {
    #[allow(dead_code)]
    config: Option<TargetConfig>,
    #[allow(dead_code)]
    initialized: bool,
}

impl Default for PayloadTargetState {
    fn default() -> Self {
        Self {
            config: None,
            initialized: false,
        }
    }
}

// Add state to PayloadTarget
impl PayloadTarget {
    /// Create new instance with state
    pub fn with_state() -> (Self, PayloadTargetState) {
        (Self::new(), PayloadTargetState::default())
    }
}

// Implement new trait system
impl TargetAdapter for PayloadTarget {
    fn metadata(&self) -> TargetMetadata {
        TargetMetadata::new(
            "Payload CMS",
            "1.0.0",
            "Generates TypeScript seed files for Payload CMS with support for complex field types and relationships"
        )
        .with_formats(vec!["typescript".to_string(), "javascript".to_string(), "json".to_string()])
        .with_author("Porter Team".to_string())
        .with_homepage("https://github.com/umi-labs/porter".to_string())
    }

    fn capabilities(&self) -> TargetCapabilities {
        TargetCapabilities::new()
            .with_batch_writing(true)
            .with_schema_validation(false) // TODO: Implement schema validation
            .with_output_formats(vec![
                "typescript".to_string(),
                "javascript".to_string(),
                "json".to_string(),
            ])
            .with_write_strategies(vec!["insert".to_string()])
            .with_batch_size(Some(1000))
            .with_preview(true)
    }

    fn init(&mut self, config: &TargetConfig) -> Result<()> {
        // Validate that this is a file-based configuration
        match &config.output_method {
            OutputMethod::SeedFiles { .. } => {
                // Store config would need state management
                // For now, just validate
                Ok(())
            }
            _ => Err(anyhow::anyhow!(
                "Payload target currently only supports seed file output"
            )),
        }
    }

    fn validate_config(&self, config: &TargetConfig) -> Result<()> {
        match &config.output_method {
            OutputMethod::SeedFiles { output_dir, format } => {
                if output_dir.is_empty() {
                    return Err(anyhow::anyhow!("Output directory must be provided"));
                }

                let supported_formats = vec!["typescript", "javascript", "json"];
                if !supported_formats.contains(&format.as_str()) {
                    return Err(anyhow::anyhow!(
                        "Unsupported format '{}'. Supported formats: {:?}",
                        format,
                        supported_formats
                    ));
                }

                // Check if output directory can be created/written to
                let output_path = std::path::Path::new(output_dir);
                if let Some(parent) = output_path.parent() {
                    if !parent.exists() {
                        return Err(anyhow::anyhow!(
                            "Parent directory does not exist: {}",
                            parent.display()
                        ));
                    }
                }

                Ok(())
            }
            _ => Err(anyhow::anyhow!(
                "Payload target currently only supports seed file output"
            )),
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        // No cleanup needed for current implementation
        Ok(())
    }

    fn validate_documents(&self, docs: &[Value]) -> Result<()> {
        // Basic document validation
        for (index, doc) in docs.iter().enumerate() {
            if !doc.is_object() {
                return Err(anyhow::anyhow!(
                    "Document at index {} is not an object",
                    index
                ));
            }

            // Check for required fields (basic validation)
            let obj = doc.as_object().unwrap();
            if obj.is_empty() {
                return Err(anyhow::anyhow!("Document at index {} is empty", index));
            }
        }

        Ok(())
    }
}

impl FileTargetAdapter for PayloadTarget {
    fn write_documents(&self, docs: &[Value], options: &TargetOptions) -> Result<()> {
        // Use the existing implementation
        // Convert new TargetOptions to old format temporarily
        let legacy_options = crate::adapter::TargetOptions {
            collection: Some(options.collection_name.clone()),
            locale: options.locale.clone(),
            related_collections: None, // TODO: map this properly
        };
        self.emit_seed(docs, &options.output_dir, &legacy_options)
    }

    fn write_documents_batch(
        &self,
        docs: &[Value],
        options: &TargetOptions,
        batch_config: &crate::adapters::BatchConfig,
    ) -> Result<()> {
        if batch_config.parallel && docs.len() > batch_config.size {
            // Process in parallel batches
            use rayon::prelude::*;

            let batches: Vec<_> = docs.chunks(batch_config.size).collect();
            let results: Result<Vec<_>, _> = batches
                .into_par_iter()
                .map(|batch| self.write_documents(batch, options))
                .collect();

            results.map(|_| ())
        } else {
            // Process in sequential batches
            for chunk in docs.chunks(batch_config.size) {
                self.write_documents(chunk, options)?;
            }
            Ok(())
        }
    }

    fn supported_formats(&self) -> Vec<String> {
        vec![
            "typescript".to_string(),
            "javascript".to_string(),
            "json".to_string(),
        ]
    }

    fn validate_output_path(&self, output_path: &str) -> Result<()> {
        let path = std::path::Path::new(output_path);

        // Check if parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                return Err(anyhow::anyhow!(
                    "Parent directory does not exist: {}",
                    parent.display()
                ));
            }

            // Check write permissions
            match std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(path.join(".porter_test"))
            {
                Ok(_) => {
                    // Cleanup test file
                    let _ = std::fs::remove_file(path.join(".porter_test"));
                    Ok(())
                }
                Err(e) => Err(anyhow::anyhow!(
                    "Cannot write to output directory {}: {}",
                    output_path,
                    e
                )),
            }
        } else {
            Err(anyhow::anyhow!("Invalid output path: {}", output_path))
        }
    }

    fn preview_output(&self, docs: &[Value], options: &TargetOptions) -> Result<String> {
        if docs.is_empty() {
            return Ok("// No documents to preview".to_string());
        }

        // Generate preview for first few documents
        let preview_docs = if docs.len() > 3 { &docs[0..3] } else { docs };

        let mut preview = String::new();
        preview.push_str(&format!(
            "// Preview for collection: {}\n",
            options.collection_name
        ));
        preview.push_str(&format!("// Total documents: {}\n", docs.len()));
        preview.push_str(&format!(
            "// Showing first {} document(s)\n\n",
            preview_docs.len()
        ));

        // Generate TypeScript interface preview
        preview.push_str(&self.generate_interface_preview(preview_docs)?);

        // Add sample data
        preview.push_str("\n// Sample data:\n");
        for (index, doc) in preview_docs.iter().enumerate() {
            preview.push_str(&format!("// Document {}:\n", index + 1));
            preview.push_str(&format!("// {}\n", serde_json::to_string_pretty(doc)?));
        }

        if docs.len() > 3 {
            preview.push_str(&format!("\n// ... and {} more documents", docs.len() - 3));
        }

        Ok(preview)
    }
}

impl PayloadTarget {
    fn generate_interface_preview(&self, docs: &[Value]) -> Result<String> {
        let mut interface = String::new();
        interface.push_str("interface DocumentPreview {\n");

        // Analyze fields from sample documents
        let mut field_types = HashMap::new();
        for doc in docs {
            if let Some(obj) = doc.as_object() {
                for (key, value) in obj {
                    let type_name = self.infer_typescript_type(value);
                    field_types.insert(key.clone(), type_name);
                }
            }
        }

        // Generate interface fields
        for (field_name, type_name) in field_types {
            interface.push_str(&format!("  {}: {};\n", field_name, type_name));
        }

        interface.push_str("}\n");
        Ok(interface)
    }

    fn infer_typescript_type(&self, value: &Value) -> String {
        match value {
            Value::String(_) => "string".to_string(),
            Value::Number(_) => "number".to_string(),
            Value::Bool(_) => "boolean".to_string(),
            Value::Array(arr) => {
                if arr.is_empty() {
                    "any[]".to_string()
                } else {
                    let element_type = self.infer_typescript_type(&arr[0]);
                    format!("{}[]", element_type)
                }
            }
            Value::Object(_) => "object".to_string(),
            Value::Null => "null".to_string(),
        }
    }
}
