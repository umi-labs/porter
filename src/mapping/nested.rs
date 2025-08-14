use anyhow::{Result, anyhow};
use serde_json::Value;
use super::json_mapping::Mapping;

/// Nested field path representation
#[derive(Debug, Clone, PartialEq)]
pub struct NestedPath {
    pub segments: Vec<PathSegment>,
}

/// Path segment types
#[derive(Debug, Clone, PartialEq)]
pub enum PathSegment {
    /// Simple field name
    Field(String),
    /// Array index
    Index(usize),
    /// Array wildcard (all elements)
    Wildcard,
    /// Array slice (start..end)
    Slice(Option<usize>, Option<usize>),
}

/// Nested mapping configuration
#[derive(Debug, Clone)]
pub struct NestedMapping {
    pub source_path: NestedPath,
    pub target_path: NestedPath,
    pub transform: Option<String>,
    pub flatten: bool,
    pub separator: Option<String>,
}

impl NestedPath {
    /// Creates a new nested path from a dot-notation string
    pub fn from_dot_notation(path: &str) -> Result<Self> {
        if path.is_empty() {
            return Err(anyhow!("Path cannot be empty"));
        }

        let mut segments = Vec::new();
        let parts: Vec<&str> = path.split('.').collect();

        for part in parts {
            if part == "*" {
                segments.push(PathSegment::Wildcard);
            } else if part.contains('[') && part.contains(']') {
                // Handle array notation like "items[0]" or "items[1:3]"
                let bracket_start = part.find('[').unwrap();
                let field_name = &part[..bracket_start];
                let bracket_content = &part[bracket_start + 1..part.len() - 1];

                // Add the field name first
                if !field_name.is_empty() {
                    segments.push(PathSegment::Field(field_name.to_string()));
                }

                // Then add the array segment
                if bracket_content == "*" {
                    segments.push(PathSegment::Wildcard);
                } else if bracket_content.contains(':') {
                    // Handle slice notation like "1:3" or ":3" or "1:"
                    let slice_parts: Vec<&str> = bracket_content.split(':').collect();
                    if slice_parts.len() == 2 {
                        let start = if slice_parts[0].is_empty() {
                            None
                        } else {
                            Some(slice_parts[0].parse::<usize>().map_err(|_| {
                                anyhow!("Invalid slice start index: {}", slice_parts[0])
                            })?)
                        };
                        let end = if slice_parts[1].is_empty() {
                            None
                        } else {
                            Some(slice_parts[1].parse::<usize>().map_err(|_| {
                                anyhow!("Invalid slice end index: {}", slice_parts[1])
                            })?)
                        };
                        segments.push(PathSegment::Slice(start, end));
                    }
                } else {
                    // Handle single index
                    let index = bracket_content.parse::<usize>().map_err(|_| {
                        anyhow!("Invalid array index: {}", bracket_content)
                    })?;
                    segments.push(PathSegment::Index(index));
                }
            } else {
                segments.push(PathSegment::Field(part.to_string()));
            }
        }

        Ok(NestedPath { segments })
    }

    /// Converts the path back to dot notation
    pub fn to_dot_notation(&self) -> String {
        self.segments
            .iter()
            .map(|segment| match segment {
                PathSegment::Field(name) => name.clone(),
                PathSegment::Index(index) => format!("[{}]", index),
                PathSegment::Wildcard => "[*]".to_string(),
                PathSegment::Slice(start, end) => {
                    let start_str = start.map(|s| s.to_string()).unwrap_or_default();
                    let end_str = end.map(|e| e.to_string()).unwrap_or_default();
                    format!("[{}:{}]", start_str, end_str)
                }
            })
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// Extracts nested values from a JSON document
pub fn extract_nested_value(doc: &Value, path: &NestedPath) -> Result<Vec<Value>> {
    let mut results = vec![doc.clone()];

    for segment in &path.segments {
        let mut new_results = Vec::new();

        for value in &results {
            match segment {
                PathSegment::Field(field_name) => {
                    if let Some(field_value) = value.get(field_name) {
                        new_results.push(field_value.clone());
                    }
                }
                PathSegment::Index(index) => {
                    if let Some(array) = value.as_array() {
                        if *index < array.len() {
                            new_results.push(array[*index].clone());
                        }
                    }
                }
                PathSegment::Wildcard => {
                    if let Some(array) = value.as_array() {
                        new_results.extend(array.iter().cloned());
                    }
                }
                PathSegment::Slice(start, end) => {
                    if let Some(array) = value.as_array() {
                        let start_idx = start.unwrap_or(0);
                        let end_idx = end.unwrap_or(array.len());
                        if start_idx < array.len() && end_idx <= array.len() && start_idx < end_idx {
                            new_results.extend(array[start_idx..end_idx].iter().cloned());
                        }
                    }
                }
            }
        }

        results = new_results;
        if results.is_empty() {
            break;
        }
    }

    Ok(results)
}

/// Sets a nested value in a JSON document
pub fn set_nested_value(doc: &mut Value, path: &NestedPath, values: &[Value]) -> Result<()> {
    if path.segments.is_empty() {
        return Err(anyhow!("Cannot set value with empty path"));
    }

    let mut current = doc;
    let last_segment_idx = path.segments.len() - 1;

    // Navigate to the parent of the target location
    for (i, segment) in path.segments.iter().enumerate() {
        if i == last_segment_idx {
            // This is the final segment, set the value
            match segment {
                PathSegment::Field(field_name) => {
                    if values.len() == 1 {
                        current[field_name] = values[0].clone();
                    } else {
                        current[field_name] = Value::Array(values.to_vec());
                    }
                }
                PathSegment::Index(index) => {
                    if let Some(array) = current.as_array_mut() {
                        while array.len() <= *index {
                            array.push(Value::Null);
                        }
                        if values.len() == 1 {
                            array[*index] = values[0].clone();
                        } else {
                            array[*index] = Value::Array(values.to_vec());
                        }
                    } else {
                        return Err(anyhow!("Cannot set array index on non-array value"));
                    }
                }
                PathSegment::Wildcard => {
                    if let Some(array) = current.as_array_mut() {
                        if values.len() == 1 {
                            array.clear();
                            array.push(values[0].clone());
                        } else {
                            array.clear();
                            array.extend(values.iter().cloned());
                        }
                    } else {
                        return Err(anyhow!("Cannot set wildcard on non-array value"));
                    }
                }
                PathSegment::Slice(start, end) => {
                    if let Some(array) = current.as_array_mut() {
                        let start_idx = start.unwrap_or(0);
                        let end_idx = end.unwrap_or(array.len());
                        if start_idx < array.len() && end_idx <= array.len() && start_idx < end_idx {
                            array.drain(start_idx..end_idx);
                            if values.len() == 1 {
                                array.insert(start_idx, values[0].clone());
                            } else {
                                for (i, value) in values.iter().enumerate() {
                                    array.insert(start_idx + i, value.clone());
                                }
                            }
                        }
                    } else {
                        return Err(anyhow!("Cannot set slice on non-array value"));
                    }
                }
            }
        } else {
            // Navigate to the next level
            match segment {
                PathSegment::Field(field_name) => {
                    if !current.get(field_name).is_some() {
                        current[field_name] = Value::Object(serde_json::Map::new());
                    }
                    current = current.get_mut(field_name).unwrap();
                }
                PathSegment::Index(index) => {
                    if let Some(array) = current.as_array_mut() {
                        while array.len() <= *index {
                            array.push(Value::Null);
                        }
                        current = &mut array[*index];
                    } else {
                        return Err(anyhow!("Cannot navigate to array index on non-array value"));
                    }
                }
                PathSegment::Wildcard => {
                    return Err(anyhow!("Wildcard not supported in intermediate path segments"));
                }
                PathSegment::Slice(_, _) => {
                    return Err(anyhow!("Slice not supported in intermediate path segments"));
                }
            }
        }
    }

    Ok(())
}

/// Applies nested mappings to a document
pub fn apply_nested_mapping(doc: &Value, mapping: &NestedMapping) -> Result<Value> {
    let source_values = extract_nested_value(doc, &mapping.source_path)?;
    
    if source_values.is_empty() {
        return Ok(Value::Null);
    }

    let mut result = Value::Object(serde_json::Map::new());
    
    if mapping.flatten {
        // Flatten the values into a single string
        let flattened = source_values
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(mapping.separator.as_deref().unwrap_or(", "));
        set_nested_value(&mut result, &mapping.target_path, &[Value::String(flattened)])?;
    } else {
        // Set the values as-is
        set_nested_value(&mut result, &mapping.target_path, &source_values)?;
    }

    Ok(result)
}

/// Extends the regular mapping system with nested mapping support
pub fn apply_mapping_with_nested(doc: &Value, mapping: &Mapping) -> Result<Value> {
    let mut result = Value::Object(serde_json::Map::new());

    for field_mapping in &mapping.field_mappings {
        let source_path = extract_source_path(&field_mapping.from)?;
        
        if let Some(nested_path) = source_path {
            // Handle nested mapping
            let nested_mapping = NestedMapping {
                source_path: nested_path,
                target_path: NestedPath::from_dot_notation(&field_mapping.to)?,
                transform: None, // TODO: Add transform support
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
            // Handle regular mapping (existing logic)
            let value = super::extract_value(doc, &field_mapping.from, &field_mapping.transforms)?;
            
            if value.is_null() && field_mapping.fallback.is_some() {
                if let Some(fallback) = &field_mapping.fallback {
                    if let Value::Object(main_obj) = &mut result {
                        main_obj.insert(field_mapping.to.clone(), Value::String(fallback.clone()));
                    }
                }
            } else {
                if let Value::Object(main_obj) = &mut result {
                    main_obj.insert(field_mapping.to.clone(), value);
                }
            }
        }
    }

    Ok(result)
}

/// Extracts source path from field mapping
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_nested_path_creation() -> Result<()> {
        let path = NestedPath::from_dot_notation("user.profile.name")?;
        assert_eq!(path.segments.len(), 3);
        assert_eq!(path.segments[0], PathSegment::Field("user".to_string()));
        assert_eq!(path.segments[1], PathSegment::Field("profile".to_string()));
        assert_eq!(path.segments[2], PathSegment::Field("name".to_string()));
        Ok(())
    }

    #[test]
    fn test_array_path_creation() -> Result<()> {
        let path = NestedPath::from_dot_notation("items[0].name")?;
        assert_eq!(path.segments.len(), 3);
        assert_eq!(path.segments[0], PathSegment::Field("items".to_string()));
        assert_eq!(path.segments[1], PathSegment::Index(0));
        assert_eq!(path.segments[2], PathSegment::Field("name".to_string()));
        Ok(())
    }

    #[test]
    fn test_wildcard_path_creation() -> Result<()> {
        let path = NestedPath::from_dot_notation("items[*].name")?;
        assert_eq!(path.segments.len(), 3);
        assert_eq!(path.segments[0], PathSegment::Field("items".to_string()));
        assert_eq!(path.segments[1], PathSegment::Wildcard);
        assert_eq!(path.segments[2], PathSegment::Field("name".to_string()));
        Ok(())
    }

    #[test]
    fn test_slice_path_creation() -> Result<()> {
        let path = NestedPath::from_dot_notation("items[1:3].name")?;
        assert_eq!(path.segments.len(), 3);
        assert_eq!(path.segments[0], PathSegment::Field("items".to_string()));
        assert_eq!(path.segments[1], PathSegment::Slice(Some(1), Some(3)));
        assert_eq!(path.segments[2], PathSegment::Field("name".to_string()));
        Ok(())
    }

    #[test]
    fn test_extract_nested_value() -> Result<()> {
        let doc = json!({
            "user": {
                "profile": {
                    "name": "John Doe",
                    "email": "john@example.com"
                }
            }
        });

        let path = NestedPath::from_dot_notation("user.profile.name")?;
        let values = extract_nested_value(&doc, &path)?;
        
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], json!("John Doe"));
        Ok(())
    }

    #[test]
    fn test_extract_array_value() -> Result<()> {
        let doc = json!({
            "items": [
                {"name": "Item 1", "price": 10},
                {"name": "Item 2", "price": 20},
                {"name": "Item 3", "price": 30}
            ]
        });

        let path = NestedPath::from_dot_notation("items[1].name")?;
        let values = extract_nested_value(&doc, &path)?;
        
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], json!("Item 2"));
        Ok(())
    }

    #[test]
    fn test_extract_wildcard_values() -> Result<()> {
        let doc = json!({
            "items": [
                {"name": "Item 1", "price": 10},
                {"name": "Item 2", "price": 20},
                {"name": "Item 3", "price": 30}
            ]
        });

        let path = NestedPath::from_dot_notation("items[*].name")?;
        let values = extract_nested_value(&doc, &path)?;
        
        assert_eq!(values.len(), 3);
        assert_eq!(values[0], json!("Item 1"));
        assert_eq!(values[1], json!("Item 2"));
        assert_eq!(values[2], json!("Item 3"));
        Ok(())
    }

    #[test]
    fn test_set_nested_value() -> Result<()> {
        let mut doc = json!({});
        let path = NestedPath::from_dot_notation("user.profile.name")?;
        
        set_nested_value(&mut doc, &path, &[json!("John Doe")])?;
        
        assert_eq!(doc["user"]["profile"]["name"], json!("John Doe"));
        Ok(())
    }

    #[test]
    fn test_apply_nested_mapping() -> Result<()> {
        let doc = json!({
            "user": {
                "profile": {
                    "name": "John Doe",
                    "email": "john@example.com"
                }
            }
        });

        let mapping = NestedMapping {
            source_path: NestedPath::from_dot_notation("user.profile.name")?,
            target_path: NestedPath::from_dot_notation("title")?,
            transform: None,
            flatten: false,
            separator: None,
        };

        let result = apply_nested_mapping(&doc, &mapping)?;
        assert_eq!(result["title"], json!("John Doe"));
        Ok(())
    }
}
