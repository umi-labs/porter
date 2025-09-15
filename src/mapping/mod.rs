pub mod generator;
pub mod graph;
pub mod template;
pub mod schema_introspect;
pub mod json_mapping;
pub mod nested;
pub mod transforms;
pub mod typescript;
pub mod validation;

use anyhow::{Result, anyhow};
use log::{info, debug, warn};
use serde_json::{Value, json};
use colored::Colorize;
use std::io::{self, Write};

use crate::util::fs;
use crate::util::interact;
use self::json_mapping::{Mapping, FieldMapping};
use self::nested::{NestedPath, NestedMapping, apply_nested_mapping};
use self::transforms::{get_path, to_point_from_latlng, combine_coordinates_from_fields};
use self::validation::{validate_mapping, create_schema_from_documents, ValidationResult};
use std::sync::OnceLock;
static MAPPINGS_BASE: OnceLock<String> = OnceLock::new();

/// Initialize the base directory used for mapping files
pub fn initialize_mappings_base(dir: &str) {
    let _ = MAPPINGS_BASE.set(dir.to_string());
}

fn resolve_mappings_base() -> String {
    if let Some(set) = MAPPINGS_BASE.get() { return set.clone(); }
    if let Ok(env_dir) = std::env::var("PORTER_MAPPINGS_DIR") { return env_dir; }
    "./migrations/output/mappings".to_string()
}

/// Result of coordinate combination operation
struct CombineCoordinatesResult {
    transforms: Vec<Value>,
    description: String,
}

/// Generates a mapping file path based on the collection name
pub fn get_mapping_path(collection: &str, source: &str, target: &str) -> String {
    let base_dir = resolve_mappings_base();
    format!("{}/{}.{}-to-{}.json", base_dir, collection, source, target)
}

/// Loads a mapping from a file, or creates a new one if the file doesn't exist
pub fn load_or_create_mapping(
    collection: &str, 
    source: &str, 
    target: &str,
    source_docs: &[Value],
    collection_path: Option<&str>,
    interactive: bool,
    collection_progress: Option<(usize, usize)>
) -> Result<Mapping> {
    let mapping_path = get_mapping_path(collection, source, target);

    info!("load_or_create_mapping called - collection: {}, interactive: {}, file exists: {}", 
          collection, interactive, fs::file_exists(&mapping_path));

    // If mapping file exists and not in interactive mode, load it
    if fs::file_exists(&mapping_path) && !interactive {
        info!("Loading existing mapping from {}", mapping_path);
        let content = fs::read_file(&mapping_path)?;
        let mapping: Mapping = serde_json::from_str(&content)?;
        return Ok(mapping);
    }

    // If interactive mode is enabled, always regenerate mappings
    if interactive && fs::file_exists(&mapping_path) {
        info!("Interactive mode enabled - regenerating mappings for {}", collection);
        // Delete existing mapping file to force recreation
        fs::delete_file(&mapping_path)?;
    }

    // Otherwise, create a new mapping
    info!("Creating new mapping for {} from {} to {}", collection, source, target);

    // If collection path is provided and target is payload, analyze the collection schema
    let target_fields = if target == "payload" && collection_path.is_some() {
        extract_payload_fields(collection_path.unwrap())?
    } else {
        Vec::new()
    };

    // Extract source fields from the first document
    let source_fields = if !source_docs.is_empty() {
        extract_source_fields(&source_docs[0])
    } else {
        Vec::new()
    };

    // Generate field mappings
    let (field_mappings, block_mappings) = generate_field_mappings(&source_fields, &target_fields, source_docs, interactive, Some(collection), collection_progress)?;

    let mapping = Mapping {
        source: source.to_string(),
        target: target.to_string(),
        collection: collection.to_string(),
        field_mappings,
        block_mappings,
    };

    // Save the mapping
    save_mapping(&mapping)?;

    Ok(mapping)
}

/// Validates a mapping against source documents and target schema
pub fn validate_mapping_with_documents(
    mapping: &Mapping,
    source_docs: &[Value],
    target_schema_path: Option<&str>,
) -> Result<ValidationResult> {
    // Create source schema from documents
    let source_schema = create_schema_from_documents(source_docs)?;

    // Create target schema (for now, we'll create a basic one)
    // In the future, this could be extracted from the TypeScript schema file
    let target_schema = if let Some(schema_path) = target_schema_path {
        // TODO: Extract schema from TypeScript file
        create_basic_target_schema(schema_path)?
    } else {
        create_basic_target_schema("unknown")?
    };

    Ok(validate_mapping(mapping, &source_schema, &target_schema))
}

/// Creates a basic target schema (placeholder for now)
fn create_basic_target_schema(_schema_path: &str) -> Result<validation::SchemaInfo> {
    // This is a placeholder implementation
    // In the future, this should extract the actual schema from the TypeScript file
    use std::collections::HashMap;
    use validation::{SchemaInfo, FieldInfo};

    let mut fields = HashMap::new();
    fields.insert(
        "title".to_string(),
        FieldInfo {
            field_type: "string".to_string(),
            required: true,
            validations: Vec::new(),
            relationship: None,
        },
    );
    fields.insert(
        "content".to_string(),
        FieldInfo {
            field_type: "string".to_string(),
            required: false,
            validations: Vec::new(),
            relationship: None,
        },
    );

    Ok(SchemaInfo {
        fields,
        required_fields: vec!["title".to_string()],
    })
}

/// Saves a mapping to a file
pub fn save_mapping(mapping: &Mapping) -> Result<()> {
    let mapping_path = get_mapping_path(&mapping.collection, &mapping.source, &mapping.target);

    // Ensure the mappings directory exists
    let base_dir = resolve_mappings_base();
    fs::ensure_dir(&base_dir)?;

    // Serialize and save the mapping
    let content = serde_json::to_string_pretty(mapping)?;
    fs::write_file(&mapping_path, &content)?;

    info!("Saved mapping to {}", mapping_path);
    Ok(())
}

/// Loads a mapping file only; errors if it does not exist
pub fn load_mapping_only(collection: &str, source: &str, target: &str) -> Result<Mapping> {
    let mapping_path = get_mapping_path(collection, source, target);
    if !fs::file_exists(&mapping_path) {
        return Err(anyhow!(
            "Mapping not found for collection '{}' (expected at {}). Run 'porter generate' first.",
            collection, mapping_path
        ));
    }
    let content = fs::read_file(&mapping_path)?;
    let mapping: Mapping = serde_json::from_str(&content)?;
    Ok(mapping)
}

/// Loads field names from existing graph file
fn load_fields_from_graph_file(collection_path: &str) -> Result<Vec<String>> {
    use std::path::Path;
    
    // Extract collection name from the path
    let collection_name = Path::new(collection_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("Could not extract collection name from path: {}", collection_path))?;
    
    // Try to find the graph file in configured locations
    // First try to load porter config to get the correct paths
    let graph_paths = if let Ok(Some(config)) = crate::config::find_and_load_config() {
        let graphs_dir = config.io.as_ref()
            .map(|io| io.graphs_dir.clone())
            .unwrap_or_else(|| "./migrations/output/graphs".to_string());
        vec![
            format!("{}/{}.json", graphs_dir, collection_name),
            format!("{}/{}.ts", graphs_dir, collection_name),
        ]
    } else {
        // Fallback to common locations if config not found
        vec![
            format!("./migrations/output/graphs/{}.json", collection_name),
            format!("./mapping/graphs/{}.json", collection_name),
            format!("./graphs/{}.json", collection_name),
            format!("./migrations/output/graphs/{}.ts", collection_name),
            format!("./mapping/graphs/{}.ts", collection_name),
            format!("./graphs/{}.ts", collection_name),
        ]
    };
    
    for graph_path in graph_paths {
        if fs::file_exists(&graph_path) {
            info!("Loading fields from graph file: {}", graph_path);
            let content = fs::read_file(&graph_path)?;
            
            // Try to parse as JSON first
            if let Ok(graph_data) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
                let mut fields = Vec::new();
                for node in graph_data {
                    if let Some(path) = node.get("path").and_then(|v| v.as_str()) {
                        fields.push(path.to_string());
                    }
                }
                if !fields.is_empty() {
                    info!("Loaded {} fields from graph file: {}", fields.len(), graph_path);
                    return Ok(fields);
                }
            }
        }
    }
    
    Err(anyhow!("No graph file found for collection: {}", collection_name))
}

/// Extracts field names from a Payload collection schema
/// First tries to load from existing graph file, falls back to parsing TypeScript schema
fn extract_payload_fields(collection_path: &str) -> Result<Vec<String>> {
    // Try to load from existing graph file first
    if let Ok(fields) = load_fields_from_graph_file(collection_path) {
        return Ok(fields);
    }

    // Fallback to parsing TypeScript schema file
    if !fs::file_exists(collection_path) {
        return Err(anyhow!("Collection file not found: {}", collection_path));
    }

    // Use the TypeScript parser to extract field definitions
    info!("Parsing TypeScript collection schema: {}", collection_path);
    match typescript::parse_typescript_file(collection_path) {
        Ok(field_definitions) => {
            debug!("Extracted {} field definitions from {}", field_definitions.len(), collection_path);

            // Convert field definitions to field names
            let field_names = typescript::field_definitions_to_names(&field_definitions);

            if field_names.is_empty() {
                warn!("No fields found in collection schema: {}", collection_path);
            } else {
                info!("Extracted {} field names from collection schema", field_names.len());
            }

            Ok(field_names)
        },
        Err(e) => {
            // Fallback to simple string-based parsing if TypeScript parsing fails
            warn!("Failed to parse TypeScript file: {}. Falling back to simple parsing.", e);

            let content = fs::read_file(collection_path)?;
            let mut fields = Vec::new();

            // Simple extraction of field names
            for line in content.lines() {
                if line.contains("name:") && line.contains("type:") {
                    if let Some(name_start) = line.find("name:") {
                        let name_part = &line[name_start + 5..];
                        if let Some(quote_start) = name_part.find('\'') {
                            if let Some(quote_end) = name_part[quote_start + 1..].find('\'') {
                                let field_name = &name_part[quote_start + 1..quote_start + 1 + quote_end];
                                fields.push(field_name.to_string());
                            }
                        } else if let Some(quote_start) = name_part.find('"') {
                            if let Some(quote_end) = name_part[quote_start + 1..].find('"') {
                                let field_name = &name_part[quote_start + 1..quote_start + 1 + quote_end];
                                fields.push(field_name.to_string());
                            }
                        }
                    }
                }
            }

            Ok(fields)
        }
    }
}

/// Extracts field names from a source document
fn extract_source_fields(doc: &Value) -> Vec<String> {
    let mut fields = Vec::new();

    if let Some(obj) = doc.as_object() {
        for key in obj.keys() {
            fields.push(key.clone());
        }
    }

    fields
}

/// Generates field mappings based on source and target fields
fn generate_field_mappings(
    source_fields: &[String], 
    target_fields: &[String],
    source_data: &[serde_json::Value],
    interactive: bool,
    collection_name: Option<&str>,
    collection_progress: Option<(usize, usize)>
) -> Result<(Vec<FieldMapping>, Option<crate::mapping::json_mapping::BlockMappings>)> {
    let mut mappings = Vec::new();
    let mut used_source_fields = std::collections::HashSet::<String>::new();
    let mut pending_mappings = Vec::new();
    let mut completed_mappings = Vec::new();

    info!("Generating field mappings - interactive: {}, source fields: {}, target fields: {}", 
          interactive, source_fields.len(), target_fields.len());

    // If not interactive, create simple 1:1 mappings for matching fields
    if !interactive {
        info!("Running in non-interactive mode - creating 1:1 mappings");
        for target in target_fields {
            if source_fields.contains(target) {
                mappings.push(FieldMapping {
                    to: target.clone(),
                    from: json!(target),
                    transforms: Vec::new(),
                    fallback: None,
                });
            }
        }
        info!("Created {} mappings in non-interactive mode", mappings.len());
        return Ok((mappings, None));
    }

    info!("Running in interactive mode - starting interactive mapping process");

    // Interactive mapping generation - collect all mappings first
    let total_fields = target_fields.len();
    
    // Show initial configuration message
    clear_screen();
    println!("{}", "=== Field Mapping Configuration ===".cyan().bold());
    if let Some(collection) = collection_name {
        println!("{}", format!("Collection: {}", collection).white().bold());
        println!();
    }
    println!("{}", "Configure field mappings from Umbraco to Payload CMS".white());
    println!("{}", "Use arrow keys to navigate and Enter to select".white());
    println!();
    
    for (index, target) in target_fields.iter().enumerate() {
        let current_field = index + 1;
        
        // Clear screen and show progress header
        clear_screen();
        if let Some(collection) = collection_name {
            println!("{}", format!("=== Collection: {} ===", collection).blue().bold());
            if let Some((current_collection, total_collections)) = collection_progress {
                println!("{}", format!("=== Collection Progress: {}/{} ===", current_collection, total_collections).green().bold());
            }
        }
        println!("{}", format!("=== Field Progress: {}/{} ===", current_field, total_fields).yellow().bold());
        
        // Check if this is a block field
        if target.starts_with("blocks.") {
            // Handle block field mapping
            if let Some(block_type) = extract_block_type_from_path(target) {
                println!("{}", format!("=== Mapping for block field: {} (block type: {}) ===", target, block_type).magenta().bold());
                println!("{}", "This is a block field. You'll need to map ACF flexible content layouts to this block type.".yellow());
                println!();
                
                // Skip individual block fields - we'll handle block mapping separately
                continue;
            }
        }
        
        println!("{}", format!("=== Mapping for target field: {} ===", target).magenta().bold());

        // Suggest matching source fields
        let mut suggestions = Vec::new();
        for source in source_fields {
            if !used_source_fields.contains(source) {
                if source.to_lowercase() == target.to_lowercase() {
                    suggestions.push(source.clone());
                } else if source.to_lowercase().contains(&target.to_lowercase()) {
                    suggestions.push(source.clone());
                } else if target.to_lowercase().contains(&source.to_lowercase()) {
                    suggestions.push(source.clone());
                }
            }
        }

        // Add remaining unused source fields as options
        let mut options = suggestions.clone();
        for source in source_fields {
            if !used_source_fields.contains(source) && !options.contains(source) {
                options.push(source.clone());
            }
        }
        options.push("Skip this field".to_string());
        
        // Show progress table
        display_progress_table(&completed_mappings, &target_fields, current_field, total_fields, collection_name, collection_progress);

        // Prompt user to select a source field using arrow keys
        let selection = interact::select_with_arrows("Select source field by using arrow keys to move up and down and enter to select:", &options, target)?;

        if selection == 0 {
            // User chose to skip this field (now at index 0)
            println!("{}", format!("✓ Skipped mapping for '{}'", target).yellow());
            completed_mappings.push((target.clone(), "SKIPPED".to_string(), "".to_string(), "".to_string()));
            continue;
        }

        // Adjust selection index since "Skip this field" is now at index 0
        let selected_source = &options[selection - 1];
        used_source_fields.insert(selected_source.clone());

        // Ask if any transformations are needed (default: no)
        let needs_transform = interact::confirm("Does this field need transformation?")?;

        let mut transforms = Vec::new();
        let mut transform_description = "direct".to_string();
        
        if needs_transform {
            // Simple transform options
            let transform_options = vec![
                "None",
                "Convert to point (for coordinates)",
                "Combine coordinates from separate fields",
                "Split by comma",
                "Custom (specify later)"
            ];

            let transform_selection = interact::select_with_default("Select transformation", &transform_options, 0)?;

            match transform_selection {
                1 => {
                    // Convert to point
                    transforms.push(json!({
                        "type": "to_point",
                        "params": {
                            "lat": format!("{}.lat", selected_source),
                            "lng": format!("{}.lng", selected_source)
                        }
                    }));
                    transform_description = "point".to_string();
                },
                2 => {
                    // Combine coordinates from separate fields
                    let combine_result = handle_combine_coordinates(selected_source, &source_fields)?;
                    transforms.extend(combine_result.transforms);
                    transform_description = combine_result.description;
                },
                3 => {
                    // Split by comma
                    transforms.push(json!({
                        "type": "split_comma"
                    }));
                    transform_description = "split".to_string();
                },
                4 => {
                    // Custom - would need more sophisticated handling
                    let custom = interact::prompt("Enter custom transform (JSON)")?;
                    if !custom.is_empty() {
                        let custom_json: Value = serde_json::from_str(&custom)?;
                        transforms.push(custom_json);
                        transform_description = "custom".to_string();
                    }
                },
                _ => {}
            }
        }

        // Ask for fallback value (default: no)
        let wants_fallback = interact::confirm("Do you want to specify a fallback value?")?;
        let fallback = if wants_fallback {
            let fallback_value = interact::prompt("Enter fallback value")?;
            Some(fallback_value)
        } else {
            None
        };

        // Store the pending mapping for confirmation
        pending_mappings.push((transform_description.clone(), selected_source.clone(), target.clone()));

        // Track completed mapping
        let transform_info = if transform_description == "direct" {
            "".to_string()
        } else {
            format!("({})", transform_description)
        };
        let fallback_info = if fallback.is_some() {
            format!("fallback: {}", fallback.as_ref().unwrap())
        } else {
            "".to_string()
        };
        completed_mappings.push((target.clone(), selected_source.clone(), transform_info, fallback_info));

        // Create the mapping
        mappings.push(FieldMapping {
            to: target.clone(),
            from: json!(selected_source),
            transforms,
            fallback,
        });
    }

    // Show summary and get confirmation
    if !pending_mappings.is_empty() {
        let confirmed = interact::confirm_mappings(&pending_mappings)?;
        if !confirmed {
            return Err(anyhow!("Mapping cancelled by user"));
        }
    }

    // Handle block mapping if we have block fields and source data
    let block_mappings = if interactive && target_fields.iter().any(|f| f.starts_with("blocks.")) {
        Some(generate_block_mappings(source_data, target_fields, collection_name)?)
    } else {
        None
    };

    Ok((mappings, block_mappings))
}

/// Extracts block type from a field path like "blocks.content.columns"
fn extract_block_type_from_path(path: &str) -> Option<String> {
    if path.starts_with("blocks.") {
        let parts: Vec<&str> = path.split('.').collect();
        if parts.len() >= 2 {
            return Some(parts[1].to_string());
        }
    }
    None
}

/// Extracts ACF flexible content blocks from source data
fn extract_acf_flexible_content(source_data: &[serde_json::Value]) -> Result<Vec<crate::mapping::json_mapping::ACFFlexibleContentBlock>> {
    let mut blocks = Vec::new();
    
    for doc in source_data {
        if let Some(acf_field) = doc.get("acf") {
            if let Some(flexible_content) = acf_field.get("flexible_content") {
                if let Some(flexible_array) = flexible_content.as_array() {
                    for block_data in flexible_array {
                        if let Some(layout) = block_data.get("acf_fc_layout") {
                            if let Some(layout_str) = layout.as_str() {
                                blocks.push(crate::mapping::json_mapping::ACFFlexibleContentBlock {
                                    acf_fc_layout: layout_str.to_string(),
                                    fields: block_data.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    
    Ok(blocks)
}

/// Gets unique ACF layout names from source data
fn get_unique_acf_layouts(source_data: &[serde_json::Value]) -> Result<Vec<String>> {
    let blocks = extract_acf_flexible_content(source_data)?;
    let mut layouts: std::collections::HashSet<String> = std::collections::HashSet::new();
    
    for block in blocks {
        layouts.insert(block.acf_fc_layout);
    }
    
    let mut result: Vec<String> = layouts.into_iter().collect();
    result.sort();
    Ok(result)
}

/// Gets unique Payload block types from target field graph
fn get_unique_payload_block_types(target_fields: &[String]) -> Vec<String> {
    let mut block_types: std::collections::HashSet<String> = std::collections::HashSet::new();
    
    for field in target_fields {
        if let Some(block_type) = extract_block_type_from_path(field) {
            block_types.insert(block_type);
        }
    }
    
    let mut result: Vec<String> = block_types.into_iter().collect();
    result.sort();
    result
}

/// Generates block mappings interactively
fn generate_block_mappings(
    source_data: &[serde_json::Value],
    target_fields: &[String],
    collection_name: Option<&str>
) -> Result<crate::mapping::json_mapping::BlockMappings> {
    use crate::mapping::json_mapping::BlockMappings;
    use std::collections::HashMap;
    
    let mut block_mappings = BlockMappings::new();
    
    // Get unique ACF layouts and Payload block types
    let acf_layouts = get_unique_acf_layouts(source_data)?;
    let payload_block_types = get_unique_payload_block_types(target_fields);
    
    if acf_layouts.is_empty() {
        info!("No ACF flexible content layouts found in source data");
        return Ok(block_mappings);
    }
    
    if payload_block_types.is_empty() {
        info!("No Payload block types found in target fields");
        return Ok(block_mappings);
    }
    
    // Clear screen and show block mapping header
    clear_screen();
    println!("{}", "=== Block Mapping Configuration ===".cyan().bold());
    if let Some(collection) = collection_name {
        println!("{}", format!("Collection: {}", collection).white().bold());
        println!();
    }
    println!("{}", "Map ACF flexible content layouts to Payload block types".white());
    println!();
    
    // Map each ACF layout to a Payload block type
    for (index, acf_layout) in acf_layouts.iter().enumerate() {
        let current_layout = index + 1;
        let total_layouts = acf_layouts.len();
        
        clear_screen();
        println!("{}", "=== Block Mapping Configuration ===".cyan().bold());
        if let Some(collection) = collection_name {
            println!("{}", format!("Collection: {}", collection).white().bold());
        }
        println!("{}", format!("=== Layout Progress: {}/{} ===", current_layout, total_layouts).yellow().bold());
        println!("{}", format!("=== Mapping ACF layout: '{}' ===", acf_layout).magenta().bold());
        println!();
        
        // Show available Payload block types
        println!("{}", "Available Payload block types:".white());
        for (i, block_type) in payload_block_types.iter().enumerate() {
            println!("  {}. {}", i + 1, block_type);
        }
        println!();
        
        // Get user selection
        let selection_index = interact::select(
            &format!("Select Payload block type for ACF layout '{}'", acf_layout),
            &payload_block_types
        )?;
        
        let selected_block_type = &payload_block_types[selection_index];
        block_mappings.layout_to_block_type.insert(acf_layout.clone(), selected_block_type.clone());
        println!("{}", format!("✓ Mapped '{}' → '{}'", acf_layout, selected_block_type).green());
        
        if current_layout < total_layouts {
            println!();
            println!("{}", "Press Enter to continue...".white());
            let _ = std::io::stdin().read_line(&mut String::new());
        }
    }
    
    // Now map fields within each block type
    for (acf_layout, payload_block_type) in &block_mappings.layout_to_block_type {
        if let Some(block_field_mappings) = map_block_fields(
            source_data, 
            acf_layout, 
            payload_block_type, 
            target_fields,
            collection_name
        )? {
            block_mappings.block_field_mappings.insert(payload_block_type.clone(), block_field_mappings);
        }
    }
    
    Ok(block_mappings)
}

/// Maps fields within a specific block type
fn map_block_fields(
    source_data: &[serde_json::Value],
    acf_layout: &str,
    payload_block_type: &str,
    target_fields: &[String],
    collection_name: Option<&str>
) -> Result<Option<Vec<FieldMapping>>> {
    // Get fields for this specific block type
    let block_target_fields: Vec<String> = target_fields
        .iter()
        .filter(|field| field.starts_with(&format!("blocks.{}.", payload_block_type)))
        .map(|field| field.clone())
        .collect();
    
    if block_target_fields.is_empty() {
        return Ok(None);
    }
    
    // Extract source fields from ACF blocks with this layout
    let mut source_fields = std::collections::HashSet::<String>::new();
    for doc in source_data {
        if let Some(acf_field) = doc.get("acf") {
            if let Some(flexible_content) = acf_field.get("flexible_content") {
                if let Some(flexible_array) = flexible_content.as_array() {
                    for block_data in flexible_array {
                        if let Some(layout) = block_data.get("acf_fc_layout") {
                            if let Some(layout_str) = layout.as_str() {
                                if layout_str == acf_layout {
                                    // Extract field names from this block
                                    if let Some(obj) = block_data.as_object() {
                                        for key in obj.keys() {
                                            if key != "acf_fc_layout" {
                                                source_fields.insert(key.clone());
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
    
    let source_fields: Vec<String> = source_fields.into_iter().collect();
    
    if source_fields.is_empty() {
        return Ok(None);
    }
    
    // Clear screen and show block field mapping
    clear_screen();
    println!("{}", "=== Block Field Mapping ===".cyan().bold());
    if let Some(collection) = collection_name {
        println!("{}", format!("Collection: {}", collection).white().bold());
    }
    println!("{}", format!("ACF Layout: '{}' → Payload Block: '{}'", acf_layout, payload_block_type).white().bold());
    println!();
    
    // Map each target field to a source field
    let mut field_mappings = Vec::new();
    for target_field in &block_target_fields {
        let field_name = target_field.split('.').last().unwrap_or(target_field);
        
        println!("{}", format!("=== Mapping field: {} ===", field_name).magenta().bold());
        
        // Suggest matching source fields
        let mut suggestions = Vec::new();
        for source in &source_fields {
            if source.to_lowercase() == field_name.to_lowercase() {
                suggestions.push(source.clone());
            } else if source.to_lowercase().contains(&field_name.to_lowercase()) {
                suggestions.push(source.clone());
            } else if field_name.to_lowercase().contains(&source.to_lowercase()) {
                suggestions.push(source.clone());
            }
        }
        
        // Add remaining source fields as options
        let mut options = suggestions.clone();
        for source in &source_fields {
            if !options.contains(source) {
                options.push(source.clone());
            }
        }
        
        // Add skip option
        options.insert(0, "⏭️  Skip this field".to_string());
        
        let selection_index = interact::select(
            &format!("Select source field for '{}'", field_name),
            &options
        )?;
        
        let selected_source = &options[selection_index];
        if selected_source != "⏭️  Skip this field" {
            field_mappings.push(FieldMapping {
                to: target_field.clone(),
                from: serde_json::Value::String(selected_source.clone()),
                transforms: Vec::new(),
                fallback: None,
            });
            println!("{}", format!("✓ Mapped '{}' → '{}'", selected_source, field_name).green());
        } else {
            println!("{}", format!("⏭️  Skipped '{}'", field_name).yellow());
        }
        
        println!();
    }
    
    Ok(Some(field_mappings))
}

/// Clears the terminal screen
fn clear_screen() {
    print!("\x1B[2J\x1B[1;1H");
    io::stdout().flush().unwrap();
}

/// Handles coordinate combination from separate fields
fn handle_combine_coordinates(selected_source: &str, source_fields: &[String]) -> Result<CombineCoordinatesResult> {
    println!("{}", "Coordinate combination options:".cyan());
    println!("1. Single coordinate object (e.g., {{ lat: 1.23, lng: 4.56 }})");
    println!("2. Separate latitude and longitude fields");
    
    let choice = interact::select_with_default("Choose coordinate format:", &["Single object", "Separate fields"], 0)?;
    
    match choice {
        0 => {
            // Single coordinate object - use existing to_point logic
            let transforms = vec![json!({
                "type": "to_point",
                "params": {
                    "lat": format!("{}.lat", selected_source),
                    "lng": format!("{}.lng", selected_source)
                }
            })];
            
            Ok(CombineCoordinatesResult {
                transforms,
                description: "point".to_string(),
            })
        },
        1 => {
            // Separate latitude and longitude fields
            let lat_field = select_coordinate_field("latitude", source_fields)?;
            let lng_field = select_coordinate_field("longitude", source_fields)?;
            
            let transforms = vec![json!({
                "type": "combine_coordinates",
                "params": {
                    "lat_field": lat_field,
                    "lng_field": lng_field
                }
            })];
            
            Ok(CombineCoordinatesResult {
                transforms,
                description: format!("combine({}+{})", lat_field, lng_field),
            })
        },
        _ => Err(anyhow!("Invalid choice for coordinate combination"))
    }
}

/// Helper function to select a coordinate field
fn select_coordinate_field(coordinate_type: &str, source_fields: &[String]) -> Result<String> {
    println!("{}", format!("Select {} field:", coordinate_type).cyan());
    
    // Filter fields that might be coordinate-related
    let coordinate_keywords = match coordinate_type {
        "latitude" => vec!["lat", "latitude", "y"],
        "longitude" => vec!["lng", "long", "longitude", "x"],
        _ => vec![]
    };
    
    let mut options = Vec::new();
    let mut coordinate_fields = Vec::new();
    
    // Add fields that match coordinate keywords first
    for field in source_fields {
        let field_lower = field.to_lowercase();
        if coordinate_keywords.iter().any(|keyword| field_lower.contains(keyword)) {
            coordinate_fields.push(field.clone());
        }
    }
    
    // Add coordinate-related fields first
    options.extend(coordinate_fields.clone());
    
    // Add separator if we have coordinate fields
    if !coordinate_fields.is_empty() {
        options.push("--- Other fields ---".to_string());
    }
    
    // Add all other fields
    for field in source_fields {
        if !coordinate_fields.contains(field) {
            options.push(field.clone());
        }
    }
    
    let selection = interact::select_with_arrows(
        &format!("Select {} field:", coordinate_type),
        &options,
        coordinate_type
    )?;
    
    if selection < coordinate_fields.len() {
        Ok(coordinate_fields[selection].clone())
    } else if !coordinate_fields.is_empty() && selection == coordinate_fields.len() {
        // User selected the separator, so select from other fields
        let other_fields: Vec<String> = source_fields.iter()
            .filter(|f| !coordinate_fields.contains(f))
            .cloned()
            .collect();
        
        let other_selection = interact::select_with_arrows(
            &format!("Select {} field from other fields:", coordinate_type),
            &other_fields,
            coordinate_type
        )?;
        
        Ok(other_fields[other_selection].clone())
    } else {
        // Direct selection from all fields
        let adjusted_index = if !coordinate_fields.is_empty() { selection - 1 } else { selection };
        Ok(options[adjusted_index].clone())
    }
}

/// Displays a progress table showing completed mappings and remaining fields
fn display_progress_table(
    completed_mappings: &[(String, String, String, String)], 
    target_fields: &[String], 
    current_field: usize, 
    total_fields: usize,
    collection_name: Option<&str>,
    collection_progress: Option<(usize, usize)>
) {
    if let Some(collection) = collection_name {
        let mut title = format!("📊 Progress Summary - Collection: {}", collection);
        if let Some((current_collection, total_collections)) = collection_progress {
            title.push_str(&format!(" ({}/{})", current_collection, total_collections));
        }
        println!("{}", title.cyan().bold());
    } else {
        println!("{}", "📊 Progress Summary".cyan().bold());
    }
    println!("{}", "─".repeat(80));
    
    // Show completed mappings
    if !completed_mappings.is_empty() {
        println!("{}", "✅ Completed Mappings:".green().bold());
        for (target, source, transform, fallback) in completed_mappings {
            let status = if source == "SKIPPED" {
                format!("  ⏭️  {} → {}", target.red(), "SKIPPED".yellow())
            } else {
                let mut info = format!("  ✓ {} → {}", target.green(), source.blue());
                if !transform.is_empty() {
                    info.push_str(&format!(" {}", transform.magenta()));
                }
                if !fallback.is_empty() {
                    info.push_str(&format!(" {}", fallback.cyan()));
                }
                info
            };
            println!("{}", status);
        }
        println!();
    }
    
    // Show remaining fields
    let remaining_fields: Vec<&String> = target_fields
        .iter()
        .skip(current_field - 1)
        .collect();
    
    if !remaining_fields.is_empty() {
        println!("{}", "⏳ Remaining Fields:".yellow().bold());
        for field in remaining_fields.iter().take(5) {
            println!("  ○ {}", field.white());
        }
        if remaining_fields.len() > 5 {
            println!("  ... and {} more", remaining_fields.len() - 5);
        }
    }
    
    // Show progress bar
    let progress_percentage = (current_field as f64 / total_fields as f64 * 100.0) as usize;
    let progress_bar_length = 30;
    let filled_length = ((progress_percentage as f64 / 100.0) * progress_bar_length as f64) as usize;
    let filled_length = filled_length.min(progress_bar_length);
    let empty_length = progress_bar_length.saturating_sub(filled_length);
    
    let progress_bar = format!(
        "[{}{}] {}% ({}/{})",
        "█".repeat(filled_length),
        "░".repeat(empty_length),
        progress_percentage,
        current_field,
        total_fields
    );
    
    println!("{}", progress_bar.cyan());
    println!("{}", "─".repeat(80));
}

/// Applies a mapping to transform a document (with nested mapping support)
pub fn apply_mapping(doc: &Value, mapping: &Mapping) -> Result<Value> {
    let mut result = Value::Object(serde_json::Map::new());

    for field_mapping in &mapping.field_mappings {
        // Check if this is a nested mapping
        let source_path = extract_source_path(&field_mapping.from)?;
        
        if let Some(nested_path) = source_path {
            // Handle nested mapping
            let nested_mapping = NestedMapping {
                source_path: nested_path,
                target_path: NestedPath::from_dot_notation(&field_mapping.to)?,
                transform: None, // TODO: Add transform support for nested mappings
                flatten: false,
                separator: None,
            };
            
            let nested_result = apply_nested_mapping(doc, &nested_mapping)?;
            
            // Merge the nested result into the main result
            if let Value::Object(nested_obj) = nested_result {
                if let Value::Object(main_obj) = &mut result {
                    main_obj.extend(nested_obj);
                }
            }
        } else {
            // Handle regular mapping
            let value = extract_value(doc, &field_mapping.from, &field_mapping.transforms)?;

            if value.is_null() && field_mapping.fallback.is_some() {
                // Use fallback value if provided and value is null
                if let Some(fallback) = &field_mapping.fallback {
                    if let Value::Object(main_obj) = &mut result {
                        main_obj.insert(field_mapping.to.clone(), Value::String(fallback.clone()));
                    }
                }
            } else {
                // Use extracted value
                if let Value::Object(main_obj) = &mut result {
                    main_obj.insert(field_mapping.to.clone(), value);
                }
            }
        }
    }

    Ok(result)
}

/// Extracts source path from field mapping to determine if it's nested
fn extract_source_path(from: &Value) -> Result<Option<NestedPath>> {
    match from {
        Value::String(path) => {
            if path.contains('.') || path.contains('[') {
                Ok(Some(NestedPath::from_dot_notation(path)?))
            } else {
                Ok(None)
            }
        }
        Value::Array(paths) => {
            // For arrays, check if any path is nested
            for path in paths {
                if let Value::String(path_str) = path {
                    if path_str.contains('.') || path_str.contains('[') {
                        return Ok(Some(NestedPath::from_dot_notation(path_str)?));
                    }
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// Extracts a value from a document using the mapping
fn extract_value(doc: &Value, from: &Value, transforms: &[Value]) -> Result<Value> {
    // Extract the value based on the 'from' field
    let mut value = match from {
        Value::String(path) => {
            // Simple path extraction
            get_path(doc, path).cloned().unwrap_or(Value::Null)
        },
        Value::Array(paths) => {
            // Try multiple paths in order
            let mut result = Value::Null;
            for path in paths {
                if let Value::String(p) = path {
                    if let Some(v) = get_path(doc, p) {
                        if !v.is_null() {
                            result = v.clone();
                            break;
                        }
                    }
                }
            }
            result
        },
        _ => Value::Null
    };

    // Apply transforms
    for transform in transforms {
        // Special handling for combine_coordinates transform
        if let Some(transform_type) = transform.get("type").and_then(Value::as_str) {
            if transform_type == "combine_coordinates" {
                // For combine_coordinates, pass the original document instead of the extracted value
                value = apply_transform(doc, transform)?;
            } else {
                // For other transforms, apply to the extracted value
                value = apply_transform(&value, transform)?;
            }
        } else {
            value = apply_transform(&value, transform)?;
        }
    }

    Ok(value)
}

/// Applies a transform to a value
fn apply_transform(value: &Value, transform: &Value) -> Result<Value> {
    if let Some(transform_type) = transform.get("type").and_then(Value::as_str) {
        match transform_type {
            "to_point" => {
                if let Some(params) = transform.get("params") {
                    let lat_path = params.get("lat").and_then(Value::as_str).unwrap_or("lat");
                    let lng_path = params.get("lng").and_then(Value::as_str).unwrap_or("lng");

                    let lat = get_path(value, lat_path).unwrap_or(&Value::Null);
                    let lng = get_path(value, lng_path).unwrap_or(&Value::Null);

                    to_point_from_latlng(lat, lng)
                } else {
                    Err(anyhow!("Missing params for to_point transform"))
                }
            },
            "combine_coordinates" => {
                if let Some(params) = transform.get("params") {
                    let lat_field = params.get("lat_field").and_then(Value::as_str)
                        .ok_or_else(|| anyhow!("Missing lat_field parameter"))?;
                    let lng_field = params.get("lng_field").and_then(Value::as_str)
                        .ok_or_else(|| anyhow!("Missing lng_field parameter"))?;

                    // Use the helper function to combine coordinates from the document
                    combine_coordinates_from_fields(value, lat_field, lng_field)
                } else {
                    Err(anyhow!("Missing params for combine_coordinates transform"))
                }
            },
            "split_comma" => {
                if let Some(s) = value.as_str() {
                    let parts: Vec<&str> = s.split(',').map(|s| s.trim()).collect();
                    Ok(json!(parts))
                } else {
                    Ok(value.clone())
                }
            },
            _ => {
                warn!("Unknown transform type: {}", transform_type);
                Ok(value.clone())
            }
        }
    } else {
        Ok(value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;


    #[test]
    fn test_field_mapping_creation() {
        let mapping = FieldMapping {
            to: "title".to_string(),
            from: json!("nodeName"),
            transforms: Vec::new(),
            fallback: None,
        };

        assert_eq!(mapping.to, "title");
        assert_eq!(mapping.from, json!("nodeName"));
        assert!(mapping.transforms.is_empty());
        assert!(mapping.fallback.is_none());
    }

    #[test]
    fn test_mapping_config_creation() {
        let mappings = vec![
            FieldMapping {
                to: "title".to_string(),
                from: json!("nodeName"),
                transforms: Vec::new(),
                fallback: None,
            },
            FieldMapping {
                to: "body".to_string(),
                from: json!("content"),
                transforms: Vec::new(),
                fallback: None,
            },
        ];

        let config = Mapping {
            source: "umbraco".to_string(),
            target: "payload".to_string(),
            collection: "hotels".to_string(),
            field_mappings: mappings,
            block_mappings: None,
        };

        assert_eq!(config.source, "umbraco");
        assert_eq!(config.target, "payload");
        assert_eq!(config.collection, "hotels");
        assert_eq!(config.field_mappings.len(), 2);
        assert_eq!(config.field_mappings[0].to, "title");
        assert_eq!(config.field_mappings[1].to, "body");
    }

    #[test]
    fn test_combine_coordinates_transform() {
        let doc = json!({
            "latitude": 40.7128,
            "longitude": -74.0060,
            "name": "New York"
        });

        let transform = json!({
            "type": "combine_coordinates",
            "params": {
                "lat_field": "latitude",
                "lng_field": "longitude"
            }
        });

        let result = apply_transform(&doc, &transform).unwrap();
        
        assert_eq!(result, json!({
            "type": "Point",
            "coordinates": [-74.0060, 40.7128]
        }));
    }

    #[test]
    fn test_combine_coordinates_with_field_mapping() {
        // Simulate the user's scenario with rcasLatitude and rcasLongitude
        let doc = json!({
            "rcasLatitude": 40.7128,
            "rcasLongitude": -74.0060,
            "name": "Hotel A"
        });

        // Create a field mapping similar to what the user created
        let field_mapping = FieldMapping {
            to: "coordinates".to_string(),
            from: json!("rcasLatitude"), // This is the issue - we're extracting from rcasLatitude
            transforms: vec![json!({
                "type": "combine_coordinates",
                "params": {
                    "lat_field": "rcasLatitude",
                    "lng_field": "rcasLongitude"
                }
            })],
            fallback: None,
        };

        // Test the extract_value function directly
        let result = extract_value(&doc, &field_mapping.from, &field_mapping.transforms).unwrap();
        
        assert_eq!(result, json!({
            "type": "Point",
            "coordinates": [-74.0060, 40.7128]
        }));
    }

    #[test]
    fn test_save_and_load_mapping() -> Result<()> {
        let original_mapping = Mapping {
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
                    to: "body".to_string(),
                    from: json!("content"),
                    transforms: Vec::new(),
                    fallback: None,
                },
            ],
            block_mappings: None,
        };

        // Save mapping
        save_mapping(&original_mapping)?;

        // Load mapping
        let loaded_mapping = load_or_create_mapping("hotels", "umbraco", "payload", &vec![json!({})], None, false, None)?;

        // Verify they match
        assert_eq!(original_mapping.source, loaded_mapping.source);
        assert_eq!(original_mapping.target, loaded_mapping.target);
        assert_eq!(original_mapping.collection, loaded_mapping.collection);
        assert_eq!(original_mapping.field_mappings.len(), loaded_mapping.field_mappings.len());
        assert_eq!(original_mapping.field_mappings[0].to, loaded_mapping.field_mappings[0].to);
        assert_eq!(original_mapping.field_mappings[1].from, loaded_mapping.field_mappings[1].from);

        Ok(())
    }

    #[test]
    fn test_apply_mapping_simple() -> Result<()> {
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
            ],
            block_mappings: None,
        };

        let source_doc = json!({
            "nodeName": "Test Hotel",
            "content": "Some content",
            "unmapped": "should be ignored"
        });

        let result = apply_mapping(&source_doc, &mapping)?;

        assert_eq!(result["title"], "Test Hotel");
        assert!(!result.as_object().unwrap().contains_key("nodeName"));
        assert!(!result.as_object().unwrap().contains_key("unmapped"));

        Ok(())
    }

    #[test]
    fn test_apply_mapping_with_transform() -> Result<()> {
        let mapping = Mapping {
            source: "umbraco".to_string(),
            target: "payload".to_string(),
            collection: "hotels".to_string(),
            field_mappings: vec![
                FieldMapping {
                    to: "title".to_string(),
                    from: json!("nodeName"),
                    transforms: vec![json!({
                        "type": "uppercase"
                    })],
                    fallback: None,
                },
            ],
            block_mappings: None,
        };

        let source_doc = json!({
            "nodeName": "Test Hotel"
        });

        let result = apply_mapping(&source_doc, &mapping)?;

        // Note: The uppercase transform is not implemented yet
        // So the value should remain unchanged
        assert_eq!(result["title"], "Test Hotel"); // Without transform function, it just copies

        Ok(())
    }

    #[test]
    fn test_apply_mapping_missing_source_field() -> Result<()> {
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
            ],
            block_mappings: None,
        };

        let source_doc = json!({
            "content": "Some content"
            // nodeName is missing
        });

        let result = apply_mapping(&source_doc, &mapping)?;

        // When source field is missing, the target field should be null or not present
        // The current implementation sets it to null
        assert!(result["title"].is_null());

        Ok(())
    }

    #[test]
    fn test_extract_source_fields() {
        let doc = json!({
            "nodeName": "Hotel 1",
            "content": "Content 1",
            "rating": 5
        });

        let fields = extract_source_fields(&doc);
        let field_set: std::collections::HashSet<_> = fields.iter().collect();

        assert!(field_set.contains(&"nodeName".to_string()));
        assert!(field_set.contains(&"content".to_string()));
        assert!(field_set.contains(&"rating".to_string()));
        assert_eq!(field_set.len(), 3);
    }

    #[test]
    fn test_extract_source_fields_empty() {
        let doc = json!({});
        let fields = extract_source_fields(&doc);
        assert!(fields.is_empty());
    }

    #[test]
    fn test_extract_source_fields_nested() {
        let doc = json!({
            "nodeName": "Hotel 1",
            "metadata": {
                "rating": 5,
                "location": "City"
            }
        });

        let fields = extract_source_fields(&doc);
        let field_set: std::collections::HashSet<_> = fields.iter().collect();

        // Should only extract top-level fields, not nested ones
        assert!(field_set.contains(&"nodeName".to_string()));
        assert!(field_set.contains(&"metadata".to_string()));
        assert!(!field_set.contains(&"rating".to_string()));
        assert!(!field_set.contains(&"location".to_string()));
        assert_eq!(field_set.len(), 2);
    }

    #[test]
    fn test_mapping_file_path_generation() {
        let path = get_mapping_path("hotels", "umbraco", "payload");
        assert!(path.contains("hotels"));
        assert!(path.contains("umbraco"));
        assert!(path.contains("payload"));
        assert!(path.ends_with(".json"));
    }

    #[test]
    fn test_load_nonexistent_mapping() {
        let result = load_or_create_mapping("hotels", "umbraco", "payload", &vec![json!({})], None, false, None);
        // Should create a new mapping when file doesn't exist
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_block_type_from_path() {
        assert_eq!(extract_block_type_from_path("blocks.content.columns"), Some("content".to_string()));
        assert_eq!(extract_block_type_from_path("blocks.section.heading"), Some("section".to_string()));
        assert_eq!(extract_block_type_from_path("blocks.testimonials-blog.haveVideoSpinner"), Some("testimonials-blog".to_string()));
        assert_eq!(extract_block_type_from_path("title"), None);
        assert_eq!(extract_block_type_from_path("blocks"), None);
    }

    #[test]
    fn test_extract_acf_flexible_content() {
        let source_data = vec![
            json!({
                "acf": {
                    "flexible_content": [
                        {
                            "acf_fc_layout": "cms",
                            "content": "<h2>Welcome</h2>",
                            "title": "Welcome Page"
                        },
                        {
                            "acf_fc_layout": "testimonials",
                            "testimonial_text": "Great service!",
                            "author": "John Doe"
                        }
                    ]
                }
            })
        ];

        let blocks = extract_acf_flexible_content(&source_data).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].acf_fc_layout, "cms");
        assert_eq!(blocks[1].acf_fc_layout, "testimonials");
    }

    #[test]
    fn test_get_unique_acf_layouts() {
        let source_data = vec![
            json!({
                "acf": {
                    "flexible_content": [
                        {
                            "acf_fc_layout": "cms",
                            "content": "<h2>Welcome</h2>"
                        },
                        {
                            "acf_fc_layout": "testimonials",
                            "testimonial_text": "Great service!"
                        },
                        {
                            "acf_fc_layout": "cms",
                            "content": "<h2>Another CMS block</h2>"
                        }
                    ]
                }
            })
        ];

        let layouts = get_unique_acf_layouts(&source_data).unwrap();
        assert_eq!(layouts.len(), 2);
        assert!(layouts.contains(&"cms".to_string()));
        assert!(layouts.contains(&"testimonials".to_string()));
    }

    #[test]
    fn test_get_unique_payload_block_types() {
        let target_fields = vec![
            "title".to_string(),
            "blocks.content.columns".to_string(),
            "blocks.content.heading".to_string(),
            "blocks.section.media".to_string(),
            "blocks.testimonials-blog.haveVideoSpinner".to_string(),
            "slug".to_string(),
        ];

        let block_types = get_unique_payload_block_types(&target_fields);
        assert_eq!(block_types.len(), 3);
        assert!(block_types.contains(&"content".to_string()));
        assert!(block_types.contains(&"section".to_string()));
        assert!(block_types.contains(&"testimonials-blog".to_string()));
    }

    #[test]
    fn test_block_mappings_creation() {
        use crate::mapping::json_mapping::BlockMappings;
        use std::collections::HashMap;

        let mut block_mappings = BlockMappings::new();
        
        // Test layout to block type mapping
        block_mappings.layout_to_block_type.insert("cms".to_string(), "content".to_string());
        block_mappings.layout_to_block_type.insert("testimonials".to_string(), "testimonials-blog".to_string());
        
        assert_eq!(block_mappings.layout_to_block_type.len(), 2);
        assert_eq!(block_mappings.layout_to_block_type.get("cms"), Some(&"content".to_string()));
        assert_eq!(block_mappings.layout_to_block_type.get("testimonials"), Some(&"testimonials-blog".to_string()));
    }

    #[test]
    fn test_extract_acf_flexible_content_empty() {
        let source_data = vec![
            json!({
                "title": "Some page",
                "content": "Regular content"
            })
        ];

        let blocks = extract_acf_flexible_content(&source_data).unwrap();
        assert_eq!(blocks.len(), 0);
    }

    #[test]
    fn test_extract_acf_flexible_content_no_acf() {
        let source_data = vec![
            json!({
                "title": "Some page",
                "content": "Regular content"
            })
        ];

        let blocks = extract_acf_flexible_content(&source_data).unwrap();
        assert_eq!(blocks.len(), 0);
    }

    #[test]
    fn test_extract_acf_flexible_content_no_flexible_content() {
        let source_data = vec![
            json!({
                "acf": {
                    "regular_field": "value"
                }
            })
        ];

        let blocks = extract_acf_flexible_content(&source_data).unwrap();
        assert_eq!(blocks.len(), 0);
    }
}
