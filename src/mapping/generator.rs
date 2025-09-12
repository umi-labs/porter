use crate::config::PorterConfig;
use crate::mapping;
use crate::mapping::graph::build_field_graph_from_ts;
use crate::mapping::template::generate_template_from_graph;
use crate::sources::wordpress::WordPressApiConnector;
use crate::sources::wordpress::config::WordPressConfig;
use crate::mapping::initialize_mappings_base;
use anyhow::{Context, Result};
use colored::Colorize;
use log::warn;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Mapping generator for creating field mappings between source and target systems
pub struct MappingGenerator {
    config: PorterConfig,
}

impl MappingGenerator {
    pub fn new(config: PorterConfig) -> Self {
        // Initialize mappings base directory to <output>/mappings
        initialize_mappings_base(&format!("{}/mappings", config.output));
        Self { config }
    }

    /// Generate mappings for all collections or a specific collection
    pub async fn generate_mappings(&self, collection_name: Option<&str>) -> Result<()> {
        println!("{}", "🔄 Generating Field Mappings".cyan().bold());
        println!("{}", "─".repeat(50));
        println!();

        // Create mappings and mapping artifacts directories if they don't exist
        // Mappings default to <output>/mappings if PORTER_MAPPINGS_DIR not set
        let mappings_base = crate::mapping::get_mapping_path("__base__", "", "");
        let mappings_base = std::path::Path::new(&mappings_base).parent().unwrap().to_string_lossy().to_string();
        let mappings_dir = Path::new(&mappings_base);
        if !mappings_dir.exists() {
            fs::create_dir_all(mappings_dir)
                .context("Failed to create mappings directory")?;
            println!("{}", "✓ Created mappings directory".green());
        }
        // Resolve graphs/templates dirs from config.io, falling back to legacy defaults
        let (graphs_dir_path, templates_dir_path) = if let Some(io) = &self.config.io {
            (io.graphs_dir.clone(), io.templates_dir.clone())
        } else {
            ("./mapping/graphs".to_string(), "./mapping/templates".to_string())
        };
        let graphs_dir = Path::new(&graphs_dir_path);
        if !graphs_dir.exists() { let _ = fs::create_dir_all(graphs_dir); }
        let templates_dir = Path::new(&templates_dir_path);
        if !templates_dir.exists() { let _ = fs::create_dir_all(templates_dir); }

        let collections_to_process = if let Some(name) = collection_name {
            self.config.collections.iter()
                .filter(|c| c.name == name)
                .collect::<Vec<_>>()
        } else {
            self.config.collections.iter().collect::<Vec<_>>()
        };

        if collections_to_process.is_empty() {
            if let Some(name) = collection_name {
                return Err(anyhow::anyhow!("Collection '{}' not found in configuration", name));
            } else {
                return Err(anyhow::anyhow!("No collections found in configuration"));
            }
        }

        for (i, collection) in collections_to_process.iter().enumerate() {
            let current = i + 1;
            let total = collections_to_process.len();

            println!(
                "{}",
                format!("📁 Processing Collection {}/{}: {}", current, total, collection.name)
                    .cyan()
                    .bold()
            );
            println!("{}", "─".repeat(40));

            self.generate_collection_mapping(collection).await?;

            // If we have a target schema, build a field graph and template
            if let Some(schema_path) = &collection.collection_path {
                if Path::new(schema_path).exists() {
                    if let Ok(graph) = build_field_graph_from_ts(schema_path) {
                        let graph_path = format!("{}/{}.json", graphs_dir_path, collection.name);
                        let tpl_path = format!("{}/{}.template.json", templates_dir_path, collection.name);
                        let graph_json = serde_json::to_string_pretty(&graph)?;
                        fs::write(&graph_path, graph_json)?;
                        let template = generate_template_from_graph(&graph);
                        let tpl_json = serde_json::to_string_pretty(&template)?;
                        fs::write(&tpl_path, tpl_json)?;
                        println!("{} {}\n{} {}",
                            "✓ Wrote field graph:".green(), graph_path,
                            "✓ Wrote mapping template:".green(), tpl_path);
                    }
                }
            }
            println!();
        }

        println!("{}", "✅ Mapping generation completed!".green().bold());
        let mappings_base = std::env::var("PORTER_MAPPINGS_DIR").unwrap_or_else(|_| format!("{}/mappings", self.config.output));
        println!("{} {}", "📁 Check the".cyan(), format!("{}", mappings_base).cyan());
        Ok(())
    }

    /// Generate mapping for a specific collection
    async fn generate_collection_mapping(&self, collection: &crate::config::CollectionConfig) -> Result<()> {
        match self.config.source.as_str() {
            "wordpress" => {
                if let Some(input_type) = self.config.metadata.as_ref()
                    .and_then(|m| m.get("wordpress_input_type")) {
                    if input_type.as_str() == "api" {
                        self.generate_wordpress_api_mapping(collection).await
                    } else {
                        self.generate_wordpress_file_mapping(collection).await
                    }
                } else {
                    self.generate_wordpress_file_mapping(collection).await
                }
            }
            "umbraco" => self.generate_umbraco_mapping(collection).await,
            _ => Err(anyhow::anyhow!("Unsupported source type: {}", self.config.source)),
        }
    }

    /// Generate mapping for WordPress API source
    async fn generate_wordpress_api_mapping(&self, collection: &crate::config::CollectionConfig) -> Result<()> {
        println!("{}", "🌐 Generating WordPress API mapping".yellow());

        // Get WordPress API URL
        let api_url = match &self.config.metadata {
            Some(metadata) => {
                match metadata.get("wordpress_api_url") {
                    Some(url_value) => url_value.as_str(),
                    None => return Err(anyhow::anyhow!("WordPress API URL not found in configuration")),
                }
            }
            None => return Err(anyhow::anyhow!("No metadata found in configuration")),
        };

        println!("API URL: {}", api_url.blue());
        println!("Endpoint: {}", collection.source_data.blue());

        // Create WordPress API connector
        let wp_config = WordPressConfig::default();
        let connector = WordPressApiConnector::new(api_url.to_string(), wp_config, None)
            .context("Failed to create WordPress API connector")?;

        // Test connection
        println!("{}", "🔗 Testing API connection...".yellow());
        connector.test_connection().await
            .context("Failed to connect to WordPress API")?;
        println!("{}", "✓ API connection successful".green());

        // Fetch endpoint data and write porter-format file
        println!("{}", "📥 Fetching endpoint data for normalization...".yellow());
        let porter_docs: Vec<Value> = match collection.source_data.as_str() {
            "posts" => connector.fetch_posts().await?,
            "pages" => connector.fetch_pages().await?,
            "media" => connector.fetch_media().await?,
            other => connector.fetch_custom_endpoint(other).await?,
        };
        println!("{}", format!("✓ Retrieved {} document(s)", porter_docs.len()).green());

        self.write_porter_format(&collection.name, &porter_docs)?;
        println!("{}", format!("✓ Wrote porter-format: ./porter-format/{}.json", collection.name).green());

        let sample_docs: Vec<Value> = if porter_docs.is_empty() { vec![] } else { porter_docs.iter().take(50).cloned().collect() };

        // Analyze WordPress data structure
        let wp_fields = self.analyze_wordpress_fields(&sample_docs)?;
        println!("{}", format!("✓ Analyzed {} WordPress fields", wp_fields.len()).green());

        // Analyze Payload collection schema
        let payload_fields = if let Some(schema_path) = &collection.collection_path {
            self.analyze_payload_schema(schema_path)?
        } else {
            warn!("No collection schema path provided, using basic field analysis");
            HashMap::new()
        };

        if !payload_fields.is_empty() {
            println!("{}", format!("✓ Analyzed {} Payload fields", payload_fields.len()).green());
        }

        if self.config.interactive {
            println!("{}", "🧭 Starting interactive mapping...".yellow());
            let _mapping = mapping::load_or_create_mapping(
                &collection.name,
                &self.config.source,
                &self.config.target,
                &sample_docs,
                collection.collection_path.as_deref(),
                true,
                None,
            )?;
            println!("{}", "✓ Interactive mapping completed".green());
        } else {
            let mappings = self.generate_field_mappings(&wp_fields, &payload_fields, collection)?;
            println!("{}", format!("✓ Generated {} field mappings", mappings.len()).green());
            let mappings_base = std::env::var("PORTER_MAPPINGS_DIR").unwrap_or_else(|_| format!("{}/mappings", self.config.output));
            let mapping_file = format!("{}/{}.{}-to-{}.json", 
                mappings_base, collection.name, self.config.source, self.config.target);
            self.save_mapping_file(&mapping_file, &mappings, collection)?;
            println!("{}", format!("✓ Saved mapping to {}", mapping_file).green());
        }

        Ok(())
    }

    /// Generate mapping for WordPress file source
    async fn generate_wordpress_file_mapping(&self, _collection: &crate::config::CollectionConfig) -> Result<()> {
        println!("{}", "📄 Generating WordPress file mapping".yellow());
        println!("{}", "⚠️  WordPress file mapping not yet implemented".yellow());
        Ok(())
    }

    /// Generate mapping for Umbraco source
    async fn generate_umbraco_mapping(&self, _collection: &crate::config::CollectionConfig) -> Result<()> {
        println!("{}", "🏢 Generating Umbraco mapping".yellow());
        println!("{}", "⚠️  Umbraco mapping not yet implemented".yellow());
        Ok(())
    }

    /// Analyze WordPress data structure to extract field information
    fn analyze_wordpress_fields(&self, documents: &[Value]) -> Result<HashMap<String, FieldInfo>> {
        let mut fields = HashMap::new();

        for doc in documents {
            if let Some(obj) = doc.as_object() {
                for (key, value) in obj {
                    let field_info = FieldInfo {
                        name: key.clone(),
                        field_type: self.infer_field_type(value),
                        sample_value: value.clone(),
                        is_required: false, // WordPress API doesn't indicate required fields
                        description: self.generate_field_description(key, value),
                    };
                    fields.insert(key.clone(), field_info);
                }
            }
        }

        Ok(fields)
    }

    /// Analyze Payload collection schema to extract field information
    fn analyze_payload_schema(&self, schema_path: &str) -> Result<HashMap<String, FieldInfo>> {
        if !Path::new(schema_path).exists() {
            warn!("Schema file not found: {}", schema_path);
            return Ok(HashMap::new());
        }

        let content = fs::read_to_string(schema_path)
            .context("Failed to read Payload schema file")?;

        // Basic TypeScript parsing to extract field information
        // This is a simplified parser - in a real implementation, you'd use a proper TypeScript parser
        let mut fields = HashMap::new();

        // Look for field definitions in the schema
        let lines: Vec<&str> = content.lines().collect();
        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with("name:") && trimmed.contains("type:") {
                // Extract field name and type
                if let Some(name_start) = trimmed.find("name: \"") {
                    let name_start = name_start + 7;
                    if let Some(name_end) = trimmed[name_start..].find("\"") {
                        let field_name = &trimmed[name_start..name_start + name_end];
                        
                        if let Some(type_start) = trimmed.find("type: \"") {
                            let type_start = type_start + 7;
                            if let Some(type_end) = trimmed[type_start..].find("\"") {
                                let field_type = &trimmed[type_start..type_start + type_end];
                                
                                let field_info = FieldInfo {
                                    name: field_name.to_string(),
                                    field_type: field_type.to_string(),
                                    sample_value: Value::Null,
                                    is_required: trimmed.contains("required: true"),
                                    description: format!("Payload field: {}", field_name),
                                };
                                fields.insert(field_name.to_string(), field_info);
                            }
                        }
                    }
                }
            }
        }

        Ok(fields)
    }

    /// Generate field mappings between WordPress and Payload fields
    fn generate_field_mappings(
        &self,
        wp_fields: &HashMap<String, FieldInfo>,
        payload_fields: &HashMap<String, FieldInfo>,
        _collection: &crate::config::CollectionConfig,
    ) -> Result<Vec<FieldMapping>> {
        let mut mappings = Vec::new();

        // Generate automatic mappings based on field name similarity
        for (wp_field_name, wp_field) in wp_fields {
            // Look for exact match first
            if let Some(payload_field) = payload_fields.get(wp_field_name) {
                mappings.push(FieldMapping {
                    source_field: wp_field_name.clone(),
                    target_field: payload_field.name.clone(),
                    transformation: self.suggest_transformation(&wp_field.field_type, &payload_field.field_type),
                    confidence: 1.0,
                    notes: "Exact field name match".to_string(),
                });
                continue;
            }

            // Look for similar field names
            let mut best_match = None;
            let mut best_score = 0.0;

            for (payload_field_name, payload_field) in payload_fields {
                let similarity = self.calculate_field_similarity(wp_field_name, payload_field_name);
                if similarity > best_score && similarity > 0.6 {
                    best_score = similarity;
                    best_match = Some((payload_field_name, payload_field));
                }
            }

            if let Some((target_field_name, target_field)) = best_match {
                mappings.push(FieldMapping {
                    source_field: wp_field_name.clone(),
                    target_field: target_field_name.clone(),
                    transformation: self.suggest_transformation(&wp_field.field_type, &target_field.field_type),
                    confidence: best_score,
                    notes: format!("Similar field name ({}% match)", (best_score * 100.0) as u32),
                });
            } else {
                // No match found - suggest manual mapping
                mappings.push(FieldMapping {
                    source_field: wp_field_name.clone(),
                    target_field: "".to_string(), // Empty means manual mapping needed
                    transformation: "manual".to_string(),
                    confidence: 0.0,
                    notes: "No automatic mapping found - requires manual mapping".to_string(),
                });
            }
        }

        Ok(mappings)
    }

    /// Calculate similarity between two field names
    fn calculate_field_similarity(&self, field1: &str, field2: &str) -> f64 {
        let f1_lower = field1.to_lowercase();
        let f2_lower = field2.to_lowercase();

        // Exact match
        if f1_lower == f2_lower {
            return 1.0;
        }

        // Check if one contains the other
        if f1_lower.contains(&f2_lower) || f2_lower.contains(&f1_lower) {
            return 0.8;
        }

        // Check for common WordPress to Payload field mappings
        let common_mappings = [
            ("title", "title"),
            ("content", "content"),
            ("excerpt", "excerpt"),
            ("slug", "slug"),
            ("date", "createdAt"),
            ("modified", "updatedAt"),
            ("status", "status"),
            ("author", "author"),
            ("featured_media", "featuredImage"),
        ];

        for (wp_field, payload_field) in &common_mappings {
            if (f1_lower == *wp_field && f2_lower == *payload_field) ||
               (f1_lower == *payload_field && f2_lower == *wp_field) {
                return 0.9;
            }
        }

        // Simple character-based similarity
        let common_chars = f1_lower.chars()
            .filter(|c| f2_lower.contains(*c))
            .count();
        let total_chars = f1_lower.len().max(f2_lower.len());

        if total_chars == 0 {
            return 0.0;
        }

        common_chars as f64 / total_chars as f64
    }

    /// Suggest transformation between field types
    fn suggest_transformation(&self, source_type: &str, target_type: &str) -> String {
        match (source_type, target_type) {
            ("string", "string") => "direct".to_string(),
            ("string", "text") => "direct".to_string(),
            ("string", "textarea") => "direct".to_string(),
            ("number", "number") => "direct".to_string(),
            ("boolean", "checkbox") => "direct".to_string(),
            ("string", "date") => "date_parse".to_string(),
            ("string", "datetime") => "datetime_parse".to_string(),
            ("array", "array") => "direct".to_string(),
            ("object", "group") => "direct".to_string(),
            _ => "manual".to_string(),
        }
    }

    /// Infer field type from JSON value
    fn infer_field_type(&self, value: &Value) -> String {
        match value {
            Value::String(_) => "string".to_string(),
            Value::Number(_) => "number".to_string(),
            Value::Bool(_) => "boolean".to_string(),
            Value::Array(_) => "array".to_string(),
            Value::Object(_) => "object".to_string(),
            Value::Null => "null".to_string(),
        }
    }

    /// Generate field description
    fn generate_field_description(&self, field_name: &str, _value: &Value) -> String {
        match field_name {
            "id" => "WordPress post/page ID".to_string(),
            "title" => "Post/page title".to_string(),
            "content" => "Post/page content".to_string(),
            "excerpt" => "Post/page excerpt".to_string(),
            "slug" => "URL slug".to_string(),
            "date" => "Publication date".to_string(),
            "modified" => "Last modified date".to_string(),
            "status" => "Publication status".to_string(),
            "author" => "Author ID".to_string(),
            "featured_media" => "Featured image ID".to_string(),
            "categories" => "Category IDs".to_string(),
            "tags" => "Tag IDs".to_string(),
            _ => format!("WordPress field: {}", field_name),
        }
    }

    /// Save mapping file to disk
    fn save_mapping_file(
        &self,
        file_path: &str,
        mappings: &[FieldMapping],
        collection: &crate::config::CollectionConfig,
    ) -> Result<()> {
        let mapping_data = json!({
            "collection": collection.name,
            "source": self.config.source,
            "target": self.config.target,
            "source_endpoint": collection.source_data,
            "generated_at": chrono::Utc::now().to_rfc3339(),
            "field_mappings": mappings,
            "metadata": {
                "total_mappings": mappings.len(),
                "automatic_mappings": mappings.iter().filter(|m| m.confidence > 0.0).count(),
                "manual_mappings_needed": mappings.iter().filter(|m| m.confidence == 0.0).count(),
            }
        });

        let content = serde_json::to_string_pretty(&mapping_data)
            .context("Failed to serialize mapping data")?;

        fs::write(file_path, content)
            .context("Failed to write mapping file")?;

        Ok(())
    }

    /// Generate only field graphs and mapping templates from target schemas
    pub fn generate_templates(&self, collection_name: Option<&str>) -> Result<()> {
        println!("{}", "🧩 Generating Field Graphs & Templates".cyan().bold());
        println!("{}", "─".repeat(50));
        println!();

        // Ensure output dirs
        let (graphs_dir_path, templates_dir_path) = if let Some(io) = &self.config.io {
            (io.graphs_dir.clone(), io.templates_dir.clone())
        } else {
            ("./mapping/graphs".to_string(), "./mapping/templates".to_string())
        };
        let graphs_dir = Path::new(&graphs_dir_path);
        if !graphs_dir.exists() { std::fs::create_dir_all(graphs_dir).context("Failed to create graphs directory")?; }
        let templates_dir = Path::new(&templates_dir_path);
        if !templates_dir.exists() { std::fs::create_dir_all(templates_dir).context("Failed to create templates directory")?; }

        let collections_to_process = if let Some(name) = collection_name {
            self.config.collections.iter().filter(|c| c.name == name).collect::<Vec<_>>()
        } else {
            self.config.collections.iter().collect::<Vec<_>>()
        };

        if collections_to_process.is_empty() {
            if let Some(name) = collection_name {
                return Err(anyhow::anyhow!("Collection '{}' not found in configuration", name));
            } else {
                return Err(anyhow::anyhow!("No collections found in configuration"));
            }
        }

        for (i, collection) in collections_to_process.iter().enumerate() {
            let current = i + 1;
            let total = collections_to_process.len();
            println!(
                "{}",
                format!("📁 Processing Collection {}/{}: {}", current, total, collection.name)
                    .cyan()
                    .bold()
            );
            println!("{}", "─".repeat(40));

            if let Some(schema_path) = &collection.collection_path {
                if Path::new(schema_path).exists() {
                    let graph = build_field_graph_from_ts(schema_path)
                        .with_context(|| format!("Failed to build field graph for {}", collection.name))?;
                    let graph_path = format!("{}/{}.json", graphs_dir_path, collection.name);
                    let tpl_path = format!("{}/{}.template.json", templates_dir_path, collection.name);
                    let graph_json = serde_json::to_string_pretty(&graph)?;
                    fs::write(&graph_path, graph_json)?;
                    let template = generate_template_from_graph(&graph);
                    let tpl_json = serde_json::to_string_pretty(&template)?;
                    fs::write(&tpl_path, tpl_json)?;
                    println!(
                        "{} {}\n{} {}",
                        "✓ Wrote field graph:".green(),
                        graph_path,
                        "✓ Wrote mapping template:".green(),
                        tpl_path
                    );
                } else {
                    warn!("Schema file not found for collection '{}': {}", collection.name, schema_path);
                }
            } else {
                warn!("No collection schema path provided for '{}'", collection.name);
            }

            println!();
        }

        println!("{}", "✅ Template generation completed!".green().bold());
        Ok(())
    }
}

impl MappingGenerator {
    fn write_porter_format(&self, collection_name: &str, docs: &[Value]) -> Result<()> {
        // Resolve porter-format dir from metadata.porter_format_dir, else default ./porter-format
        let porter_dir = self.config.metadata.as_ref()
            .and_then(|m| m.get("porter_format_dir")).map(|s| s.clone())
            .unwrap_or_else(|| "./porter-format".to_string());
        let out_dir = Path::new(&porter_dir);
        if !out_dir.exists() {
            fs::create_dir_all(out_dir).context("Failed to create porter-format directory")?;
        }
        let out_file = out_dir.join(format!("{}.json", collection_name));
        let content = serde_json::to_string_pretty(&docs)
            .context("Failed to serialize porter-format docs")?;
        fs::write(out_file, content).context("Failed to write porter-format file")?;
        Ok(())
    }
}

/// Information about a field
#[derive(Debug, Clone)]
struct FieldInfo {
    name: String,
    field_type: String,
    sample_value: Value,
    is_required: bool,
    description: String,
}

/// Field mapping between source and target
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FieldMapping {
    source_field: String,
    target_field: String,
    transformation: String,
    confidence: f64,
    notes: String,
}
