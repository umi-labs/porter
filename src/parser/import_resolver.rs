use super::schema::{
    CollectionSchema, FieldDefinition, FieldAdmin, FieldType, BlockDefinition, BlockLabels,
    TabDefinition, SelectOption, DateAdmin, DateConfig, HooksConfig, AccessConfig, AdminConfig,
    ImportInfo, ImportSpecifier, SourceLocation
};
use super::errors::{Result, SchemaParseError};
use super::ast_analyzer::{AstAnalyzer, ExportedItem};
use crate::config::{TypescriptSection, PathAliasMapping};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use swc_common::{sync::Lrc, FileName, SourceMap};
use swc_core::ecma::ast::*;
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
pub struct ParsedFile {
    pub module: Module,
    pub analyzer: AstAnalyzer,
    pub exports: HashMap<String, ExportedItem>,
}

impl ImportResolver {
    pub fn new(base_dir: PathBuf, ts_config: Option<&TypescriptSection>) -> Self {
        let mut alias_map = HashMap::new();
        
        // Load TypeScript configuration from the migration config file
        if let Some(config) = ts_config {
            dlog!("Loading TypeScript config with path mappings");
            
            if let Some(mappings) = &config.path_mappings {
                dlog!("Found {} path mappings", mappings.len());
                
                for mapping in mappings {
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
                dlog!("No path mappings found, using defaults");
                // Default aliases if no mappings provided
                alias_map.insert("@/".to_string(), vec![base_dir.join("src")]);
            }
        } else {
            dlog!("No TypeScript config provided, using default aliases");
            // Default aliases if no config provided
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

    pub fn parse_file(&mut self, file_path: &Path) -> Result<ParsedFile> {
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

    pub fn resolve_import_path(&self, import_path: &str, from_file: &Path) -> Result<PathBuf> {
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
        
        // Try each alias - prioritize longer matches first
        let mut sorted_aliases: Vec<_> = self.alias_map.keys().collect();
        sorted_aliases.sort_by(|a, b| b.len().cmp(&a.len()));
        
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
