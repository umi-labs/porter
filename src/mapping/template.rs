use serde::{Deserialize, Serialize};
use crate::mapping::graph::FieldNode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingRule {
    pub target_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_type: Option<String>,
}

pub fn generate_template_from_graph(graph: &[FieldNode]) -> Vec<MappingRule> {
    graph
        .iter()
        .filter(|n| matches!(n.kind, crate::mapping::graph::NodeKind::Scalar))
        .map(|n| MappingRule {
            target_path: n.path.clone(),
            source_path: None,
            transform: default_transform_for(n.field_type.as_deref()),
            block_type: n.block_type.clone(),
        })
        .collect()
}

fn default_transform_for(field_type: Option<&str>) -> Option<String> {
    match field_type.unwrap_or("") {
        "richtext" => Some("htmlToLexical".to_string()),
        "date" | "datetime" => Some("dateISO".to_string()),
        _ => None,
    }
}


