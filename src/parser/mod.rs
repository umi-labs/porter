mod schema;
mod ast_analyzer;
mod import_resolver;
mod template_generator;
mod errors;

#[cfg(test)]
mod tests;

pub use schema::*;
pub use errors::{SchemaParseError, Result};
pub use template_generator::{FlattenedTemplate, TemplateGenerator};

use std::path::{Path, PathBuf};
use import_resolver::ImportResolver;
use ast_analyzer::AstAnalyzer;
use crate::config::TypescriptSection;
use crate::{dlog, dlog_info, dlog_success, dlog_warning, dlog_error, dlog_step, dlog_data, dlog_file, dlog_processing, dlog_result};

pub struct PayloadSchemaParser {
    base_dir: PathBuf,
    resolver: ImportResolver,
}

impl PayloadSchemaParser {
    pub fn new(base_dir: PathBuf, ts_config: Option<&TypescriptSection>) -> Self {
        dlog_processing!("Creating PayloadSchemaParser with base_dir: {:?}", base_dir);
        let resolver = ImportResolver::new(base_dir.clone(), ts_config);
        Self {
            base_dir,
            resolver,
        }
    }

    pub fn parse_collection(&mut self, config_path: &Path) -> Result<CollectionSchema> {
        dlog_processing!("Parsing collection config: {:?}", config_path);
        
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

        dlog_processing!("Initial parse found {} fields and {} imports", 
              schema.fields.len(), schema.imports.len());

        // Resolve all imports and merge schemas
        self.resolver.resolve_imports(&mut schema, config_path)?;

        dlog_processing!("After import resolution: {} fields", schema.fields.len());

        Ok(schema)
    }

    pub fn generate_template(&mut self, config_path: &Path) -> Result<FlattenedTemplate> {
        dlog_processing!("Generating template for: {:?}", config_path);
        let schema = self.parse_collection(config_path)?;
        TemplateGenerator::generate(&schema)
    }
}
