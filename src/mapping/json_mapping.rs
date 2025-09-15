use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct Mapping {
    pub source: String,
    pub target: String,
    pub collection: String,
    #[serde(rename = "fieldMappings")]
    pub field_mappings: Vec<FieldMapping>,
    #[serde(default)]
    #[serde(rename = "blockMappings")]
    pub block_mappings: Option<BlockMappings>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FieldMapping {
    pub to: String,
    #[serde(default)]
    pub from: serde_json::Value, // string or array
    #[serde(default)]
    pub transforms: Vec<serde_json::Value>,
    #[serde(default)]
    pub fallback: Option<String>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlockMappings {
    /// Maps ACF flexible content layout names to Payload block types
    /// e.g., "cms" -> "content", "testimonials" -> "testimonials-blog"
    pub layout_to_block_type: HashMap<String, String>,
    /// Maps block types to their field mappings
    /// e.g., "content" -> { "columns" -> "content", "heading" -> "title" }
    pub block_field_mappings: HashMap<String, Vec<FieldMapping>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ACFFlexibleContentBlock {
    pub acf_fc_layout: String,
    pub fields: serde_json::Value, // The actual field data
}

impl BlockMappings {
    pub fn new() -> Self {
        Self {
            layout_to_block_type: HashMap::new(),
            block_field_mappings: HashMap::new(),
        }
    }
}
