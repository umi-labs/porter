use anyhow::{Result, anyhow};
use serde_json::Value;
use std::collections::HashMap;

use super::json_mapping::{Mapping, FieldMapping};

/// Validation result for a mapping
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

/// Validation error
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
    pub severity: ErrorSeverity,
}

/// Validation warning
#[derive(Debug, Clone)]
pub struct ValidationWarning {
    pub field: String,
    pub message: String,
    pub suggestion: Option<String>,
}

/// Error severity levels
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorSeverity {
    Error,
    Warning,
    Info,
}

/// Schema information for validation
#[derive(Debug, Clone)]
pub struct SchemaInfo {
    pub fields: HashMap<String, FieldInfo>,
    pub required_fields: Vec<String>,
}

/// Field information for validation
#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub field_type: String,
    pub required: bool,
    pub validations: Vec<Value>,
    pub relationship: Option<RelationshipInfo>,
}

/// Relationship information
#[derive(Debug, Clone)]
pub struct RelationshipInfo {
    pub collection: String,
    pub relationship_type: String,
}

/// Validates a mapping against source and target schemas
pub fn validate_mapping(
    mapping: &Mapping,
    source_schema: &SchemaInfo,
    target_schema: &SchemaInfo,
) -> ValidationResult {
    let mut result = ValidationResult {
        is_valid: true,
        errors: Vec::new(),
        warnings: Vec::new(),
    };

    // Validate each field mapping
    for field_mapping in &mapping.field_mappings {
        validate_field_mapping(field_mapping, source_schema, target_schema, &mut result);
    }

    // Check for missing required fields
    validate_required_fields(mapping, target_schema, &mut result);

    // Check for unmapped source fields
    validate_unmapped_source_fields(mapping, source_schema, &mut result);

    result.is_valid = result.errors.is_empty();
    result
}

/// Validates a single field mapping
fn validate_field_mapping(
    field_mapping: &FieldMapping,
    source_schema: &SchemaInfo,
    target_schema: &SchemaInfo,
    result: &mut ValidationResult,
) {
    let target_field = &field_mapping.to;
    let source_field = extract_source_field_name(&field_mapping.from);

    // Check if target field exists in target schema
    if let Some(target_field_info) = target_schema.fields.get(target_field) {
        // Check if source field exists in source schema
        if let Some(source_field_info) = source_schema.fields.get(&source_field) {
            // Validate type compatibility
            validate_type_compatibility(
                source_field_info,
                target_field_info,
                &source_field,
                target_field,
                result,
            );
        } else {
            result.errors.push(ValidationError {
                field: source_field.clone(),
                message: format!("Source field '{}' does not exist in source schema", source_field),
                severity: ErrorSeverity::Error,
            });
        }
    } else {
        result.errors.push(ValidationError {
            field: target_field.clone(),
            message: format!("Target field '{}' does not exist in target schema", target_field),
            severity: ErrorSeverity::Error,
        });
    }

    // Validate transforms
    validate_transforms(field_mapping, source_schema, target_schema, result);
}

/// Extracts the source field name from the 'from' field
fn extract_source_field_name(from: &Value) -> String {
    match from {
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            if let Some(Value::String(s)) = arr.first() {
                s.clone()
            } else {
                "unknown".to_string()
            }
        }
        _ => "unknown".to_string(),
    }
}

/// Validates type compatibility between source and target fields
fn validate_type_compatibility(
    source_field_info: &FieldInfo,
    target_field_info: &FieldInfo,
    source_field: &str,
    target_field: &str,
    result: &mut ValidationResult,
) {
    let source_type = &source_field_info.field_type;
    let target_type = &target_field_info.field_type;

    // Check if types are compatible
    if !are_types_compatible(source_type, target_type) {
        result.warnings.push(ValidationWarning {
            field: target_field.to_string(),
            message: format!(
                "Type mismatch: source field '{}' is '{}' but target field '{}' expects '{}'",
                source_field, source_type, target_field, target_type
            ),
            suggestion: Some(format!(
                "Consider adding a transform to convert {} to {}",
                source_type, target_type
            )),
        });
    }
}

/// Checks if two types are compatible
fn are_types_compatible(source_type: &str, target_type: &str) -> bool {
    match (source_type.to_lowercase().as_str(), target_type.to_lowercase().as_str()) {
        // String types are compatible
        ("string", "text") | ("text", "string") => true,
        ("string", "textarea") | ("textarea", "string") => true,
        ("string", "email") | ("email", "string") => true,
        ("string", "url") | ("url", "string") => true,
        
        // Number types are compatible
        ("number", "integer") | ("integer", "number") => true,
        ("number", "float") | ("float", "number") => true,
        ("integer", "float") | ("float", "integer") => true,
        
        // Boolean types are compatible
        ("boolean", "checkbox") | ("checkbox", "boolean") => true,
        
        // Array types are compatible
        ("array", "select") | ("select", "array") => true,
        ("array", "relationship") | ("relationship", "array") => true,
        
        // Same types are always compatible
        (a, b) if a == b => true,
        
        // Default: not compatible
        _ => false,
    }
}

/// Validates transforms for a field mapping
fn validate_transforms(
    field_mapping: &FieldMapping,
    _source_schema: &SchemaInfo,
    _target_schema: &SchemaInfo,
    result: &mut ValidationResult,
) {
    for transform in &field_mapping.transforms {
        if let Some(transform_type) = transform.get("type").and_then(Value::as_str) {
            match transform_type {
                "uppercase" | "lowercase" => {
                    // These transforms are always valid for string fields
                }
                "split_comma" => {
                    // This transform is always valid
                }
                "to_point" => {
                    // Validate to_point transform
                    validate_to_point_transform(transform, result);
                }
                "combine_coordinates" => {
                    // Validate combine_coordinates transform
                    validate_combine_coordinates_transform(transform, result);
                }
                _ => {
                    result.warnings.push(ValidationWarning {
                        field: field_mapping.to.clone(),
                        message: format!("Unknown transform type: {}", transform_type),
                        suggestion: Some("Consider using a supported transform type".to_string()),
                    });
                }
            }
        }
    }
}

/// Validates the to_point transform
fn validate_to_point_transform(transform: &Value, result: &mut ValidationResult) {
    if let Some(params) = transform.get("params") {
        let lat_path = params.get("lat").and_then(Value::as_str);
        let lng_path = params.get("lng").and_then(Value::as_str);

        if lat_path.is_none() || lng_path.is_none() {
            result.errors.push(ValidationError {
                field: "to_point".to_string(),
                message: "to_point transform requires 'lat' and 'lng' parameters".to_string(),
                severity: ErrorSeverity::Error,
            });
        }
    } else {
        result.errors.push(ValidationError {
            field: "to_point".to_string(),
            message: "to_point transform requires 'params' object".to_string(),
            severity: ErrorSeverity::Error,
        });
    }
}

/// Validates the combine_coordinates transform
fn validate_combine_coordinates_transform(transform: &Value, result: &mut ValidationResult) {
    if let Some(params) = transform.get("params") {
        let lat_field = params.get("lat_field").and_then(Value::as_str);
        let lng_field = params.get("lng_field").and_then(Value::as_str);

        if lat_field.is_none() || lng_field.is_none() {
            result.errors.push(ValidationError {
                field: "combine_coordinates".to_string(),
                message: "combine_coordinates transform requires 'lat_field' and 'lng_field' parameters".to_string(),
                severity: ErrorSeverity::Error,
            });
        }
    } else {
        result.errors.push(ValidationError {
            field: "combine_coordinates".to_string(),
            message: "combine_coordinates transform requires 'params' object".to_string(),
            severity: ErrorSeverity::Error,
        });
    }
}

/// Validates that all required target fields are mapped
fn validate_required_fields(
    mapping: &Mapping,
    target_schema: &SchemaInfo,
    result: &mut ValidationResult,
) {
    let mapped_fields: std::collections::HashSet<_> = mapping
        .field_mappings
        .iter()
        .map(|fm| fm.to.clone())
        .collect();

    for required_field in &target_schema.required_fields {
        if !mapped_fields.contains(required_field) {
            result.errors.push(ValidationError {
                field: required_field.clone(),
                message: format!("Required field '{}' is not mapped", required_field),
                severity: ErrorSeverity::Error,
            });
        }
    }
}

/// Validates that source fields are not unnecessarily unmapped
fn validate_unmapped_source_fields(
    mapping: &Mapping,
    source_schema: &SchemaInfo,
    result: &mut ValidationResult,
) {
    let mapped_source_fields: std::collections::HashSet<_> = mapping
        .field_mappings
        .iter()
        .map(|fm| extract_source_field_name(&fm.from))
        .collect();

    for (source_field, _field_info) in &source_schema.fields {
        if !mapped_source_fields.contains(source_field) {
            result.warnings.push(ValidationWarning {
                field: source_field.clone(),
                message: format!("Source field '{}' is not mapped", source_field),
                suggestion: Some(format!(
                    "Consider mapping '{}' to a target field if needed",
                    source_field
                )),
            });
        }
    }
}

/// Creates a schema info from a collection of documents
pub fn create_schema_from_documents(documents: &[Value]) -> Result<SchemaInfo> {
    if documents.is_empty() {
        return Err(anyhow!("Cannot create schema from empty document collection"));
    }

    let mut fields = HashMap::new();
    let mut required_fields = Vec::new();

    // Analyze the first document to determine field types
    let first_doc = &documents[0];
    if let Some(obj) = first_doc.as_object() {
        for (field_name, value) in obj {
            let field_type = infer_field_type(value);
            let required = is_field_required(field_name, documents);

            fields.insert(
                field_name.clone(),
                FieldInfo {
                    field_type,
                    required,
                    validations: Vec::new(),
                    relationship: None,
                },
            );

            if required {
                required_fields.push(field_name.clone());
            }
        }
    }

    Ok(SchemaInfo {
        fields,
        required_fields,
    })
}

/// Infers the type of a field from its value
fn infer_field_type(value: &Value) -> String {
    match value {
        Value::String(_) => "string".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::Bool(_) => "boolean".to_string(),
        Value::Array(_) => "array".to_string(),
        Value::Object(_) => "object".to_string(),
        Value::Null => "null".to_string(),
    }
}

/// Determines if a field is required based on its presence across all documents
fn is_field_required(field_name: &str, documents: &[Value]) -> bool {
    documents.iter().all(|doc| {
        doc.get(field_name).is_some() && !doc.get(field_name).unwrap().is_null()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_type_compatibility() {
        assert!(are_types_compatible("string", "text"));
        assert!(are_types_compatible("number", "integer"));
        assert!(are_types_compatible("boolean", "checkbox"));
        assert!(are_types_compatible("array", "select"));
        assert!(are_types_compatible("string", "string"));
        
        assert!(!are_types_compatible("string", "number"));
        assert!(!are_types_compatible("boolean", "string"));
        assert!(!are_types_compatible("array", "number"));
    }

    #[test]
    fn test_extract_source_field_name() {
        assert_eq!(extract_source_field_name(&json!("fieldName")), "fieldName");
        assert_eq!(
            extract_source_field_name(&json!(["field1", "field2"])),
            "field1"
        );
        assert_eq!(extract_source_field_name(&json!(null)), "unknown");
    }

    #[test]
    fn test_create_schema_from_documents() -> Result<()> {
        let documents = vec![
            json!({
                "name": "Hotel 1",
                "rating": 5,
                "active": true,
                "tags": ["luxury", "spa"]
            }),
            json!({
                "name": "Hotel 2",
                "rating": 4,
                "active": false,
                "tags": ["budget"]
            }),
        ];

        let schema = create_schema_from_documents(&documents)?;

        assert!(schema.fields.contains_key("name"));
        assert!(schema.fields.contains_key("rating"));
        assert!(schema.fields.contains_key("active"));
        assert!(schema.fields.contains_key("tags"));

        assert_eq!(schema.fields["name"].field_type, "string");
        assert_eq!(schema.fields["rating"].field_type, "number");
        assert_eq!(schema.fields["active"].field_type, "boolean");
        assert_eq!(schema.fields["tags"].field_type, "array");

        // All fields should be required since they appear in all documents
        assert_eq!(schema.required_fields.len(), 4);

        Ok(())
    }

    #[test]
    fn test_validate_mapping() -> Result<()> {
        let source_docs = vec![
            json!({
                "nodeName": "Hotel 1",
                "rating": 5,
                "active": true
            }),
        ];

        let source_schema = create_schema_from_documents(&source_docs)?;

        let target_schema = SchemaInfo {
            fields: {
                let mut map = HashMap::new();
                map.insert(
                    "title".to_string(),
                    FieldInfo {
                        field_type: "string".to_string(),
                        required: true,
                        validations: Vec::new(),
                        relationship: None,
                    },
                );
                map.insert(
                    "rating".to_string(),
                    FieldInfo {
                        field_type: "number".to_string(),
                        required: false,
                        validations: Vec::new(),
                        relationship: None,
                    },
                );
                map
            },
            required_fields: vec!["title".to_string()],
        };

        let mapping = Mapping {
            source: "umbraco".to_string(),
            target: "payload".to_string(),
            collection: "hotels".to_string(),
            field_mappings: vec![
                FieldMapping {
                    to: "title".to_string(),
                    from: json!("nodeName"),
                    transforms: Vec::new(),
                    fallback: None,
                },
                FieldMapping {
                    to: "rating".to_string(),
                    from: json!("rating"),
                    transforms: Vec::new(),
                    fallback: None,
                },
            ],
            block_mappings: None,
        };

        let result = validate_mapping(&mapping, &source_schema, &target_schema);

        assert!(result.is_valid);
        assert!(result.errors.is_empty());
        // Should have a warning about unmapped source field 'active'
        assert!(!result.warnings.is_empty());

        Ok(())
    }
}
