use super::super::adapter::TargetWriter;
use super::super::adapter::TargetOptions;
use anyhow::{Result, Context};
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
        self.field_type_mappings.insert(field_type.to_string(), mapping.to_string());
        self
    }

    /// Process a document to handle special field types
    fn process_document(&self, doc: &Value, opts: &TargetOptions) -> Result<Value> {
        let mut processed_doc = doc.clone();

        // Process special field types
        self.process_field_types(&mut processed_doc)?;

        // Process relationships
        if let Some(related_collections) = &opts.related_collections {
            self.process_relationships(&mut processed_doc, related_collections)?;
        }

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
                                    let url = field_obj.get("url").and_then(|v| v.as_str()).unwrap_or("");
                                    let filename = Path::new(url).file_name()
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
                                },
                                "relationship" => {
                                    // Basic relationship handling (enhanced in process_relationships)
                                    if let Some(id) = field_obj.get("id") {
                                        obj[key] = id.clone();
                                    }
                                },
                                "point" => {
                                    // Handle point fields
                                    let lat = field_obj.get("lat").cloned().unwrap_or(Value::Null);
                                    let lng = field_obj.get("lng").cloned().unwrap_or(Value::Null);

                                    obj[key] = json!({
                                        "type": "Point",
                                        "coordinates": [lng, lat]
                                    });
                                },
                                "date" => {
                                    // Handle date fields
                                    if let Some(date_str) = field_obj.get("value").and_then(|v| v.as_str()) {
                                        obj[key] = Value::String(date_str.to_string());
                                    }
                                },
                                "richText" => {
                                    // Handle rich text fields
                                    if let Some(content) = field_obj.get("content") {
                                        obj[key] = content.clone();
                                    }
                                },
                                "array" | "blocks" => {
                                    // Handle array and blocks fields recursively
                                    if let Some(items) = field_obj.get("items").and_then(|v| v.as_array()) {
                                        let mut processed_items = Vec::new();
                                        for item in items {
                                            let mut item_clone = item.clone();
                                            self.process_field_types(&mut item_clone)?;
                                            processed_items.push(item_clone);
                                        }
                                        obj[key] = json!(processed_items);
                                    }
                                },
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
                            if let Some(collection) = field_obj.get("collection").and_then(|v| v.as_str()) {
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
                            if let Some(language) = variant_obj.get("language").and_then(|v| v.as_str()) {
                                if language == locale {
                                    debug!("Found matching locale variant: {}", locale);

                                    // Get the properties for this locale
                                    if let Some(properties) = variant_obj.get("properties") {
                                        if let Some(props_obj) = properties.as_object() {
                                            // Collect properties to insert
                                            for (key, value) in props_obj {
                                                properties_to_insert.push((key.clone(), value.clone()));
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
}

impl TargetWriter for PayloadTarget {
    fn emit_seed(&self, docs: &[Value], out_dir: &str, opts: &TargetOptions) -> Result<()> {
        // Create output directory
        create_dir_all(out_dir).context("Failed to create output directory")?;

        // Get collection name
        let collection = opts.collection.as_deref().unwrap_or("seed");
        let file = format!("{}/{}.seed.ts", out_dir, collection);

        info!("Generating Payload seed file for collection: {}", collection);

        // Process and validate each document
        let mut processed_docs = Vec::new();
        for doc in docs {
            match self.process_document(doc, opts) {
                Ok(processed_doc) => {
                    // Validate the processed document
                    if let Err(e) = self.validate_document(&processed_doc) {
                        warn!("Document validation warning: {}", e);
                    }
                    processed_docs.push(processed_doc);
                },
                Err(e) => {
                    // Log the error but continue processing other documents
                    warn!("Error processing document: {}. Skipping.", e);
                }
            }
        }

        // Generate the seed file
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

        buf.push_str("\nexport const seed = ");
        buf.push_str(&serde_json::to_string_pretty(&processed_docs)
            .context("Failed to serialize processed documents")?);
        buf.push_str(" as const;\n");

        // Write the file
        fs::write(&file, buf).context("Failed to write seed file")?;
        info!("Successfully wrote seed file: {}", file);

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
