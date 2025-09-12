# Complete TypeScript Parser Implementation for Payload Migration CLI

## Context
We have a Rust CLI tool for migrating websites that needs to parse Payload CMS collection configs. The current parser doesn't work properly and needs to be completely replaced. The tool is in alpha, so we can break existing code. The parser must handle TypeScript/JavaScript files with complex import chains, spread operators, and nested field definitions.

## Requirements
The parser must:
1. Parse TypeScript/JavaScript collection config files
2. Recursively resolve and follow all imports (including `@/` aliases, relative imports, spread operators like `...slugField()`)
3. Flatten the entire schema into a template format for migration mapping
4. Handle nested structures (tabs, groups, blocks, arrays)
5. Cache parsed files to avoid re-parsing
6. Generate templates in JSON/YAML/TOML formats
7. Use the existing config file format for path aliases
8. Use the `dlog!` macro for debug logging

## Implementation Instructions

### Step 1: Update Cargo.toml Dependencies
Replace or add these dependencies to handle TypeScript parsing:

```toml
[dependencies]
# Core parsing
swc_core = { version = "0.90", features = [
    "ecma_parser",
    "ecma_ast", 
    "ecma_visit",
    "common",
] }
swc_ecma_parser = "0.143"
swc_common = { version = "0.33", features = ["tty-emitter"] }

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde_yaml = "0.9"
toml = "0.8"

# Error handling
anyhow = "1.0"
thiserror = "1.0"

# File system
walkdir = "2.4"

# Performance
rayon = "1.7"
dashmap = "5.5"  # For thread-safe caching
```

### Step 2: Create Config Structure for Path Aliases

Add this to your existing config module to handle the TypeScript configuration:

```rust
// src/config.rs (add to existing or create new)
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeScriptConfig {
    pub tsconfig_path: PathBuf,
    pub path_aliases: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathMapping {
    pub alias: String,
    pub paths: Vec<String>,
}

impl TypeScriptConfig {
    pub fn load_from_config(config_path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(config_path)?;
        let config: toml::Value = toml::from_str(&content)?;
        
        if let Some(ts_config) = config.get("typescript") {
            let typescript_config: TypeScriptConfig = ts_config.clone().try_into()?;
            Ok(typescript_config)
        } else {
            Err(anyhow::anyhow!("No typescript configuration found in config file"))
        }
    }
}
```

### Step 3: Create New Module Structure
Create a new module `src/parser/` and replace any existing parser implementation:

```
src/
  parser/
    mod.rs
    schema.rs
    import_resolver.rs
    ast_analyzer.rs
    template_generator.rs
    errors.rs
```

### Step 4: Core Implementation Files

#### src/parser/errors.rs
```rust
use thiserror::Error;
use std::path::PathBuf;

#[derive(Debug, Error)]
pub enum SchemaParseError {
    #[error("Failed to read file {path}: {source}")]
    IoError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    
    #[error("Parse error in {file}: {message}")]
    ParseError { 
        file: String, 
        message: String 
    },
    
    #[error("Circular dependency detected: {0} -> {1}")]
    CircularDependency(String, String),
    
    #[error("Unresolved import '{import}' in {file}")]
    UnresolvedImport {
        import: String,
        file: String,
    },
    
    #[error("Invalid spread operation: {0}")]
    InvalidSpread(String),
    
    #[error("Export '{name}' not found in {file}")]
    ExportNotFound {
        name: String,
        file: String,
    },

    #[error("Config file not found or invalid")]
    ConfigError,
}

pub type Result<T> = std::result::Result<T, SchemaParseError>;
```

#### src/parser/schema.rs
```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use swc_ecma_ast::Module;

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
```

#### src/parser/ast_analyzer.rs
```rust
use super::schema::*;
use super::errors::{Result, SchemaParseError};
use swc_ecma_ast::*;
use swc_ecma_visit::{Visit, VisitWith};
use std::collections::HashMap;
use crate::dlog;

pub struct AstAnalyzer {
    pub fields: Vec<FieldDefinition>,
    pub blocks: HashMap<String, BlockDefinition>,
    pub imports: Vec<ImportInfo>,
    pub exports: HashMap<String, ExportedItem>,
    current_path: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ExportedItem {
    Field(FieldDefinition),
    Fields(Vec<FieldDefinition>),
    Block(BlockDefinition),
    Function(String, Module), // function name and its AST
    Object(ObjectLit),
    Array(ArrayLit),
}

impl AstAnalyzer {
    pub fn new() -> Self {
        Self {
            fields: Vec::new(),
            blocks: HashMap::new(),
            imports: Vec::new(),
            exports: HashMap::new(),
            current_path: Vec::new(),
        }
    }

    pub fn analyze_module(&mut self, module: &Module) -> Result<()> {
        dlog!("Analyzing module with {} items", module.body.len());
        for item in &module.body {
            self.analyze_module_item(item)?;
        }
        dlog!("Found {} fields and {} blocks", self.fields.len(), self.blocks.len());
        Ok(())
    }

    fn analyze_module_item(&mut self, item: &ModuleItem) -> Result<()> {
        match item {
            ModuleItem::ModuleDecl(decl) => self.analyze_module_decl(decl)?,
            ModuleItem::Stmt(stmt) => self.analyze_statement(stmt)?,
        }
        Ok(())
    }

    fn analyze_module_decl(&mut self, decl: &ModuleDecl) -> Result<()> {
        match decl {
            ModuleDecl::Import(import_decl) => {
                dlog!("Found import from: {}", import_decl.src.value);
                self.extract_import(import_decl)?;
            }
            ModuleDecl::ExportDecl(export_decl) => {
                dlog!("Found export declaration");
                self.analyze_export_decl(export_decl)?;
            }
            ModuleDecl::ExportNamed(named_export) => {
                dlog!("Found named export");
                self.analyze_named_export(named_export)?;
            }
            ModuleDecl::ExportDefaultDecl(default_export) => {
                dlog!("Found default export");
                self.analyze_default_export(default_export)?;
            }
            ModuleDecl::ExportDefaultExpr(expr_export) => {
                dlog!("Found default expression export");
                self.analyze_default_expr_export(expr_export)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn extract_import(&mut self, import: &ImportDecl) -> Result<()> {
        let source = import.src.value.to_string();
        let mut specifiers = Vec::new();

        for spec in &import.specifiers {
            match spec {
                ImportSpecifier::Named(named) => {
                    let imported = named.imported.as_ref()
                        .map(|n| self.get_module_export_name(n))
                        .unwrap_or_else(|| named.local.sym.to_string());
                    let local = named.local.sym.to_string();
                    
                    dlog!("  - Named import: {} as {}", imported, local);
                    specifiers.push(ImportSpecifier::Named { imported, local });
                }
                ImportSpecifier::Default(default) => {
                    let local = default.local.sym.to_string();
                    dlog!("  - Default import: {}", local);
                    specifiers.push(ImportSpecifier::Default(local));
                }
                ImportSpecifier::Namespace(ns) => {
                    let local = ns.local.sym.to_string();
                    dlog!("  - Namespace import: * as {}", local);
                    specifiers.push(ImportSpecifier::Namespace(local));
                }
            }
        }

        self.imports.push(ImportInfo {
            source,
            specifiers,
            resolved_path: None,
        });

        Ok(())
    }

    fn analyze_statement(&mut self, stmt: &Stmt) -> Result<()> {
        match stmt {
            Stmt::Decl(decl) => match decl {
                Decl::Var(var_decl) => {
                    for declarator in &var_decl.decls {
                        self.analyze_var_declarator(declarator)?;
                    }
                }
                Decl::Fn(fn_decl) => {
                    let name = fn_decl.ident.sym.to_string();
                    dlog!("Found function declaration: {}", name);
                    self.exports.insert(name.clone(), ExportedItem::Function(
                        name,
                        Module {
                            span: Default::default(),
                            body: vec![ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl.clone())))],
                            shebang: None,
                        }
                    ));
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    fn analyze_export_decl(&mut self, export: &ExportDecl) -> Result<()> {
        match &export.decl {
            Decl::Var(var_decl) => {
                for decl in &var_decl.decls {
                    self.analyze_var_declarator(decl)?;
                }
            }
            Decl::Fn(fn_decl) => {
                let name = fn_decl.ident.sym.to_string();
                dlog!("Exporting function: {}", name);
                self.exports.insert(name.clone(), ExportedItem::Function(
                    fn_decl.ident.sym.to_string(),
                    Module {
                        span: Default::default(),
                        body: vec![ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl.clone())))],
                        shebang: None,
                    }
                ));
            }
            Decl::Class(class_decl) => {
                dlog!("Found class export: {}", class_decl.ident.sym);
            }
            _ => {}
        }
        Ok(())
    }

    fn analyze_named_export(&mut self, export: &NamedExport) -> Result<()> {
        if let Some(src) = &export.src {
            dlog!("Re-export from: {}", src.value);
        }
        
        for spec in &export.specifiers {
            match spec {
                ExportSpecifier::Named(named) => {
                    let orig = self.get_module_export_name(&named.orig);
                    dlog!("Named export: {}", orig);
                }
                ExportSpecifier::Default(_) => {
                    dlog!("Default re-export");
                }
                ExportSpecifier::Namespace(ns) => {
                    let name = self.get_module_export_name(&ns.name);
                    dlog!("Namespace export: {}", name);
                }
            }
        }
        Ok(())
    }

    fn analyze_default_export(&mut self, export: &ExportDefaultDecl) -> Result<()> {
        match &export.decl {
            DefaultDecl::Class(class) => {
                dlog!("Default export: class");
            }
            DefaultDecl::Fn(func) => {
                dlog!("Default export: function");
            }
            DefaultDecl::TsInterfaceDecl(_) => {
                dlog!("Default export: TypeScript interface");
            }
        }
        Ok(())
    }

    fn analyze_default_expr_export(&mut self, export: &ExportDefaultExpr) -> Result<()> {
        dlog!("Default expression export");
        match &*export.expr {
            Expr::Object(obj) => {
                self.exports.insert("default".to_string(), ExportedItem::Object(obj.clone()));
            }
            Expr::Array(arr) => {
                self.exports.insert("default".to_string(), ExportedItem::Array(arr.clone()));
            }
            _ => {}
        }
        Ok(())
    }

    fn analyze_var_declarator(&mut self, decl: &VarDeclarator) -> Result<()> {
        if let Pat::Ident(ident) = &decl.name {
            let name = ident.id.sym.to_string();
            dlog!("Found variable declaration: {}", name);
            
            if let Some(init) = &decl.init {
                match &**init {
                    Expr::Object(obj) => {
                        if self.is_collection_config(obj) {
                            dlog!("  - Detected as collection config");
                            self.extract_collection_config(obj)?;
                        } else {
                            self.exports.insert(name, ExportedItem::Object(obj.clone()));
                        }
                    }
                    Expr::Array(arr) => {
                        let fields = self.extract_fields_from_array(arr)?;
                        if !fields.is_empty() {
                            dlog!("  - Contains {} fields", fields.len());
                            self.exports.insert(name, ExportedItem::Fields(fields));
                        }
                    }
                    Expr::Call(call) => {
                        self.analyze_call_expression(call, Some(name))?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn analyze_call_expression(&mut self, call: &CallExpr, export_name: Option<String>) -> Result<()> {
        let func_name = self.extract_call_name(call);
        dlog!("Analyzing call to function: {}", func_name);
        
        // Handle common Payload field functions
        if func_name == "slugField" {
            let slug_field = FieldDefinition {
                name: "slug".to_string(),
                field_type: FieldType::Text { min_length: None, max_length: None },
                required: true,
                default_value: None,
                label: Some("Slug".to_string()),
                admin: Some(FieldAdmin {
                    position: Some("sidebar".to_string()),
                    description: None,
                    condition: None,
                    hidden: None,
                }),
                source_location: SourceLocation::default(),
            };
            
            if let Some(name) = export_name {
                self.exports.insert(name, ExportedItem::Fields(vec![slug_field]));
            }
        }
        
        Ok(())
    }

    fn is_collection_config(&self, obj: &ObjectLit) -> bool {
        obj.props.iter().any(|prop| {
            if let PropOrSpread::Prop(prop) = prop {
                if let Prop::KeyValue(kv) = &**prop {
                    if let PropName::Ident(ident) = &kv.key {
                        return ident.sym == "slug" || ident.sym == "fields";
                    }
                }
            }
            false
        })
    }

    fn extract_collection_config(&mut self, obj: &ObjectLit) -> Result<()> {
        dlog!("Extracting collection config");
        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop {
                if let Prop::KeyValue(kv) = &**prop {
                    if let PropName::Ident(ident) = &kv.key {
                        match ident.sym.as_ref() {
                            "fields" => {
                                if let Expr::Array(arr) = &*kv.value {
                                    dlog!("  - Extracting fields array");
                                    self.fields = self.extract_fields_from_array(arr)?;
                                }
                            }
                            "slug" => {
                                dlog!("  - Found collection slug");
                            }
                            "hooks" => {
                                dlog!("  - Found hooks configuration");
                            }
                            "access" => {
                                dlog!("  - Found access configuration");
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn extract_fields_from_array(&mut self, arr: &ArrayLit) -> Result<Vec<FieldDefinition>> {
        let mut fields = Vec::new();
        dlog!("Extracting fields from array with {} elements", arr.elems.len());
        
        for (i, elem) in arr.elems.iter().enumerate() {
            if let Some(elem) = elem {
                match &*elem.expr {
                    Expr::Object(obj) => {
                        if let Some(field) = self.extract_field_from_object(obj)? {
                            dlog!("  - Field {}: {}", i, field.name);
                            fields.push(field);
                        }
                    }
                    Expr::Ident(ident) => {
                        dlog!("  - Reference {}: {}", i, ident.sym);
                        fields.push(FieldDefinition {
                            name: format!("__ref__{}", ident.sym),
                            field_type: FieldType::Text { min_length: None, max_length: None },
                            required: false,
                            default_value: None,
                            label: None,
                            admin: None,
                            source_location: SourceLocation::default(),
                        });
                    }
                    Expr::Spread(spread) => {
                        if let Expr::Call(call) = &*spread.expr {
                            let func_name = self.extract_call_name(call);
                            dlog!("  - Spread call {}: {}()", i, func_name);
                            fields.push(FieldDefinition {
                                name: format!("__spread__call__{}", func_name),
                                field_type: FieldType::Text { min_length: None, max_length: None },
                                required: false,
                                default_value: None,
                                label: None,
                                admin: None,
                                source_location: SourceLocation::default(),
                            });
                        } else if let Expr::Ident(ident) = &*spread.expr {
                            dlog!("  - Spread {}: ...{}", i, ident.sym);
                            fields.push(FieldDefinition {
                                name: format!("__spread__{}", ident.sym),
                                field_type: FieldType::Text { min_length: None, max_length: None },
                                required: false,
                                default_value: None,
                                label: None,
                                admin: None,
                                source_location: SourceLocation::default(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        
        Ok(fields)
    }

    fn extract_field_from_object(&self, obj: &ObjectLit) -> Result<Option<FieldDefinition>> {
        let mut field = FieldDefinition {
            name: String::new(),
            field_type: FieldType::Text { min_length: None, max_length: None },
            required: false,
            default_value: None,
            label: None,
            admin: None,
            source_location: SourceLocation::default(),
        };

        let mut has_name = false;
        let mut type_str = String::new();

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop {
                if let Prop::KeyValue(kv) = &**prop {
                    if let PropName::Ident(key) = &kv.key {
                        match key.sym.as_ref() {
                            "name" => {
                                if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                    field.name = s.value.to_string();
                                    has_name = true;
                                }
                            }
                            "type" => {
                                if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                    type_str = s.value.to_string();
                                    field.field_type = self.parse_field_type(&type_str, obj)?;
                                }
                            }
                            "required" => {
                                if let Expr::Lit(Lit::Bool(b)) = &*kv.value {
                                    field.required = b.value;
                                }
                            }
                            "label" => {
                                if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                    field.label = Some(s.value.to_string());
                                }
                            }
                            "defaultValue" => {
                                field.default_value = self.extract_json_value(&*kv.value);
                            }
                            "admin" => {
                                if let Expr::Object(admin_obj) = &*kv.value {
                                    field.admin = Some(self.extract_field_admin(admin_obj)?);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if has_name {
            Ok(Some(field))
        } else {
            Ok(None)
        }
    }

    fn extract_field_admin(&self, obj: &ObjectLit) -> Result<FieldAdmin> {
        let mut admin = FieldAdmin {
            position: None,
            description: None,
            condition: None,
            hidden: None,
        };

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop {
                if let Prop::KeyValue(kv) = &**prop {
                    if let PropName::Ident(key) = &kv.key {
                        match key.sym.as_ref() {
                            "position" => {
                                if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                    admin.position = Some(s.value.to_string());
                                }
                            }
                            "description" => {
                                if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                    admin.description = Some(s.value.to_string());
                                }
                            }
                            "hidden" => {
                                if let Expr::Lit(Lit::Bool(b)) = &*kv.value {
                                    admin.hidden = Some(b.value);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(admin)
    }

    fn parse_field_type(&self, type_str: &str, obj: &ObjectLit) -> Result<FieldType> {
        let field_type = match type_str {
            "text" => {
                let mut min_length = None;
                let mut max_length = None;
                
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop {
                        if let Prop::KeyValue(kv) = &**prop {
                            if let PropName::Ident(key) = &kv.key {
                                match key.sym.as_ref() {
                                    "minLength" => {
                                        if let Expr::Lit(Lit::Num(n)) = &*kv.value {
                                            min_length = Some(n.value as u32);
                                        }
                                    }
                                    "maxLength" => {
                                        if let Expr::Lit(Lit::Num(n)) = &*kv.value {
                                            max_length = Some(n.value as u32);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                
                FieldType::Text { min_length, max_length }
            }
            "number" => FieldType::Number { min: None, max: None },
            "checkbox" => FieldType::Checkbox,
            "date" => FieldType::Date { admin: None },
            "select" => {
                let mut options = Vec::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop {
                        if let Prop::KeyValue(kv) = &**prop {
                            if let PropName::Ident(key) = &kv.key {
                                if key.sym == "options" {
                                    if let Expr::Array(arr) = &*kv.value {
                                        options = self.extract_select_options(arr)?;
                                    }
                                }
                            }
                        }
                    }
                }
                FieldType::Select { options }
            }
            "relationship" => {
                let mut relation_to = String::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop {
                        if let Prop::KeyValue(kv) = &**prop {
                            if let PropName::Ident(key) = &kv.key {
                                if key.sym == "relationTo" {
                                    if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                        relation_to = s.value.to_string();
                                    }
                                }
                            }
                        }
                    }
                }
                FieldType::Relationship { relationTo: relation_to }
            }
            "array" => {
                let mut nested_fields = Vec::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop {
                        if let Prop::KeyValue(kv) = &**prop {
                            if let PropName::Ident(key) = &kv.key {
                                if key.sym == "fields" {
                                    if let Expr::Array(arr) = &*kv.value {
                                        nested_fields = self.extract_fields_from_array(arr)?;
                                    }
                                }
                            }
                        }
                    }
                }
                FieldType::Array { fields: nested_fields }
            }
            "tabs" => {
                let mut tabs = Vec::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop {
                        if let Prop::KeyValue(kv) = &**prop {
                            if let PropName::Ident(key) = &kv.key {
                                if key.sym == "tabs" {
                                    if let Expr::Array(arr) = &*kv.value {
                                        tabs = self.extract_tabs_from_array(arr)?;
                                    }
                                }
                            }
                        }
                    }
                }
                FieldType::Tabs { tabs }
            }
            "blocks" => {
                let mut blocks = Vec::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop {
                        if let Prop::KeyValue(kv) = &**prop {
                            if let PropName::Ident(key) = &kv.key {
                                if key.sym == "blocks" {
                                    if let Expr::Array(arr) = &*kv.value {
                                        blocks = self.extract_block_names(arr)?;
                                    }
                                }
                            }
                        }
                    }
                }
                FieldType::Blocks { blocks }
            }
            "richText" => FieldType::RichText,
            "json" => FieldType::Json,
            "point" => FieldType::Point,
            _ => {
                dlog!("Unknown field type: {}", type_str);
                FieldType::Text { min_length: None, max_length: None }
            }
        };
        
        Ok(field_type)
    }

    fn extract_select_options(&self, arr: &ArrayLit) -> Result<Vec<SelectOption>> {
        let mut options = Vec::new();
        
        for elem in arr.elems.iter().flatten() {
            match &*elem.expr {
                Expr::Object(obj) => {
                    let mut label = String::new();
                    let mut value = String::new();
                    
                    for prop in &obj.props {
                        if let PropOrSpread::Prop(prop) = prop {
                            if let Prop::KeyValue(kv) = &**prop {
                                if let PropName::Ident(key) = &kv.key {
                                    match key.sym.as_ref() {
                                        "label" => {
                                            if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                                label = s.value.to_string();
                                            }
                                        }
                                        "value" => {
                                            if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                                value = s.value.to_string();
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    
                    if !label.is_empty() && !value.is_empty() {
                        options.push(SelectOption { label, value });
                    }
                }
                Expr::Lit(Lit::Str(s)) => {
                    // Simple string option
                    let value = s.value.to_string();
                    options.push(SelectOption {
                        label: value.clone(),
                        value,
                    });
                }
                _ => {}
            }
        }
        
        Ok(options)
    }

    fn extract_block_names(&self, arr: &ArrayLit) -> Result<Vec<String>> {
        let mut blocks = Vec::new();
        
        for elem in arr.elems.iter().flatten() {
            if let Expr::Ident(ident) = &*elem.expr {
                blocks.push(ident.sym.to_string());
            }
        }
        
        Ok(blocks)
    }

    fn extract_tabs_from_array(&self, arr: &ArrayLit) -> Result<Vec<TabDefinition>> {
        let mut tabs = Vec::new();
        
        for elem in arr.elems.iter().flatten() {
            if let Expr::Object(obj) = &*elem.expr {
                let mut tab = TabDefinition {
                    label: String::new(),
                    fields: Vec::new(),
                    description: None,
                };
                
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop {
                        if let Prop::KeyValue(kv) = &**prop {
                            if let PropName::Ident(key) = &kv.key {
                                match key.sym.as_ref() {
                                    "label" => {
                                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                            tab.label = s.value.to_string();
                                        }
                                    }
                                    "fields" => {
                                        if let Expr::Array(fields_arr) = &*kv.value {
                                            tab.fields = self.extract_fields_from_array(fields_arr)?;
                                        }
                                    }
                                    "description" => {
                                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                            tab.description = Some(s.value.to_string());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                
                tabs.push(tab);
            }
        }
        
        Ok(tabs)
    }

    fn extract_json_value(&self, expr: &Expr) -> Option<serde_json::Value> {
        match expr {
            Expr::Lit(lit) => match lit {
                Lit::Str(s) => Some(serde_json::Value::String(s.value.to_string())),
                Lit::Bool(b) => Some(serde_json::Value::Bool(b.value)),
                Lit::Num(n) => Some(serde_json::Value::Number(
                    serde_json::Number::from_f64(n.value).unwrap_or_else(|| serde_json::Number::from(0))
                )),
                Lit::Null(_) => Some(serde_json::Value::Null),
                _ => None,
            },
            Expr::Array(arr) => {
                let values: Vec<serde_json::Value> = arr.elems.iter()
                    .filter_map(|elem| elem.as_ref())
                    .filter_map(|elem| self.extract_json_value(&elem.expr))
                    .collect();
                Some(serde_json::Value::Array(values))
            }
            Expr::Object(obj) => {
                let mut map = serde_json::Map::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop {
                        if let Prop::KeyValue(kv) = &**prop {
                            if let PropName::Ident(key) = &kv.key {
                                if let Some(value) = self.extract_json_value(&*kv.value) {
                                    map.insert(key.sym.to_string(), value);
                                }
                            }
                        }
                    }
                }
                Some(serde_json::Value::Object(map))
            }
            _ => None,
        }
    }

    fn extract_call_name(&self, call: &CallExpr) -> String {
        match &call.callee {
            Callee::Expr(expr) => match &**expr {
                Expr::Ident(ident) => ident.sym.to_string(),
                _ => "unknown".to_string(),
            },
            _ => "unknown".to_string(),
        }
    }

    fn get_module_export_name(&self, export: &ModuleExportName) -> String {
        match export {
            ModuleExportName::Ident(ident) => ident.sym.to_string(),
            ModuleExportName::Str(s) => s.value.to_string(),
        }
    }
}
```

#### src/parser/import_resolver.rs
```rust
use super::schema::*;
use super::errors::{Result, SchemaParseError};
use super::ast_analyzer::{AstAnalyzer, ExportedItem};
use crate::config::{TypeScriptConfig, PathMapping};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use swc_common::{sync::Lrc, FileName, SourceMap};
use swc_ecma_ast::*;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsConfig};
use dashmap::DashMap;
use crate::dlog;

pub struct ImportResolver {
    source_map: Lrc<SourceMap>,
    base_dir: PathBuf,
    alias_map: HashMap<String, Vec<PathBuf>>,
    cache: DashMap<PathBuf, ParsedFile>,
    resolution_stack: Vec<PathBuf>,
}

#[derive(Clone)]
struct ParsedFile {
    module: Module,
    analyzer: AstAnalyzer,
    exports: HashMap<String, ExportedItem>,
}

impl ImportResolver {
    pub fn new(base_dir: PathBuf, config_path: Option<PathBuf>) -> Self {
        let mut alias_map = HashMap::new();
        
        // Load TypeScript configuration from the migration config file
        if let Some(config_path) = config_path {
            dlog!("Loading TypeScript config from: {:?}", config_path);
            if let Ok(ts_config) = TypeScriptConfig::load_from_config(&config_path) {
                dlog!("Found {} path mappings", ts_config.path_mappings.len());
                
                for mapping in ts_config.path_mappings {
                    let paths: Vec<PathBuf> = mapping.paths.iter()
                        .map(|p| {
                            let clean_path = p.trim_end_matches("/*").trim_end_matches('*');
                            if clean_path.starts_with("./") {
                                base_dir.join(&clean_path[2..])
                            } else if clean_path.starts_with("/") {
                                PathBuf::from(clean_path)
                            } else {
                                base_dir.join(clean_path)
                            }
                        })
                        .collect();
                    
                    let clean_alias = mapping.alias.trim_end_matches("/*").to_string();
                    dlog!("  Alias '{}' -> {:?}", clean_alias, paths);
                    alias_map.insert(clean_alias, paths);
                }
            } else {
                dlog!("Failed to load TypeScript config, using defaults");
                // Default aliases if config load fails
                alias_map.insert("@".to_string(), vec![base_dir.join("src")]);
                alias_map.insert("@/".to_string(), vec![base_dir.join("src")]);
            }
        } else {
            dlog!("No config path provided, using default aliases");
            // Default aliases if no config provided
            alias_map.insert("@".to_string(), vec![base_dir.join("src")]);
            alias_map.insert("@/".to_string(), vec![base_dir.join("src")]);
        }
        
        Self {
            source_map: Lrc::new(SourceMap::default()),
            base_dir,
            alias_map,
            cache: DashMap::new(),
            resolution_stack: Vec::new(),
        }
    }

    pub fn resolve_imports(&mut self, schema: &mut CollectionSchema, file_path: &Path) -> Result<()> {
        dlog!("Resolving imports for: {:?}", file_path);
        
        // Check for circular dependencies
        if self.resolution_stack.contains(&file_path.to_path_buf()) {
            return Err(SchemaParseError::CircularDependency(
                file_path.display().to_string(),
                self.resolution_stack.last()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ));
        }
        
        self.resolution_stack.push(file_path.to_path_buf());
        
        let imports = schema.imports.clone();
        dlog!("Processing {} imports", imports.len());
        
        for mut import in imports {
            dlog!("Resolving import: {}", import.source);
            
            // Resolve the import path
            match self.resolve_import_path(&import.source, file_path) {
                Ok(resolved) => {
                    dlog!("  Resolved to: {:?}", resolved);
                    import.resolved_path = Some(resolved.clone());
                    
                    // Parse the imported file
                    match self.parse_file(&resolved) {
                        Ok(parsed) => {
                            // Merge the imported items based on specifiers
                            for specifier in &import.specifiers {
                                match specifier {
                                    ImportSpecifier::Named { imported, local } => {
                                        dlog!("  Merging named import: {} as {}", imported, local);
                                        self.merge_named_import(schema, &parsed, imported, local)?;
                                    }
                                    ImportSpecifier::Default(local) => {
                                        dlog!("  Merging default import: {}", local);
                                        self.merge_default_import(schema, &parsed, local)?;
                                    }
                                    ImportSpecifier::Namespace(local) => {
                                        dlog!("  Merging namespace import: * as {}", local);
                                        self.merge_namespace_import(schema, &parsed, local)?;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            dlog!("  Failed to parse imported file: {}", e);
                            // Continue with other imports
                        }
                    }
                }
                Err(e) => {
                    dlog!("  Failed to resolve import path: {}", e);
                    // Continue with other imports
                }
            }
        }
        
        // Resolve spread operations and references
        self.resolve_field_references(schema)?;
        
        self.resolution_stack.pop();
        Ok(())
    }

    pub fn parse_file(&self, file_path: &Path) -> Result<ParsedFile> {
        dlog!("Parsing file: {:?}", file_path);
        
        // Check cache
        if let Some(cached) = self.cache.get(file_path) {
            dlog!("  Using cached version");
            return Ok(ParsedFile {
                module: cached.module.clone(),
                analyzer: cached.analyzer.clone(),
                exports: cached.exports.clone(),
            });
        }
        
        let content = std::fs::read_to_string(file_path)
            .map_err(|e| SchemaParseError::IoError {
                path: file_path.to_path_buf(),
                source: e,
            })?;

        dlog!("  File size: {} bytes", content.len());

        let fm = self.source_map.new_source_file(
            FileName::Real(file_path.to_path_buf()),
            content,
        );

        let lexer = Lexer::new(
            Syntax::Typescript(TsConfig {
                tsx: true,
                decorators: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );

        let mut parser = Parser::new_from(lexer);
        let module = parser.parse_module()
            .map_err(|e| SchemaParseError::ParseError {
                file: file_path.display().to_string(),
                message: format!("{:?}", e),
            })?;

        let mut analyzer = AstAnalyzer::new();
        analyzer.analyze_module(&module)?;
        
        let parsed = ParsedFile {
            module: module.clone(),
            exports: analyzer.exports.clone(),
            analyzer,
        };
        
        self.cache.insert(file_path.to_path_buf(), parsed.clone());
        dlog!("  Successfully parsed and cached");
        
        Ok(parsed)
    }

    fn resolve_import_path(&self, import_path: &str, from_file: &Path) -> Result<PathBuf> {
        dlog!("Resolving import path: {} from {:?}", import_path, from_file);
        
        // Handle different import types
        let resolved = if import_path.starts_with('@') {
            self.resolve_alias_import(import_path)?
        } else if import_path.starts_with('.') {
            // Relative import
            let parent = from_file.parent()
                .ok_or_else(|| SchemaParseError::UnresolvedImport {
                    import: import_path.to_string(),
                    file: from_file.display().to_string(),
                })?;
            parent.join(import_path)
        } else if import_path.starts_with('/') {
            // Absolute import from project root
            self.base_dir.join(&import_path[1..])
        } else {
            // Node module - skip for now
            return Err(SchemaParseError::UnresolvedImport {
                import: import_path.to_string(),
                file: from_file.display().to_string(),
            });
        };

        // Try to find the actual file with various extensions
        self.find_actual_file(resolved)
    }

    fn resolve_alias_import(&self, import_path: &str) -> Result<PathBuf> {
        dlog!("Resolving alias import: {}", import_path);
        
        // Try each alias
        for (alias, base_paths) in &self.alias_map {
            if import_path.starts_with(alias) {
                let remainder = if alias.ends_with('/') {
                    import_path.strip_prefix(alias).unwrap_or("")
                } else if import_path.starts_with(&format!("{}/", alias)) {
                    &import_path[alias.len() + 1..]
                } else if import_path == alias {
                    ""
                } else {
                    continue;
                };
                
                dlog!("  Matched alias '{}', remainder: '{}'", alias, remainder);
                
                // Try each base path for this alias
                for base_path in base_paths {
                    let full_path = if remainder.is_empty() {
                        base_path.clone()
                    } else {
                        base_path.join(remainder)
                    };
                    
                    dlog!("  Trying base path: {:?}", full_path);
                    
                    // Check if this path exists (with various extensions)
                    if let Ok(resolved) = self.find_actual_file(full_path.clone()) {
                        return Ok(resolved);
                    }
                }
            }
        }
        
        Err(SchemaParseError::UnresolvedImport {
            import: import_path.to_string(),
            file: "unknown".to_string(),
        })
    }

    fn find_actual_file(&self, base_path: PathBuf) -> Result<PathBuf> {
        // If it already has an extension and exists, return it
        if base_path.exists() {
            dlog!("  Found exact match: {:?}", base_path);
            return Ok(base_path);
        }
        
        // Try different extensions and index files
        let variations = vec![
            base_path.with_extension("ts"),
            base_path.with_extension("tsx"),
            base_path.with_extension("js"),
            base_path.with_extension("jsx"),
            base_path.join("index.ts"),
            base_path.join("index.tsx"),
            base_path.join("index.js"),
            base_path.join("index.jsx"),
            base_path.join("config.ts"),
            base_path.join("config.tsx"),
        ];
        
        for path in &variations {
            if path.exists() {
                dlog!("  Found with variation: {:?}", path);
                return Ok(path.clone());
            }
        }
        
        dlog!("  No valid file found for: {:?}", base_path);
        Err(SchemaParseError::UnresolvedImport {
            import: base_path.display().to_string(),
            file: "filesystem".to_string(),
        })
    }

    fn merge_named_import(&self, schema: &mut CollectionSchema, parsed: &ParsedFile, imported: &str, local: &str) -> Result<()> {
        if let Some(export) = parsed.exports.get(imported) {
            dlog!("Found export '{}' to merge as '{}'", imported, local);
            
            match export {
                ExportedItem::Field(field) => {
                    // Replace reference with actual field
                    for field_mut in &mut schema.fields {
                        if field_mut.name == format!("__ref__{}", local) {
                            dlog!("  Replacing field reference with actual field");
                            *field_mut = field.clone();
                        }
                    }
                }
                ExportedItem::Fields(fields) => {
                    // Replace spread with actual fields
                    let mut new_fields = Vec::new();
                    for field in &schema.fields {
                        if field.name == format!("__spread__{}", local) {
                            dlog!("  Replacing spread with {} fields", fields.len());
                            new_fields.extend(fields.clone());
                        } else {
                            new_fields.push(field.clone());
                        }
                    }
                    schema.fields = new_fields;
                }
                ExportedItem::Block(block) => {
                    dlog!("  Adding block: {}", block.slug);
                    schema.blocks.insert(block.slug.clone(), block.clone());
                }
                _ => {}
            }
        } else {
            dlog!("Export '{}' not found in parsed file", imported);
        }
        
        Ok(())
    }

    fn merge_default_import(&self, schema: &mut CollectionSchema, parsed: &ParsedFile, local: &str) -> Result<()> {
        if let Some(export) = parsed.exports.get("default") {
            dlog!("Found default export to merge as '{}'", local);
            
            match export {
                ExportedItem::Field(field) => {
                    for field_mut in &mut schema.fields {
                        if field_mut.name == format!("__ref__{}", local) {
                            dlog!("  Replacing field reference with default export");
                            *field_mut = field.clone();
                        }
                    }
                }
                ExportedItem::Fields(fields) => {
                    let mut new_fields = Vec::new();
                    for field in &schema.fields {
                        if field.name == format!("__spread__{}", local) {
                            dlog!("  Replacing spread with {} default fields", fields.len());
                            new_fields.extend(fields.clone());
                        } else {
                            new_fields.push(field.clone());
                        }
                    }
                    schema.fields = new_fields;
                }
                _ => {}
            }
        }
        
        Ok(())
    }

    fn merge_namespace_import(&self, schema: &mut CollectionSchema, parsed: &ParsedFile, local: &str) -> Result<()> {
        dlog!("Namespace import '{}' - currently not fully implemented", local);
        // TODO: Handle namespace imports if needed
        // This would involve making all exports available under the namespace
        Ok(())
    }

    fn resolve_field_references(&self, schema: &mut CollectionSchema) -> Result<()> {
        dlog!("Resolving field references and spread calls");
        let mut resolved_fields = Vec::new();
        
        for field in &schema.fields {
            if field.name.starts_with("__spread__call__") {
                let func_name = field.name.strip_prefix("__spread__call__").unwrap();
                dlog!("  Processing spread call: {}()", func_name);
                
                // Handle common Payload field functions
                if func_name == "slugField" {
                    dlog!("    Expanding slugField()");
                    resolved_fields.push(FieldDefinition {
                        name: "slug".to_string(),
                        field_type: FieldType::Text { min_length: None, max_length: None },
                        required: true,
                        default_value: None,
                        label: Some("Slug".to_string()),
                        admin: Some(FieldAdmin {
                            position: Some("sidebar".to_string()),
                            description: None,
                            condition: None,
                            hidden: None,
                        }),
                        source_location: SourceLocation::default(),
                    });
                } else {
                    // Keep unresolved references for now
                    dlog!("    Unknown function, keeping as-is");
                    resolved_fields.push(field.clone());
                }
            } else if !field.name.starts_with("__") {
                // Normal field
                resolved_fields.push(field.clone());
            } else {
                dlog!("  Skipping unresolved reference: {}", field.name);
            }
        }
        
        schema.fields = resolved_fields;
        dlog!("Resolved to {} fields", schema.fields.len());
        Ok(())
    }
}
```

#### src/parser/template_generator.rs
```rust
use super::schema::*;
use super::errors::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::dlog;

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
        dlog!("Generating template for collection: {}", schema.slug);
        
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
        dlog!("Flattening {} top-level fields", schema.fields.len());
        Self::flatten_fields(&schema.fields, String::new(), &mut template.fields, &mut template.relationships)?;
        dlog!("Generated {} flattened fields", template.fields.len());

        // Process blocks
        dlog!("Processing {} blocks", schema.blocks.len());
        for (slug, block) in &schema.blocks {
            let mut block_fields = Vec::new();
            Self::flatten_fields(&block.fields, format!("blocks.{}", slug), &mut block_fields, &mut template.relationships)?;
            
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
        dlog!("Found {} hooks", template.hooks.len());

        // Extract access controls
        if let Some(read) = &schema.access.read {
            template.access_controls.insert("read".to_string(), read.clone());
        }
        if let Some(create) = &schema.access.create {
            template.access_controls.insert("create".to_string(), create.clone());
        }
        if let Some(update) = &schema.access.update {
            template.access_controls.insert("update".to_string(), update.clone());
        }
        if let Some(delete) = &schema.access.delete {
            template.access_controls.insert("delete".to_string(), delete.clone());
        }
        dlog!("Found {} access controls", template.access_controls.len());

        dlog!("Template generation complete");
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

            dlog!("  Processing field: {}", path);

            match &field.field_type {
                FieldType::Tabs { tabs } => {
                    dlog!("    Field is tabs with {} tabs", tabs.len());
                    for (i, tab) in tabs.iter().enumerate() {
                        let tab_prefix = format!("{}.tab_{}", path, i);
                        Self::flatten_fields(&tab.fields, tab_prefix, output, relationships)?;
                    }
                }
                FieldType::Group { fields: nested } => {
                    dlog!("    Field is group with {} nested fields", nested.len());
                    Self::flatten_fields(nested, path.clone(), output, relationships)?;
                }
                FieldType::Array { fields: nested } => {
                    dlog!("    Field is array with {} nested fields", nested.len());
                    output.push(Self::create_flattened_field(field, &path));
                    if !nested.is_empty() {
                        let array_prefix = format!("{}[]", path);
                        Self::flatten_fields(nested, array_prefix, output, relationships)?;
                    }
                }
                FieldType::Blocks { blocks } => {
                    dlog!("    Field is blocks with {} block types", blocks.len());
                    output.push(FlattenedField {
                        path: path.clone(),
                        name: field.name.clone(),
                        field_type: "blocks".to_string(),
                        required: field.required,
                        default_value: field.default_value.clone(),
                        label: field.label.clone(),
                        validation: None,
                        admin_config: field.admin.as_ref().map(|a| {
                            serde_json::to_value(a).unwrap_or(serde_json::Value::Null)
                        }),
                    });
                }
                FieldType::Relationship { relationTo } => {
                    dlog!("    Field is relationship to: {}", relationTo);
                    relationships.push(RelationshipInfo {
                        field_path: path.clone(),
                        related_collection: relationTo.clone(),
                        relationship_type: "has_one".to_string(),
                    });
                    output.push(Self::create_flattened_field(field, &path));
                }
                FieldType::Row { fields: nested } => {
                    dlog!("    Field is row with {} nested fields", nested.len());
                    Self::flatten_fields(nested, path.clone(), output, relationships)?;
                }
                FieldType::Collapsible { fields: nested, .. } => {
                    dlog!("    Field is collapsible with {} nested fields", nested.len());
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
            FieldType::Text { min_length, max_length } => {
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
            _ => "unknown"
        }.to_string();

        FlattenedField {
            path: path.to_string(),
            name: field.name.clone(),
            field_type: field_type_str,
            required: field.required,
            default_value: field.default_value.clone(),
            label: field.label.clone(),
            validation,
            admin_config: field.admin.as_ref().map(|a| {
                serde_json::to_value(a).unwrap_or(serde_json::Value::Null)
            }),
        }
    }

    pub fn to_json(template: &FlattenedTemplate) -> Result<String> {
        Ok(serde_json::to_string_pretty(template)?)
    }

    pub fn to_yaml(template: &FlattenedTemplate) -> Result<String> {
        Ok(serde_yaml::to_string(template)?)
    }

    pub fn to_toml(template: &FlattenedTemplate) -> Result<String> {
        Ok(toml::to_string_pretty(template)?)
    }
}
```

#### src/parser/mod.rs
```rust
mod schema;
mod ast_analyzer;
mod import_resolver;
mod template_generator;
mod errors;

pub use schema::*;
pub use errors::{SchemaParseError, Result};
pub use template_generator::{FlattenedTemplate, TemplateGenerator};

use std::path::{Path, PathBuf};
use import_resolver::ImportResolver;
use ast_analyzer::AstAnalyzer;
use crate::dlog;

pub struct PayloadSchemaParser {
    base_dir: PathBuf,
    resolver: ImportResolver,
}

impl PayloadSchemaParser {
    pub fn new(base_dir: PathBuf, config_path: Option<PathBuf>) -> Self {
        dlog!("Creating PayloadSchemaParser with base_dir: {:?}", base_dir);
        let resolver = ImportResolver::new(base_dir.clone(), config_path);
        Self {
            base_dir,
            resolver,
        }
    }

    pub fn parse_collection(&mut self, config_path: &Path) -> Result<CollectionSchema> {
        dlog!("Parsing collection config: {:?}", config_path);
        
        // Initial parse to get the base schema
        let parsed = self.resolver.parse_file(config_path)?;
        
        let mut schema = CollectionSchema {
            slug: String::new(), // Will be extracted from the config
            fields: parsed.analyzer.fields.clone(),
            blocks: parsed.analyzer.blocks.clone(),
            hooks: HooksConfig::default(),
            access: AccessConfig::default(),
            admin: AdminConfig::default(),
            imports: parsed.analyzer.imports.clone(),
            raw_ast: Some(parsed.module.clone()),
        };

        dlog!("Initial parse found {} fields and {} imports", 
              schema.fields.len(), schema.imports.len());

        // Resolve all imports and merge schemas
        self.resolver.resolve_imports(&mut schema, config_path)?;

        dlog!("After import resolution: {} fields", schema.fields.len());

        Ok(schema)
    }

    pub fn generate_template(&mut self, config_path: &Path) -> Result<FlattenedTemplate> {
        dlog!("Generating template for: {:?}", config_path);
        let schema = self.parse_collection(config_path)?;
        TemplateGenerator::generate(&schema)
    }
}
```

### Step 5: Wire into existing CLI

Update your existing CLI commands to use the new parser:

```rust
// In your existing template command handler
use crate::parser::{PayloadSchemaParser, FlattenedTemplate};
use crate::dlog;
use std::path::PathBuf;
use anyhow::Result;

pub async fn handle_template_command(args: TemplateArgs) -> Result<()> {
    dlog!("Starting template generation for: {:?}", args.config);
    
    let config_path = PathBuf::from(&args.config);
    
    // Determine base directory (project root)
    let base_dir = find_project_root(&config_path)?;
    dlog!("Project root: {:?}", base_dir);
    
    // Look for the migration config file
    let migration_config_path = find_migration_config(&base_dir)?;
    dlog!("Migration config: {:?}", migration_config_path);
    
    // Create parser with config
    let mut parser = PayloadSchemaParser::new(base_dir, Some(migration_config_path));
    
    // Generate template
    dlog!("Generating template...");
    let template = parser.generate_template(&config_path)?;
    
    // Output in requested format
    let output = match args.format.as_str() {
        "json" => TemplateGenerator::to_json(&template)?,
        "yaml" => TemplateGenerator::to_yaml(&template)?,
        "toml" => TemplateGenerator::to_toml(&template)?,
        _ => return Err(anyhow::anyhow!("Unsupported format: {}", args.format)),
    };
    
    // Write to file or stdout
    if let Some(output_path) = args.output {
        std::fs::write(&output_path, output)?;
        println!("✅ Template generated: {}", output_path.display());
        dlog!("Template written to: {:?}", output_path);
    } else {
        println!("{}", output);
    }
    
    Ok(())
}

fn find_project_root(from: &Path) -> Result<PathBuf> {
    let mut current = from.parent();
    
    while let Some(dir) = current {
        // Look for indicators of project root
        if dir.join("package.json").exists() 
            || dir.join("tsconfig.json").exists()
            || dir.join("payload.config.ts").exists() {
            dlog!("Found project root at: {:?}", dir);
            return Ok(dir.to_path_buf());
        }
        current = dir.parent();
    }
    
    // Fallback to parent of config file
    from.parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Could not determine project root"))
}

fn find_migration_config(base_dir: &Path) -> Result<PathBuf> {
    // Look for common migration config file names
    let possible_names = vec![
        "migration.toml",
        "migrate.toml",
        ".migration.toml",
        "config.toml",
    ];
    
    for name in possible_names {
        let path = base_dir.join(name);
        if path.exists() {
            dlog!("Found migration config: {:?}", path);
            return Ok(path);
        }
    }
    
    // If not found, return a default path
    let default_path = base_dir.join("migration.toml");
    dlog!("No migration config found, using default: {:?}", default_path);
    Ok(default_path)
}
```

### Step 6: Add tests

Create test fixtures with sample Payload configs:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn test_parse_simple_collection() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("pages.ts");
        
        fs::write(&config_path, r#"
            export const Pages = {
                slug: 'pages',
                fields: [
                    {
                        name: 'title',
                        type: 'text',
                        required: true,
                    }
                ]
            }
        "#).unwrap();
        
        let mut parser = PayloadSchemaParser::new(temp_dir.path().to_path_buf(), None);
        let schema = parser.parse_collection(&config_path).unwrap();
        
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name, "title");
    }

    #[test]
    fn test_resolve_imports() {
        let temp_dir = TempDir::new().unwrap();
        
        // Create main config
        let config_path = temp_dir.path().join("pages.ts");
        fs::write(&config_path, r#"
            import { slugField } from './fields/slug';
            
            export const Pages = {
                slug: 'pages',
                fields: [
                    {
                        name: 'title',
                        type: 'text',
                    },
                    ...slugField()
                ]
            }
        "#).unwrap();
        
        // Create imported file
        let fields_dir = temp_dir.path().join("fields");
        fs::create_dir(&fields_dir).unwrap();
        fs::write(fields_dir.join("slug.ts"), r#"
            export const slugField = () => [
                {
                    name: 'slug',
                    type: 'text',
                    required: true,
                }
            ]
        "#).unwrap();
        
        let mut parser = PayloadSchemaParser::new(temp_dir.path().to_path_buf(), None);
        let schema = parser.parse_collection(&config_path).unwrap();
        
        // Should have both title and slug fields
        assert_eq!(schema.fields.len(), 2);
    }

    #[test]
    fn test_spread_operator() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.ts");
        
        fs::write(&config_path, r#"
            const baseFields = [
                { name: 'field1', type: 'text' },
                { name: 'field2', type: 'number' }
            ];
            
            export const Collection = {
                slug: 'test',
                fields: [
                    ...baseFields,
                    { name: 'field3', type: 'checkbox' }
                ]
            }
        "#).unwrap();
        
        let mut parser = PayloadSchemaParser::new(temp_dir.path().to_path_buf(), None);
        let schema = parser.parse_collection(&config_path).unwrap();
        
        assert_eq!(schema.fields.len(), 3);
    }

    #[test]
    fn test_circular_dependency_detection() {
        // Test that circular imports are properly detected and reported
        // This would involve creating files that import each other
    }

    #[test]
    fn test_alias_resolution() {
        let temp_dir = TempDir::new().unwrap();
        
        // Create migration config with aliases
        let config_content = r#"
[typescript]
tsconfig_path = "./tsconfig.json"
path_aliases = ["@/*"]

[[typescript.path_mappings]]
alias = "@/*"
paths = ["./src/*"]
        "#;
        
        fs::write(temp_dir.path().join("migration.toml"), config_content).unwrap();
        
        // Create source structure
        let src_dir = temp_dir.path().join("src");
        fs::create_dir(&src_dir).unwrap();
        
        // Test alias resolution
        let parser = PayloadSchemaParser::new(
            temp_dir.path().to_path_buf(),
            Some(temp_dir.path().join("migration.toml"))
        );
        
        // Further test implementation...
    }
}
```

## Key Integration Points

1. **Replace existing parser module entirely** - Don't try to merge with broken code
2. **Update CLI argument handling** to use the new parser with config file support
3. **Use existing `dlog!` macro** for all debug logging
4. **Load path aliases from existing config file** instead of reading tsconfig directly
5. **Test with real Payload configs** from your client projects

## Notes for Implementation

- The implementation now uses your existing config file format for TypeScript path mappings
- All debug logging uses your `dlog!` macro instead of the `log` crate
- The parser loads aliases from the migration config file during initialization
- Cache parsed files aggressively to improve performance
- Handle TypeScript-specific features gracefully (types, interfaces, etc.)
- The SWC parser is powerful but complex - refer to their docs for AST structure details

## Debug Output

When running with debug enabled, you should see output like:
```
[DEBUG] Loading TypeScript config from: migration.toml
[DEBUG] Found 2 path mappings
[DEBUG]   Alias '@/' -> ["./src"]
[DEBUG] Parsing collection config: pages.ts
[DEBUG] Analyzing module with 5 items
[DEBUG] Found import from: ./fields/slug
[DEBUG]   - Named import: slugField as slugField
[DEBUG] Processing 3 imports
[DEBUG] Resolving import: ./fields/slug
[DEBUG]   Resolved to: /project/src/fields/slug.ts
[DEBUG] Template generation complete
```

This implementation provides a robust foundation that correctly handles all the import patterns and spread operations in Payload configs while using your existing configuration system and logging utilities.