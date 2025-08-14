// src/mapping/typescript.rs
use anyhow::{Result, anyhow, Context};
use log::debug;
use std::path::Path;
use swc_common::sync::Lrc;
use swc_common::{
    errors::{ColorConfig, Handler},
    SourceMap,
};
use swc_ecma_ast::*;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsConfig};
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::util::fs;

/// Field definition extracted from a TypeScript file
#[derive(Debug, Clone)]
pub struct FieldDefinition {
    /// Field name
    pub name: String,
    /// Field type
    pub field_type: String,
    /// Whether the field is required
    pub required: bool,
    /// Field validations
    pub validations: Vec<Value>,
    /// Field relationship information
    pub relationship: Option<RelationshipInfo>,
    /// Additional field properties
    pub properties: HashMap<String, Value>,
}

/// Relationship information for a field
#[derive(Debug, Clone)]
pub struct RelationshipInfo {
    /// Related collection name
    pub collection: String,
    /// Relationship type (hasOne, hasMany)
    pub relationship_type: String,
}

/// Parse a TypeScript file and extract field definitions
pub fn parse_typescript_file(file_path: &str) -> Result<Vec<FieldDefinition>> {
    // Check if file exists
    if !Path::new(file_path).exists() {
        return Err(anyhow!("File not found: {}", file_path));
    }

    // Read file content
    let content = fs::read_file(file_path)
        .with_context(|| format!("Failed to read TypeScript file: {}", file_path))?;

    // Set up SWC parser
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

    // Create source file
    let fm = cm.new_source_file(
        swc_common::FileName::Custom(file_path.into()),
        content,
    );

    // Create lexer and parser
    let lexer = Lexer::new(
        Syntax::Typescript(TsConfig {
            tsx: false,
            decorators: true,
            dts: false,
            no_early_errors: false,
            disallow_ambiguous_jsx_like: false,
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);

    // Parse the module
    let module = parser
        .parse_module()
        .map_err(|e| {
            e.into_diagnostic(&handler).emit();
            anyhow!("Failed to parse TypeScript file: {}", file_path)
        })?;

    // Extract field definitions
    let fields = extract_fields_from_module(&module);

    Ok(fields)
}

/// Extract field definitions from a TypeScript module
fn extract_fields_from_module(module: &Module) -> Vec<FieldDefinition> {
    let mut fields = Vec::new();

    debug!("Parsing module with {} items", module.body.len());

    // Iterate through module items
    for (i, item) in module.body.iter().enumerate() {
        debug!("Processing item {}: {:?}", i, std::mem::discriminant(item));
        
        // Handle variable declarations
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item {
            debug!("Found variable declaration with {} decls", var_decl.decls.len());
            fields.extend(extract_fields_from_var_decl(var_decl));
        }
        
        // Handle export declarations
        if let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) = item {
            debug!("Found export declaration");
            if let Decl::Var(var_decl) = &export_decl.decl {
                debug!("Export contains variable declaration with {} decls", var_decl.decls.len());
                fields.extend(extract_fields_from_var_decl(var_decl));
            }
        }
    }

    debug!("Extracted {} fields total", fields.len());
    fields
}

/// Extract fields from a variable declaration
fn extract_fields_from_var_decl(var_decl: &VarDecl) -> Vec<FieldDefinition> {
    let mut fields = Vec::new();
    
    // Look for variable declarations
    for (j, decl) in var_decl.decls.iter().enumerate() {
        debug!("Processing decl {}: {:?}", j, decl.name);
        
        if let Some(init) = &decl.init {
            debug!("Decl has init expression");
            
            // Check if it's an object with a fields property
            if let Expr::Object(obj_lit) = &**init {
                debug!("Init is an object literal with {} props", obj_lit.props.len());
                
                // Look for the 'fields' property
                for prop in &obj_lit.props {
                    if let PropOrSpread::Prop(prop_box) = prop {
                        if let Prop::KeyValue(key_value) = &**prop_box {
                            if let PropName::Ident(ident) = &key_value.key {
                                let key_name = ident.sym.to_string();
                                debug!("Found property: {}", key_name);
                                
                                if key_name == "fields" {
                                    debug!("Found fields property!");
                                    
                                    // Found the fields property, extract the array
                                    if let Expr::Array(array_lit) = &*key_value.value {
                                        debug!("Fields is an array with {} elements", array_lit.elems.len());
                                        
                                        // Process each field in the array
                                        for (k, elem) in array_lit.elems.iter().enumerate() {
                                            if let Some(expr) = elem {
                                                debug!("Processing field element {}: {:?}", k, std::mem::discriminant(&*expr.expr));
                                                
                                                if let Expr::Object(obj_lit) = &*expr.expr {
                                                    debug!("Field element is an object literal");
                                                    
                                                    // Extract field definition from object literal
                                                    if let Some(field) = extract_field_from_object(obj_lit) {
                                                        debug!("Extracted field: {}", field.name);
                                                        fields.push(field);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    fields
}

/// Extract a field definition from an object literal
fn extract_field_from_object(obj: &ObjectLit) -> Option<FieldDefinition> {
    let mut name = None;
    let mut field_type = None;
    let mut required = false;
    let mut validations = Vec::new();
    let mut relationship = None;
    let mut properties = HashMap::new();

    // Process each property in the object
    for prop in &obj.props {
        if let PropOrSpread::Prop(prop_box) = prop {
            if let Prop::KeyValue(key_value) = &**prop_box {
                // Extract property key
                let key = match &key_value.key {
                    PropName::Ident(ident) => ident.sym.to_string(),
                    PropName::Str(str) => str.value.to_string(),
                    _ => continue,
                };

                // Process property value based on key
                match key.as_str() {
                    "name" => {
                        name = extract_string_value(&key_value.value);
                    },
                    "type" => {
                        field_type = extract_string_value(&key_value.value);
                    },
                    "required" => {
                        required = extract_boolean_value(&key_value.value).unwrap_or(false);
                    },
                    "validate" => {
                        // Extract validation rules
                        if let Some(validation) = extract_validation(&key_value.value) {
                            validations.push(validation);
                        }
                    },
                    "relationTo" => {
                        // Extract relationship information
                        if let Some(relation_to) = extract_string_value(&key_value.value) {
                            relationship = Some(RelationshipInfo {
                                collection: relation_to,
                                relationship_type: "hasOne".to_string(),
                            });
                        } else if let Some(relations) = extract_string_array(&key_value.value) {
                            if !relations.is_empty() {
                                relationship = Some(RelationshipInfo {
                                    collection: relations[0].clone(),
                                    relationship_type: "hasMany".to_string(),
                                });
                            }
                        }
                    },
                    "hasMany" => {
                        if let Some(has_many) = extract_boolean_value(&key_value.value) {
                            if has_many && relationship.is_some() {
                                let mut rel = relationship.unwrap();
                                rel.relationship_type = "hasMany".to_string();
                                relationship = Some(rel);
                            }
                        }
                    },
                    _ => {
                        // Store other properties
                        if let Some(value) = extract_json_value(&key_value.value) {
                            properties.insert(key, value);
                        }
                    }
                }
            }
        }
    }

    // Create field definition if name and type are present
    if let (Some(name_str), Some(type_str)) = (name, field_type) {
        Some(FieldDefinition {
            name: name_str,
            field_type: type_str,
            required,
            validations,
            relationship,
            properties,
        })
    } else {
        None
    }
}

/// Extract a string value from an expression
fn extract_string_value(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(Lit::Str(str_lit)) => Some(str_lit.value.to_string()),
        _ => None,
    }
}

/// Extract a boolean value from an expression
fn extract_boolean_value(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::Lit(Lit::Bool(bool_lit)) => Some(bool_lit.value),
        _ => None,
    }
}

/// Extract an array of strings from an expression
fn extract_string_array(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Array(array_lit) => {
            let mut strings = Vec::new();
            for elem in &array_lit.elems {
                if let Some(expr) = elem {
                    if let Expr::Lit(Lit::Str(str_lit)) = &*expr.expr {
                        strings.push(str_lit.value.to_string());
                    }
                }
            }
            Some(strings)
        },
        _ => None,
    }
}

/// Extract validation rules from an expression
fn extract_validation(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Object(obj_lit) => {
            let mut validation = json!({});

            for prop in &obj_lit.props {
                if let PropOrSpread::Prop(prop_box) = prop {
                    if let Prop::KeyValue(key_value) = &**prop_box {
                        let key = match &key_value.key {
                            PropName::Ident(ident) => ident.sym.to_string(),
                            PropName::Str(str) => str.value.to_string(),
                            _ => continue,
                        };

                        if let Some(value) = extract_json_value(&key_value.value) {
                            validation[key] = value;
                        }
                    }
                }
            }

            Some(validation)
        },
        _ => None,
    }
}

/// Extract a JSON value from an expression
fn extract_json_value(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Lit(lit) => match lit {
            Lit::Str(str_lit) => Some(Value::String(str_lit.value.to_string())),
            Lit::Bool(bool_lit) => Some(Value::Bool(bool_lit.value)),
            Lit::Num(num_lit) => Some(Value::Number(serde_json::Number::from_f64(num_lit.value)?)),
            Lit::Null(_) => Some(Value::Null),
            _ => None,
        },
        Expr::Array(array_lit) => {
            let mut values = Vec::new();
            for elem in &array_lit.elems {
                if let Some(expr) = elem {
                    if let Some(value) = extract_json_value(&expr.expr) {
                        values.push(value);
                    }
                }
            }
            Some(Value::Array(values))
        },
        Expr::Object(obj_lit) => {
            let mut map = serde_json::Map::new();
            for prop in &obj_lit.props {
                if let PropOrSpread::Prop(prop_box) = prop {
                    if let Prop::KeyValue(key_value) = &**prop_box {
                        let key = match &key_value.key {
                            PropName::Ident(ident) => ident.sym.to_string(),
                            PropName::Str(str) => str.value.to_string(),
                            _ => continue,
                        };

                        if let Some(value) = extract_json_value(&key_value.value) {
                            map.insert(key, value);
                        }
                    }
                }
            }
            Some(Value::Object(map))
        },
        _ => None,
    }
}

/// Convert field definitions to a list of field names
pub fn field_definitions_to_names(fields: &[FieldDefinition]) -> Vec<String> {
    fields.iter().map(|f| f.name.clone()).collect()
}

/// Get field type information for a field
pub fn get_field_type_info(field: &FieldDefinition) -> Value {
    let mut info = json!({
        "name": field.name,
        "type": field.field_type,
        "required": field.required,
    });

    // Add relationship information if present
    if let Some(rel) = &field.relationship {
        info["relationship"] = json!({
            "collection": rel.collection,
            "type": rel.relationship_type,
        });
    }

    // Add validations if present
    if !field.validations.is_empty() {
        info["validations"] = Value::Array(field.validations.clone());
    }

    // Add other properties
    for (key, value) in &field.properties {
        info[key] = value.clone();
    }

    info
}
