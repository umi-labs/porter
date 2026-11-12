use super::errors::Result;
use super::schema::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlattenedTemplate {
    pub collection_name: String,
    pub slug: String,
    pub fields: Vec<FlattenedField>,
    pub blocks: Vec<FlattenedBlock>,
    pub relationships: Vec<RelationshipInfo>,
    pub hooks: Vec<String>,
    pub access_controls: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlattenedField {
    pub path: String,
    pub name: String,
    pub field_type: String,
    pub required: bool,
    pub default_value: Option<serde_json::Value>,
    pub label: Option<String>,
    pub validation: Option<ValidationRules>,
    pub admin_config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlattenedBlock {
    pub slug: String,
    pub fields: Vec<FlattenedField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipInfo {
    pub field_path: String,
    pub related_collection: String,
    pub relationship_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRules {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub min_length: Option<u32>,
    pub max_length: Option<u32>,
    pub pattern: Option<String>,
}

pub struct TemplateGenerator;

impl TemplateGenerator {
    pub fn generate(schema: &CollectionSchema) -> Result<FlattenedTemplate> {
        let mut template = FlattenedTemplate {
            collection_name: schema.slug.clone(),
            slug: schema.slug.clone(),
            fields: Vec::new(),
            blocks: Vec::new(),
            relationships: Vec::new(),
            hooks: Vec::new(),
            access_controls: HashMap::new(),
        };

        // Flatten fields recursively
        Self::flatten_fields(
            &schema.fields,
            String::new(),
            &mut template.fields,
            &mut template.relationships,
        )?;

        // Process blocks
        for (slug, block) in &schema.blocks {
            let mut block_fields = Vec::new();
            Self::flatten_fields(
                &block.fields,
                format!("blocks.{}", slug),
                &mut block_fields,
                &mut template.relationships,
            )?;

            template.blocks.push(FlattenedBlock {
                slug: slug.clone(),
                fields: block_fields,
            });
        }

        // Extract hooks
        template.hooks.extend(schema.hooks.before_change.clone());
        template.hooks.extend(schema.hooks.after_change.clone());
        template.hooks.extend(schema.hooks.before_delete.clone());
        template.hooks.extend(schema.hooks.after_delete.clone());

        // Extract access controls
        if let Some(read) = &schema.access.read {
            template
                .access_controls
                .insert("read".to_string(), read.clone());
        }
        if let Some(create) = &schema.access.create {
            template
                .access_controls
                .insert("create".to_string(), create.clone());
        }
        if let Some(update) = &schema.access.update {
            template
                .access_controls
                .insert("update".to_string(), update.clone());
        }
        if let Some(delete) = &schema.access.delete {
            template
                .access_controls
                .insert("delete".to_string(), delete.clone());
        }

        Ok(template)
    }

    fn flatten_fields(
        fields: &[FieldDefinition],
        prefix: String,
        output: &mut Vec<FlattenedField>,
        relationships: &mut Vec<RelationshipInfo>,
    ) -> Result<()> {
        for field in fields {
            let path = if prefix.is_empty() {
                field.name.clone()
            } else {
                format!("{}.{}", prefix, field.name)
            };

            match &field.field_type {
                FieldType::Tabs { tabs } => {
                    for (i, tab) in tabs.iter().enumerate() {
                        let tab_prefix = format!("{}.tab_{}", path, i);
                        Self::flatten_fields(&tab.fields, tab_prefix, output, relationships)?;
                    }
                }
                FieldType::Group { fields: nested } => {
                    Self::flatten_fields(nested, path.clone(), output, relationships)?;
                }
                FieldType::Array { fields: nested } => {
                    output.push(Self::create_flattened_field(field, &path));
                    if !nested.is_empty() {
                        let array_prefix = format!("{}[]", path);
                        Self::flatten_fields(nested, array_prefix, output, relationships)?;
                    }
                }
                FieldType::Blocks { blocks: _ } => {
                    output.push(FlattenedField {
                        path: path.clone(),
                        name: field.name.clone(),
                        field_type: "blocks".to_string(),
                        required: field.required,
                        default_value: field.default_value.clone(),
                        label: field.label.clone(),
                        validation: None,
                        admin_config: field
                            .admin
                            .as_ref()
                            .map(|a| serde_json::to_value(a).unwrap_or(serde_json::Value::Null)),
                    });
                }
                FieldType::Relationship { relationTo } => {
                    relationships.push(RelationshipInfo {
                        field_path: path.clone(),
                        related_collection: relationTo.clone(),
                        relationship_type: "has_one".to_string(),
                    });
                    output.push(Self::create_flattened_field(field, &path));
                }
                FieldType::Row { fields: nested } => {
                    Self::flatten_fields(nested, path.clone(), output, relationships)?;
                }
                FieldType::Collapsible { fields: nested, .. } => {
                    Self::flatten_fields(nested, path.clone(), output, relationships)?;
                }
                _ => {
                    output.push(Self::create_flattened_field(field, &path));
                }
            }
        }

        Ok(())
    }

    fn create_flattened_field(field: &FieldDefinition, path: &str) -> FlattenedField {
        let mut validation = None;

        let field_type_str = match &field.field_type {
            FieldType::Text {
                min_length,
                max_length,
            } => {
                validation = Some(ValidationRules {
                    min: None,
                    max: None,
                    min_length: *min_length,
                    max_length: *max_length,
                    pattern: None,
                });
                "text"
            }
            FieldType::Number { min, max } => {
                validation = Some(ValidationRules {
                    min: *min,
                    max: *max,
                    min_length: None,
                    max_length: None,
                    pattern: None,
                });
                "number"
            }
            FieldType::Date { .. } => "date",
            FieldType::Checkbox => "checkbox",
            FieldType::Select { .. } => "select",
            FieldType::Relationship { .. } => "relationship",
            FieldType::Array { .. } => "array",
            FieldType::Upload { .. } => "upload",
            FieldType::RichText => "richText",
            FieldType::Json => "json",
            FieldType::Point => "point",
            _ => "unknown",
        }
        .to_string();

        FlattenedField {
            path: path.to_string(),
            name: field.name.clone(),
            field_type: field_type_str,
            required: field.required,
            default_value: field.default_value.clone(),
            label: field.label.clone(),
            validation,
            admin_config: field
                .admin
                .as_ref()
                .map(|a| serde_json::to_value(a).unwrap_or(serde_json::Value::Null)),
        }
    }

    pub fn to_json(template: &FlattenedTemplate) -> Result<String> {
        serde_json::to_string_pretty(template).map_err(|e| {
            super::errors::SchemaParseError::ParseError {
                file: "template".to_string(),
                message: e.to_string(),
            }
        })
    }

    pub fn to_toml(template: &FlattenedTemplate) -> Result<String> {
        toml::to_string_pretty(template).map_err(|e| super::errors::SchemaParseError::ParseError {
            file: "template".to_string(),
            message: e.to_string(),
        })
    }
}
