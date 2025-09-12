use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::mapping::typescript::{parse_typescript_file, FieldDefinition};
use crate::mapping::schema_introspect::extract_fields_json;
use crate::parser::{PayloadSchemaParser, FlattenedTemplate};
use crate::config::PorterConfig;
use std::path::{Path, PathBuf};

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
    // Try to use the new parser first if we can determine the project root
    if let Ok(template) = build_field_graph_with_new_parser(schema_path) {
        return Ok(template);
    }
    
    // Fallback to the old implementation
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

/// Build field graph using the new parser with full import resolution
fn build_field_graph_with_new_parser(schema_path: &str) -> Result<Vec<FieldNode>> {
    use crate::dlog;
    
    dlog!("Attempting to use new parser for: {}", schema_path);
    
    let config_path = Path::new(schema_path);
    
    // Try to find the project root
    let base_dir = find_project_root(config_path)?;
    dlog!("Found project root: {:?}", base_dir);
    
    // Try to load the Porter config to get TypeScript settings
    let ts_config = load_typescript_config(&base_dir);
    
    // Create the new parser
    let mut parser = PayloadSchemaParser::new(base_dir, ts_config.as_ref());
    
    // Generate the template
    let template = parser.generate_template(config_path)?;
    dlog!("Generated template with {} fields", template.fields.len());
    
    // Convert FlattenedTemplate to Vec<FieldNode>
    let mut nodes = Vec::new();
    
    for field in &template.fields {
        let kind = match field.field_type.as_str() {
            "array" => NodeKind::Array,
            "group" => NodeKind::Group,
            "blocks" => NodeKind::Blocks,
            "block" => NodeKind::Block,
            _ => NodeKind::Scalar,
        };
        
        nodes.push(FieldNode {
            path: field.path.clone(),
            kind,
            field_type: Some(field.field_type.clone()),
            block_type: None,
            relation_to: if field.field_type == "relationship" {
                field.validation.as_ref().and_then(|v| {
                    serde_json::to_value(v).ok()
                })
            } else {
                None
            },
            required: Some(field.required),
        });
    }
    
    // Process blocks
    for block in &template.blocks {
        for field in &block.fields {
            nodes.push(FieldNode {
                path: field.path.clone(),
                kind: NodeKind::Scalar,
                field_type: Some(field.field_type.clone()),
                block_type: Some(block.slug.clone()),
                relation_to: None,
                required: Some(field.required),
            });
        }
    }
    
    dlog!("Converted to {} field nodes", nodes.len());
    Ok(nodes)
}

fn find_project_root(from: &Path) -> Result<PathBuf> {
    let mut current = from.parent();
    
    while let Some(dir) = current {
        // Look for indicators of project root
        if dir.join("package.json").exists() 
            || dir.join("tsconfig.json").exists()
            || dir.join("payload.config.ts").exists() {
            return Ok(dir.to_path_buf());
        }
        current = dir.parent();
    }
    
    // Fallback to parent of config file
    from.parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow!("Could not determine project root"))
}

fn load_typescript_config(base_dir: &Path) -> Option<crate::config::TypescriptSection> {
    use crate::dlog;
    
    // Look for porter config files
    let config_names = vec![
        "porter.config.toml",
        ".porter.toml",
        "migration.toml",
        "migrate.toml",
    ];
    
    for name in config_names {
        let config_path = base_dir.join(name);
        if config_path.exists() {
            dlog!("Found config file: {:?}", config_path);
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(config) = toml::from_str::<PorterConfig>(&content) {
                    return config.typescript;
                }
            }
        }
    }
    
    None
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
            // If group has no name, inline its fields at current base
            let next_base = if name.is_empty() { base } else { &path_prefix };
            if !name.is_empty() {
                out.push(FieldNode { path: path_prefix.clone(), kind: NodeKind::Group, field_type: Some(def.field_type.clone()), block_type: None, relation_to: None, required: Some(def.required) });
            }
            if let Some(fields) = def.properties.get("fields") {
                flatten_fields_value(fields, next_base, out)?;
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
                match item {
                    Value::Object(map) => {
                        let ftype = map.get("type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        // Special-case container without name: tabs
                        if ftype == "tabs" {
                            if let Some(Value::Array(tabs)) = map.get("tabs") {
                                for tab in tabs {
                                    if let Some(fields) = tab.get("fields") {
                                        flatten_fields_value(fields, base, out)?;
                                    }
                                }
                            }
                            continue;
                        }

                        // Recognize SEO helper argument objects and synthesize scalar nodes
                        if ftype.is_empty() {
                            let title_path = map.get("titlePath").and_then(|v| v.as_str());
                            let desc_path = map.get("descriptionPath").and_then(|v| v.as_str());
                            let image_path = map.get("imagePath").and_then(|v| v.as_str());
                            if title_path.is_some() || desc_path.is_some() || image_path.is_some() {
                                if let Some(p) = title_path { out.push(FieldNode { path: p.to_string(), kind: NodeKind::Scalar, field_type: Some("text".to_string()), block_type: None, relation_to: None, required: None }); }
                                if let Some(p) = desc_path { out.push(FieldNode { path: p.to_string(), kind: NodeKind::Scalar, field_type: Some("textarea".to_string()), block_type: None, relation_to: None, required: None }); }
                                if let Some(p) = image_path { out.push(FieldNode { path: p.to_string(), kind: NodeKind::Scalar, field_type: Some("upload".to_string()), block_type: None, relation_to: Some(Value::String("media".to_string())), required: None }); }
                                continue;
                            }
                        }

                        let name = map.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        if ftype.is_empty() { continue; }
                        if name.is_empty() && ftype != "tabs" {
                            // Without a name, we cannot place scalar/group/array/blocks on a path; skip
                            // (tabs handled above)
                            // But allow top-level field objects (like hero group) passed by reference with name inside
                            if let Some(Value::Array(fields)) = map.get("fields") {
                                // Inline unnamed group by recursing with current base
                                flatten_fields_value(&Value::Array(fields.clone()), base, out)?;
                                continue;
                            } else {
                                continue;
                            }
                        }
                        let mut properties: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
                        for (k, v) in map.iter() {
                            properties.insert(k.clone(), v.clone());
                        }
                        let def = FieldDefinition {
                            name,
                            field_type: ftype.clone(),
                            required: map.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
                            validations: Vec::new(),
                            relationship: None,
                            properties,
                        };
                        flatten_field_definition(&def, base, out)?;
                    }
                    Value::Array(inner) => {
                        // handle spread results like ...slugField() that returned an array of fields
                        flatten_fields_value(&Value::Array(inner.clone()), base, out)?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        _ => Err(anyhow!("Expected array for fields property")),
    }
}


