use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use swc_core::ecma::ast::Module;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionSchema {
    pub slug: String,
    pub fields: Vec<FieldDefinition>,
    pub blocks: HashMap<String, BlockDefinition>,
    pub hooks: HooksConfig,
    pub access: AccessConfig,
    pub admin: AdminConfig,
    #[serde(skip)]
    pub imports: Vec<ImportInfo>,
    #[serde(skip)]
    pub raw_ast: Option<Module>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDefinition {
    pub name: String,
    pub field_type: FieldType,
    pub required: bool,
    pub default_value: Option<serde_json::Value>,
    pub label: Option<String>,
    pub admin: Option<FieldAdmin>,
    #[serde(skip)]
    pub source_location: SourceLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldAdmin {
    pub position: Option<String>,
    pub description: Option<String>,
    pub condition: Option<String>,
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FieldType {
    Text { min_length: Option<u32>, max_length: Option<u32> },
    Number { min: Option<f64>, max: Option<f64> },
    Date { admin: Option<DateAdmin> },
    Checkbox,
    Select { options: Vec<SelectOption> },
    Relationship { relationTo: String },
    Array { fields: Vec<FieldDefinition> },
    Group { fields: Vec<FieldDefinition> },
    Blocks { blocks: Vec<String> },
    Tabs { tabs: Vec<TabDefinition> },
    Upload { relationTo: String },
    RichText,
    Json,
    Point,
    Row { fields: Vec<FieldDefinition> },
    Collapsible { fields: Vec<FieldDefinition>, label: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateAdmin {
    pub date: Option<DateConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateConfig {
    pub picker_appearance: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabDefinition {
    pub label: String,
    pub fields: Vec<FieldDefinition>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDefinition {
    pub slug: String,
    pub fields: Vec<FieldDefinition>,
    pub labels: BlockLabels,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockLabels {
    pub singular: Option<String>,
    pub plural: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    pub before_change: Vec<String>,
    pub after_change: Vec<String>,
    pub before_delete: Vec<String>,
    pub after_delete: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccessConfig {
    pub read: Option<String>,
    pub create: Option<String>,
    pub update: Option<String>,
    pub delete: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdminConfig {
    pub use_as_title: Option<String>,
    pub default_columns: Vec<String>,
    pub list_searchable_fields: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ImportInfo {
    pub source: String,
    pub specifiers: Vec<ImportSpecifier>,
    pub resolved_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum ImportSpecifier {
    Named { imported: String, local: String },
    Default(String),
    Namespace(String),
}

#[derive(Debug, Clone, Default)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: u32,
    pub column: u32,
}
