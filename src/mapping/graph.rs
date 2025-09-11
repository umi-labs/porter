use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::mapping::typescript::{parse_typescript_file, FieldDefinition};
use crate::mapping::schema_introspect::extract_fields_json;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Scalar,
    Group,
    Array,
    Blocks,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldNode {
    pub path: String,
    pub kind: NodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation_to: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// Build a field graph from a Payload TypeScript collection schema file.
/// This performs a best-effort static analysis using SWC to parse object literals
/// and recursively flatten nested fields/tabs/arrays/blocks.
pub fn build_field_graph_from_ts(schema_path: &str) -> Result<Vec<FieldNode>> {
    // Prefer JSON extraction that resolves custom field factories/imports; fallback to FieldDefinition
    let mut nodes: Vec<FieldNode> = Vec::new();
    if let Ok(values) = extract_fields_json(schema_path) {
        flatten_fields_value(&Value::Array(values), "", &mut nodes)?;
    } else {
        let defs = parse_typescript_file(schema_path)?;
        for def in defs {
            flatten_field_definition(&def, "", &mut nodes)?;
        }
    }

    // Dedup by (path, block_type)
    nodes.sort_by(|a, b| a.path.cmp(&b.path).then(a.block_type.cmp(&b.block_type)));
    nodes.dedup_by(|a, b| a.path == b.path && a.block_type == b.block_type);
    Ok(nodes)
}

fn flatten_field_definition(def: &FieldDefinition, base: &str, out: &mut Vec<FieldNode>) -> Result<()> {
    let name = &def.name;
    let ftype = def.field_type.to_lowercase();
    let path_prefix = if base.is_empty() { name.to_string() } else { format!("{}.{}", base, name) };

    match ftype.as_str() {
        "tabs" => {
            // tabs: expect properties["tabs"] as array of objects with optional fields
            out.push(FieldNode { path: path_prefix.clone(), kind: NodeKind::Group, field_type: Some(def.field_type.clone()), block_type: None, relation_to: None, required: Some(def.required) });
            if let Some(Value::Array(tabs)) = def.properties.get("tabs") {
                for tab in tabs {
                    if let Some(fields) = tab.get("fields") {
                        flatten_fields_value(fields, base, out)?;
                    }
                }
            }
        }
        "group" => {
            out.push(FieldNode { path: path_prefix.clone(), kind: NodeKind::Group, field_type: Some(def.field_type.clone()), block_type: None, relation_to: None, required: Some(def.required) });
            if let Some(fields) = def.properties.get("fields") {
                flatten_fields_value(fields, &path_prefix, out)?;
            }
        }
        "array" => {
            let array_path = format!("{}[]", path_prefix);
            out.push(FieldNode { path: array_path.clone(), kind: NodeKind::Array, field_type: Some(def.field_type.clone()), block_type: None, relation_to: None, required: Some(def.required) });
            if let Some(fields) = def.properties.get("fields") {
                flatten_fields_value(fields, &array_path, out)?;
            }
        }
        "blocks" => {
            let blocks_path = format!("{}[]", path_prefix);
            out.push(FieldNode { path: blocks_path.clone(), kind: NodeKind::Blocks, field_type: Some(def.field_type.clone()), block_type: None, relation_to: None, required: Some(def.required) });
            if let Some(Value::Array(blocks)) = def.properties.get("blocks") {
                for block in blocks {
                    let block_slug = block.get("slug").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let block_path = format!("{}.{{block={}}}", blocks_path, block_slug);
                    out.push(FieldNode { path: block_path.clone(), kind: NodeKind::Block, field_type: Some("block".to_string()), block_type: Some(block_slug.clone()), relation_to: None, required: None });
                    if let Some(fields_val) = block.get("fields") {
                        flatten_fields_value(fields_val, &block_path, out)?;
                    }
                }
            }
        }
        _ => {
            // scalar or relationship/media
            let relation_to = def.properties.get("relationTo").cloned();
            out.push(FieldNode { path: path_prefix, kind: NodeKind::Scalar, field_type: Some(def.field_type.clone()), block_type: None, relation_to, required: Some(def.required) });
        }
    }

    Ok(())
}

fn flatten_fields_value(value: &Value, base: &str, out: &mut Vec<FieldNode>) -> Result<()> {
    match value {
        Value::Array(items) => {
            for item in items {
                if let Value::Object(map) = item {
                    let name = map.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let ftype = map.get("type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                    if name.is_empty() || ftype.is_empty() { continue; }
                    let mut def = FieldDefinition {
                        name,
                        field_type: ftype.clone(),
                        required: map.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
                        validations: Vec::new(),
                        relationship: None,
                        properties: map.clone(),
                    };
                    // For nested objects, properties map already contains nested JSON
                    flatten_field_definition(&def, base, out)?;
                }
            }
            Ok(())
        }
        _ => Err(anyhow!("Expected array for fields property")),
    }
}


