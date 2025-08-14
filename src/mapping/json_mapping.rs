use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Mapping {
    pub source: String,
    pub target: String,
    pub collection: String,
    #[serde(rename = "fieldMappings")]
    pub field_mappings: Vec<FieldMapping>,
    #[serde(default)]
    #[serde(rename = "blockMappings")]
    pub block_mappings: Option<serde_json::Value>
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
