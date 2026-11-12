use super::ast_analyzer::{AstAnalyzer, ExportedItem};
use super::errors::{Result, SchemaParseError};
use super::schema::{
    CollectionSchema, FieldAdmin, FieldDefinition, FieldType, ImportSpecifier, SourceLocation,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use swc_common::{FileName, SourceMap, Spanned, sync::Lrc};
use swc_ecma_ast::*;
use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

/// One tsconfig `paths` entry: `alias` like `@/*`, `paths` relative to the base dir.
#[derive(Debug, Clone)]
pub struct PathAlias {
    pub alias: String,
    pub paths: Vec<String>,
}

/// Reads and parses one TypeScript file (TSX when the extension says so).
pub fn parse_ts(cm: &Lrc<SourceMap>, path: &Path) -> Result<Module> {
    let src = std::fs::read_to_string(path).map_err(|e| SchemaParseError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let tsx = path.extension().is_some_and(|e| e == "tsx");
    parse_ts_str(cm, FileName::Real(path.to_path_buf()), src, tsx)
}

/// Parses TypeScript source. Recoverable syntax errors count as errors too.
pub fn parse_ts_str(cm: &Lrc<SourceMap>, name: FileName, src: String, tsx: bool) -> Result<Module> {
    let file = name.to_string();
    let fm = cm.new_source_file(Lrc::new(name), src);
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx,
            decorators: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let fail = |e: swc_ecma_parser::error::Error| SchemaParseError::ParseError {
        file: file.clone(),
        message: format!(
            "line {}: {}",
            cm.lookup_char_pos(e.span().lo).line,
            e.kind().msg()
        ),
    };
    let module = parser.parse_module().map_err(fail)?;
    if let Some(e) = parser.take_errors().into_iter().next() {
        return Err(fail(e));
    }
    Ok(module)
}

pub struct ImportResolver {
    source_map: Lrc<SourceMap>,
    base_dir: PathBuf,
    alias_map: HashMap<String, Vec<PathBuf>>,
    cache: HashMap<PathBuf, ParsedFile>,
    resolution_stack: Vec<PathBuf>,
}

#[derive(Clone)]
pub struct ParsedFile {
    pub module: Module,
    pub analyzer: AstAnalyzer,
    pub exports: HashMap<String, ExportedItem>,
}

impl ImportResolver {
    /// `aliases` are tsconfig `paths` (see [`PathAlias`]); `None` means the default `@/` -> `src/`.
    pub fn new(base_dir: PathBuf, aliases: Option<&[PathAlias]>) -> Self {
        let mut alias_map = HashMap::new();
        match aliases {
            Some(list) => {
                for a in list {
                    let paths = a
                        .paths
                        .iter()
                        .map(|p| base_dir.join(p.trim_end_matches("/*").trim_end_matches('*')))
                        .collect();
                    alias_map.insert(a.alias.trim_end_matches("/*").to_string(), paths);
                }
            }
            None => {
                alias_map.insert("@/".to_string(), vec![base_dir.join("src")]);
            }
        }
        Self {
            source_map: Lrc::new(SourceMap::default()),
            base_dir,
            alias_map,
            cache: HashMap::new(),
            resolution_stack: Vec::new(),
        }
    }

    pub fn resolve_imports(
        &mut self,
        schema: &mut CollectionSchema,
        file_path: &Path,
    ) -> Result<()> {
        // Check for circular dependencies
        if self.resolution_stack.contains(&file_path.to_path_buf()) {
            return Err(SchemaParseError::CircularDependency(
                file_path.display().to_string(),
                self.resolution_stack
                    .last()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ));
        }

        self.resolution_stack.push(file_path.to_path_buf());

        let imports = schema.imports.clone();

        for mut import in imports {
            // Resolve the import path
            match self.resolve_import_path(&import.source, file_path) {
                Ok(resolved) => {
                    import.resolved_path = Some(resolved.clone());

                    // Parse the imported file
                    match self.parse_file(&resolved) {
                        Ok(parsed) => {
                            // Merge the imported items based on specifiers
                            for specifier in &import.specifiers {
                                match specifier {
                                    ImportSpecifier::Named { imported, local } => {
                                        self.merge_named_import(schema, &parsed, imported, local)?;
                                    }
                                    ImportSpecifier::Default(local) => {
                                        self.merge_default_import(schema, &parsed, local)?;
                                    }
                                    ImportSpecifier::Namespace(local) => {
                                        self.merge_namespace_import(schema, &parsed, local)?;
                                    }
                                }
                            }
                        }
                        Err(_e) => {
                            // Continue with other imports
                        }
                    }
                }
                Err(_e) => {
                    // Continue with other imports
                }
            }
        }

        // Resolve spread operations and references
        self.resolve_field_references(schema)?;

        self.resolution_stack.pop();
        Ok(())
    }

    pub fn parse_file(&mut self, file_path: &Path) -> Result<ParsedFile> {
        // Check cache
        if let Some(cached) = self.cache.get(file_path) {
            return Ok(ParsedFile {
                module: cached.module.clone(),
                analyzer: cached.analyzer.clone(),
                exports: cached.exports.clone(),
            });
        }

        let module = parse_ts(&self.source_map, file_path)?;
        let mut analyzer = AstAnalyzer::new();
        analyzer.analyze_module(&module)?;

        let parsed = ParsedFile {
            module: module.clone(),
            exports: analyzer.exports.clone(),
            analyzer,
        };

        self.cache.insert(file_path.to_path_buf(), parsed.clone());

        Ok(parsed)
    }

    pub fn resolve_import_path(&self, import_path: &str, from_file: &Path) -> Result<PathBuf> {
        // Handle different import types
        let resolved = if import_path.starts_with('.') {
            let parent = from_file
                .parent()
                .ok_or_else(|| SchemaParseError::UnresolvedImport {
                    import: import_path.to_string(),
                    file: from_file.display().to_string(),
                })?;
            parent.join(import_path)
        } else if let Some(rest) = import_path.strip_prefix('/') {
            self.base_dir.join(rest)
        } else {
            // tsconfig path alias; anything else is a node module, which isn't followed.
            return self.resolve_alias_import(import_path);
        };

        // Try to find the actual file with various extensions
        self.find_actual_file(resolved)
    }

    fn resolve_alias_import(&self, import_path: &str) -> Result<PathBuf> {
        // Try each alias - prioritize longer matches first
        let mut sorted_aliases: Vec<_> = self.alias_map.keys().collect();
        sorted_aliases.sort_by_key(|a| std::cmp::Reverse(a.len()));

        for alias in sorted_aliases {
            let base_paths = &self.alias_map[alias];

            // Check if this import matches the alias
            let remainder = if alias.ends_with('/') {
                // Alias ends with /, so we need exact prefix match
                if import_path.starts_with(alias) {
                    import_path.strip_prefix(alias).unwrap_or("")
                } else {
                    continue;
                }
            } else {
                // Alias doesn't end with /, so we need to match either:
                // 1. Exact match: import_path == alias
                // 2. Followed by /: import_path starts with alias + "/"
                if import_path == alias {
                    ""
                } else if import_path.starts_with(&format!("{}/", alias)) {
                    &import_path[alias.len() + 1..]
                } else {
                    continue;
                }
            };

            // Try each base path for this alias
            for base_path in base_paths {
                let full_path = if remainder.is_empty() {
                    base_path.clone()
                } else {
                    base_path.join(remainder)
                };

                // Check if this path exists (with various extensions)
                if let Ok(resolved) = self.find_actual_file(full_path.clone()) {
                    return Ok(resolved);
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
            // Check if it's a directory
            if base_path.is_dir() {
                // Try to find index files in the directory
                let index_variations = vec![
                    base_path.join("index.ts"),
                    base_path.join("index.tsx"),
                    base_path.join("index.js"),
                    base_path.join("index.jsx"),
                ];

                for path in &index_variations {
                    if path.exists() {
                        return Ok(path.clone());
                    }
                }

                return Err(SchemaParseError::UnresolvedImport {
                    import: base_path.display().to_string(),
                    file: "filesystem".to_string(),
                });
            } else {
                return Ok(base_path);
            }
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
                return Ok(path.clone());
            }
        }

        Err(SchemaParseError::UnresolvedImport {
            import: base_path.display().to_string(),
            file: "filesystem".to_string(),
        })
    }

    fn merge_named_import(
        &self,
        schema: &mut CollectionSchema,
        parsed: &ParsedFile,
        imported: &str,
        local: &str,
    ) -> Result<()> {
        if let Some(export) = parsed.exports.get(imported) {
            match export {
                ExportedItem::Field(field) => {
                    // Replace reference with actual field
                    for field_mut in &mut schema.fields {
                        if field_mut.name == format!("__ref__{}", local) {
                            *field_mut = field.clone();
                        }
                    }
                }
                ExportedItem::Fields(fields) => {
                    // Replace spread with actual fields
                    let mut new_fields = Vec::new();
                    for field in &schema.fields {
                        if field.name == format!("__spread__{}", local) {
                            new_fields.extend(fields.clone());
                        } else {
                            new_fields.push(field.clone());
                        }
                    }
                    schema.fields = new_fields;
                }
                ExportedItem::Block(block) => {
                    schema.blocks.insert(block.slug.clone(), block.clone());
                }
                ExportedItem::Object(obj)
                    // Check if this object represents a field config
                    if self.is_field_config_object(obj) => {
                        if let Ok(field_def) = self.extract_field_from_object(obj, local) {
                            let target_ref = format!("__ref__{}", local);

                            // Debug: list all current field names
                            for _field in schema.fields.iter() {
                            }

                            // Replace field reference with the extracted field
                            let mut found = false;
                            for field_mut in &mut schema.fields {
                                if field_mut.name == target_ref {
                                    *field_mut = field_def.clone();
                                    found = true;
                                    break;
                                }
                            }

                            if !found {
                            }
                        }
                    }
                _ => {}
            }
        }

        Ok(())
    }

    fn merge_default_import(
        &self,
        schema: &mut CollectionSchema,
        parsed: &ParsedFile,
        local: &str,
    ) -> Result<()> {
        if let Some(export) = parsed.exports.get("default") {
            match export {
                ExportedItem::Field(field) => {
                    for field_mut in &mut schema.fields {
                        if field_mut.name == format!("__ref__{}", local) {
                            *field_mut = field.clone();
                        }
                    }
                }
                ExportedItem::Fields(fields) => {
                    let mut new_fields = Vec::new();
                    for field in &schema.fields {
                        if field.name == format!("__spread__{}", local) {
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

    fn merge_namespace_import(
        &self,
        _schema: &mut CollectionSchema,
        _parsed: &ParsedFile,
        _local: &str,
    ) -> Result<()> {
        // TODO: Handle namespace imports if needed
        // This would involve making all exports available under the namespace
        Ok(())
    }

    fn resolve_field_references(&self, schema: &mut CollectionSchema) -> Result<()> {
        let mut resolved_fields = Vec::new();

        for field in &schema.fields {
            if let Some(func_name) = field.name.strip_prefix("__spread__call__") {
                // Handle common Payload field functions
                if func_name == "slugField" {
                    resolved_fields.push(FieldDefinition {
                        name: "slug".to_string(),
                        field_type: FieldType::Text {
                            min_length: None,
                            max_length: None,
                        },
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
                    resolved_fields.push(field.clone());
                }
            } else if !field.name.starts_with("__") {
                // Normal field
                resolved_fields.push(field.clone());
            }
        }

        schema.fields = resolved_fields;
        Ok(())
    }

    /// Check if an ObjectLit represents a field config (has name and type properties)
    fn is_field_config_object(&self, obj: &swc_ecma_ast::ObjectLit) -> bool {
        use swc_ecma_ast::*;

        let mut has_name = false;
        let mut has_type = false;

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(ident) = &kv.key
            {
                match ident.sym.as_ref() {
                    "name" => has_name = true,
                    "type" => has_type = true,
                    _ => {}
                }
            }
        }

        has_name && has_type
    }

    /// Extract a field definition from an ObjectLit (field config)
    fn extract_field_from_object(
        &self,
        obj: &swc_ecma_ast::ObjectLit,
        name: &str,
    ) -> Result<FieldDefinition> {
        use crate::destinations::payload::ts::schema::FieldType;
        use swc_ecma_ast::*;

        let mut field = FieldDefinition {
            name: name.to_string(),
            field_type: FieldType::Group { fields: Vec::new() },
            required: false,
            default_value: None,
            label: None,
            admin: None,
            source_location: crate::destinations::payload::ts::schema::SourceLocation::default(),
        };

        // Extract field properties
        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(key) = &kv.key
            {
                match key.sym.as_ref() {
                    "name" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            field.name = s.value.to_atom_lossy().to_string();
                        }
                    }
                    "type" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            // For now, assume group type for field configs
                            // This should be expanded to handle other types
                            if s.value.to_atom_lossy().as_str() == "group" {
                                field.field_type = FieldType::Group { fields: Vec::new() };
                            } else {
                                field.field_type = FieldType::Text {
                                    min_length: None,
                                    max_length: None,
                                };
                            }
                        }
                    }
                    "required" => {
                        if let Expr::Lit(Lit::Bool(b)) = &*kv.value {
                            field.required = b.value;
                        }
                    }
                    "label" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            field.label = Some(s.value.to_atom_lossy().to_string());
                        } else if let Expr::Lit(Lit::Bool(b)) = &*kv.value
                            && !b.value
                        {
                            field.label = None;
                        }
                    }
                    "fields" => {
                        // For group fields, extract nested fields
                        if let Expr::Array(arr) = &*kv.value
                            && let Ok(nested_fields) = self.extract_fields_from_array(arr)
                        {
                            field.field_type = FieldType::Group {
                                fields: nested_fields,
                            };
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(field)
    }

    /// Extract fields from an array literal (helper for field configs)
    fn extract_fields_from_array(
        &self,
        arr: &swc_ecma_ast::ArrayLit,
    ) -> Result<Vec<FieldDefinition>> {
        use swc_ecma_ast::*;

        let mut fields = Vec::new();

        for elem in arr.elems.iter().flatten() {
            {
                if let Expr::Object(obj) = &*elem.expr {
                    if let Ok(Some(field)) = self.extract_simple_field_from_object(obj) {
                        fields.push(field);
                    }
                } else if let Expr::Ident(ident) = &*elem.expr {
                    // Handle field references within the array
                    fields.push(FieldDefinition {
                        name: format!("__ref__{}", ident.sym),
                        field_type: FieldType::Text {
                            min_length: None,
                            max_length: None,
                        },
                        required: false,
                        default_value: None,
                        label: None,
                        admin: None,
                        source_location:
                            crate::destinations::payload::ts::schema::SourceLocation::default(),
                    });
                }
            }
        }

        Ok(fields)
    }

    /// Extract a simple field definition from an object (for nested fields)
    fn extract_simple_field_from_object(
        &self,
        obj: &swc_ecma_ast::ObjectLit,
    ) -> Result<Option<FieldDefinition>> {
        use crate::destinations::payload::ts::schema::FieldType;
        use swc_ecma_ast::*;

        let mut field = FieldDefinition {
            name: String::new(),
            field_type: FieldType::Text {
                min_length: None,
                max_length: None,
            },
            required: false,
            default_value: None,
            label: None,
            admin: None,
            source_location: crate::destinations::payload::ts::schema::SourceLocation::default(),
        };

        let mut has_name = false;

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(key) = &kv.key
            {
                match key.sym.as_ref() {
                    "name" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            field.name = s.value.to_atom_lossy().to_string();
                            has_name = true;
                        }
                    }
                    "type" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            match s.value.to_atom_lossy().as_str() {
                                "text" => {
                                    field.field_type = FieldType::Text {
                                        min_length: None,
                                        max_length: None,
                                    }
                                }
                                "select" => {
                                    field.field_type = FieldType::Select {
                                        options: Vec::new(),
                                    }
                                }
                                "checkbox" => field.field_type = FieldType::Checkbox,
                                "richText" => field.field_type = FieldType::RichText,
                                "relationship" => {
                                    field.field_type = FieldType::Relationship {
                                        relationTo: String::new(),
                                    }
                                }
                                "array" => {
                                    field.field_type = FieldType::Array { fields: Vec::new() }
                                }
                                "group" => {
                                    field.field_type = FieldType::Group { fields: Vec::new() }
                                }
                                _ => {
                                    field.field_type = FieldType::Text {
                                        min_length: None,
                                        max_length: None,
                                    }
                                }
                            }
                        }
                    }
                    "required" => {
                        if let Expr::Lit(Lit::Bool(b)) = &*kv.value {
                            field.required = b.value;
                        }
                    }
                    "label" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            field.label = Some(s.value.to_atom_lossy().to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        if has_name { Ok(Some(field)) } else { Ok(None) }
    }
}

/// The 1.x entry point: one collection file, imports merged in.
pub struct PayloadSchemaParser {
    resolver: ImportResolver,
}

impl PayloadSchemaParser {
    pub fn new(base_dir: PathBuf, aliases: Option<&[PathAlias]>) -> Self {
        Self {
            resolver: ImportResolver::new(base_dir, aliases),
        }
    }

    pub fn parse_collection(&mut self, config_path: &Path) -> Result<CollectionSchema> {
        let parsed = self.resolver.parse_file(config_path)?;
        let mut schema = CollectionSchema {
            slug: String::new(),
            fields: parsed.analyzer.fields.clone(),
            blocks: parsed.analyzer.blocks.clone(),
            hooks: Default::default(),
            access: Default::default(),
            admin: Default::default(),
            imports: parsed.analyzer.imports.clone(),
            raw_ast: Some(parsed.module.clone()),
        };
        self.resolver.resolve_imports(&mut schema, config_path)?;
        Ok(schema)
    }
}
