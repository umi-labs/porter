use crate::sources::wordpress::config::*;
use anyhow::{Context, Result};
use log::info;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;

/// WordPress WXR (WordPress Extended RSS) parser
pub struct WXRParser {
    config: WordPressConfig,
}

impl WXRParser {
    pub fn new(config: WordPressConfig) -> Self {
        Self { config }
    }

    /// Parse a WXR (XML) file
    pub fn parse_wxr_file(&self, file_path: &str) -> Result<Vec<Value>> {
        info!("Parsing WXR file: {}", file_path);

        let content = fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read WXR file: {}", file_path))?;

        // For now, implement a basic XML parser
        // In a production implementation, you'd want to use a proper XML parser like quick-xml
        self.parse_wxr_content(&content)
    }

    /// Parse a JSON file (WordPress REST API export or similar)
    pub fn parse_json_file(&self, file_path: &str) -> Result<Vec<Value>> {
        info!("Parsing JSON file: {}", file_path);

        let content = fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read JSON file: {}", file_path))?;

        let data: Value = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse JSON from file: {}", file_path))?;

        self.parse_json_data(&data)
    }

    /// Parse WXR content (basic implementation)
    fn parse_wxr_content(&self, content: &str) -> Result<Vec<Value>> {
        let mut documents = Vec::new();

        // This is a simplified WXR parser for the MVP
        // A full implementation would use proper XML parsing

        // Extract basic post information using regex (temporary solution)
        let posts = self.extract_posts_from_wxr(content)?;
        let users = self.extract_users_from_wxr(content)?;
        let terms = if self.config.include_taxonomies {
            self.extract_terms_from_wxr(content)?
        } else {
            HashMap::new()
        };

        info!(
            "Extracted {} posts, {} users, {} terms",
            posts.len(),
            users.len(),
            terms.len()
        );

        // Process each post
        for post in posts {
            if self.should_include_post(&post) {
                let processed_post = self.process_post(post, &users, &terms)?;
                documents.push(processed_post);
            }
        }

        // Add users if requested
        if self.config.include_users {
            for (_id, user) in users {
                documents.push(user);
            }
        }

        Ok(documents)
    }

    /// Parse JSON data
    fn parse_json_data(&self, data: &Value) -> Result<Vec<Value>> {
        let mut documents = Vec::new();

        // Handle different JSON structures
        match data {
            Value::Array(posts) => {
                // Direct array of posts
                for post in posts {
                    if let Some(processed_post) = self.process_json_post(post)? {
                        documents.push(processed_post);
                    }
                }
            }
            Value::Object(obj) => {
                // Check for common WordPress REST API structure
                if let Some(posts) = obj.get("posts").or_else(|| obj.get("data")) {
                    if let Some(posts_array) = posts.as_array() {
                        for post in posts_array {
                            if let Some(processed_post) = self.process_json_post(post)? {
                                documents.push(processed_post);
                            }
                        }
                    }
                } else {
                    // Single post object
                    if let Some(processed_post) = self.process_json_post(data)? {
                        documents.push(processed_post);
                    }
                }
            }
            _ => return Err(anyhow::anyhow!("Invalid JSON structure for WordPress data")),
        }

        Ok(documents)
    }

    /// Extract posts from WXR content (basic regex-based approach for MVP)
    fn extract_posts_from_wxr(&self, content: &str) -> Result<Vec<WXRPost>> {
        let mut posts = Vec::new();

        // This is a very basic implementation - in production, use proper XML parsing
        // Look for <item> tags that represent posts
        let item_pattern = regex::Regex::new(r"<item>(.*?)</item>")
            .map_err(|e| anyhow::anyhow!("Regex error: {}", e))?;

        for item_match in item_pattern.captures_iter(content) {
            let item_content = &item_match[1];

            if let Ok(post) = self.parse_wxr_item(item_content) {
                posts.push(post);
            }
        }

        Ok(posts)
    }

    /// Extract users from WXR content
    fn extract_users_from_wxr(&self, content: &str) -> Result<HashMap<String, Value>> {
        let mut users = HashMap::new();

        // Look for wp:author tags
        let author_pattern = regex::Regex::new(r"<wp:author>(.*?)</wp:author>")
            .map_err(|e| anyhow::anyhow!("Regex error: {}", e))?;

        for author_match in author_pattern.captures_iter(content) {
            let author_content = &author_match[1];

            if let Ok(user) = self.parse_wxr_author(author_content) {
                if let Some(id) = user.get("id").and_then(|v| v.as_str()) {
                    users.insert(id.to_string(), user);
                }
            }
        }

        Ok(users)
    }

    /// Extract taxonomy terms from WXR content
    fn extract_terms_from_wxr(&self, content: &str) -> Result<HashMap<String, Value>> {
        let mut terms = HashMap::new();

        // Look for wp:category and wp:tag
        let patterns = vec![
            ("category", r"<wp:category>(.*?)</wp:category>"),
            ("tag", r"<wp:tag>(.*?)</wp:tag>"),
        ];

        for (term_type, pattern_str) in patterns {
            let pattern = regex::Regex::new(pattern_str)
                .map_err(|e| anyhow::anyhow!("Regex error: {}", e))?;

            for term_match in pattern.captures_iter(content) {
                let term_content = &term_match[1];

                if let Ok(mut term) = self.parse_wxr_term(term_content) {
                    term["taxonomy"] = json!(term_type);

                    if let Some(id) = term.get("id").and_then(|v| v.as_str()) {
                        terms.insert(id.to_string(), term);
                    }
                }
            }
        }

        Ok(terms)
    }

    /// Parse a WXR item (post) - basic implementation
    fn parse_wxr_item(&self, content: &str) -> Result<WXRPost> {
        let mut post = WXRPost::default();

        // Extract basic fields using simple string matching
        // In production, use proper XML parsing

        if let Some(title) = self.extract_tag_content(content, "title") {
            post.title = title;
        }

        if let Some(content_encoded) = self.extract_cdata_content(content, "content:encoded") {
            post.content = content_encoded;
        }

        if let Some(excerpt) = self.extract_cdata_content(content, "excerpt:encoded") {
            post.excerpt = excerpt;
        }

        if let Some(post_id) = self.extract_tag_content(content, "wp:post_id") {
            post.id = post_id;
        }

        if let Some(post_type) = self.extract_tag_content(content, "wp:post_type") {
            post.post_type = post_type;
        }

        if let Some(status) = self.extract_tag_content(content, "wp:status") {
            post.status = PostStatus::from(status.as_str());
        }

        if let Some(post_name) = self.extract_tag_content(content, "wp:post_name") {
            post.slug = post_name;
        }

        // Extract post meta
        post.meta = self.extract_post_meta(content)?;

        Ok(post)
    }

    /// Parse a WXR author
    fn parse_wxr_author(&self, content: &str) -> Result<Value> {
        let mut author = serde_json::Map::new();

        if let Some(id) = self.extract_tag_content(content, "wp:author_id") {
            author.insert("id".to_string(), json!(id));
        }

        if let Some(login) = self.extract_tag_content(content, "wp:author_login") {
            author.insert("username".to_string(), json!(login));
        }

        if let Some(email) = self.extract_tag_content(content, "wp:author_email") {
            author.insert("email".to_string(), json!(email));
        }

        if let Some(display_name) = self.extract_tag_content(content, "wp:author_display_name") {
            author.insert("display_name".to_string(), json!(display_name));
        }

        author.insert("type".to_string(), json!("user"));

        Ok(Value::Object(author))
    }

    /// Parse a WXR term
    fn parse_wxr_term(&self, content: &str) -> Result<Value> {
        let mut term = serde_json::Map::new();

        if let Some(id) = self.extract_tag_content(content, "wp:term_id") {
            term.insert("id".to_string(), json!(id));
        }

        if let Some(name) = self
            .extract_cdata_content(content, "wp:cat_name")
            .or_else(|| self.extract_cdata_content(content, "wp:tag_name"))
        {
            term.insert("name".to_string(), json!(name));
        }

        if let Some(slug) = self
            .extract_tag_content(content, "wp:category_nicename")
            .or_else(|| self.extract_tag_content(content, "wp:tag_slug"))
        {
            term.insert("slug".to_string(), json!(slug));
        }

        term.insert("type".to_string(), json!("term"));

        Ok(Value::Object(term))
    }

    /// Extract content from XML tags
    fn extract_tag_content(&self, content: &str, tag: &str) -> Option<String> {
        let pattern = format!(r"<{}>(.*?)</{}>", tag, tag);
        if let Ok(regex) = regex::Regex::new(&pattern) {
            if let Some(captures) = regex.captures(content) {
                return Some(captures[1].to_string());
            }
        }
        None
    }

    /// Extract content from CDATA sections
    fn extract_cdata_content(&self, content: &str, tag: &str) -> Option<String> {
        let pattern = format!(r"<{}><!\[CDATA\[(.*?)\]\]></{}>", tag, tag);
        if let Ok(regex) = regex::Regex::new(&pattern) {
            if let Some(captures) = regex.captures(content) {
                return Some(captures[1].to_string());
            }
        }
        None
    }

    /// Extract post meta fields
    fn extract_post_meta(&self, content: &str) -> Result<HashMap<String, String>> {
        let mut meta = HashMap::new();

        let meta_pattern = regex::Regex::new(r"<wp:postmeta>(.*?)</wp:postmeta>")
            .map_err(|e| anyhow::anyhow!("Regex error: {}", e))?;

        for meta_match in meta_pattern.captures_iter(content) {
            let meta_content = &meta_match[1];

            let key = self.extract_cdata_content(meta_content, "wp:meta_key");
            let value = self.extract_cdata_content(meta_content, "wp:meta_value");

            if let (Some(k), Some(v)) = (key, value) {
                meta.insert(k, v);
            }
        }

        Ok(meta)
    }

    /// Check if a post should be included based on configuration
    fn should_include_post(&self, post: &WXRPost) -> bool {
        // Check content type
        if !self.config.content_types.contains(&post.post_type) {
            return false;
        }

        // Check status
        match post.status {
            PostStatus::Draft if !self.config.include_drafts => false,
            PostStatus::Private if !self.config.include_private => false,
            PostStatus::Trash => false, // Never include trash
            _ => true,
        }
    }

    /// Process a WXR post into Porter format
    fn process_post(
        &self,
        post: WXRPost,
        users: &HashMap<String, Value>,
        _terms: &HashMap<String, Value>,
    ) -> Result<Value> {
        let mut doc = json!({
            "id": post.id,
            "title": post.title,
            "content": post.content,
            "excerpt": post.excerpt,
            "status": post.status.to_string(),
            "post_type": post.post_type,
            "slug": post.slug,
            "type": "post" // Porter document type
        });

        // Add author information if available
        if let Some(author_id) = post.meta.get("post_author") {
            if let Some(author) = users.get(author_id) {
                doc["author"] = author.clone();
            }
        }

        // Process custom fields and meta
        if !post.meta.is_empty() {
            let mut custom_fields = serde_json::Map::new();
            let mut meta_fields = serde_json::Map::new();

            for (key, value) in post.meta {
                // Apply field mappings
                let target_key = self
                    .config
                    .field_mappings
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| key.clone());

                // Separate meta fields (starting with _) from custom fields
                if key.starts_with('_') {
                    meta_fields.insert(target_key, json!(value));
                } else {
                    custom_fields.insert(target_key, json!(value));
                }
            }

            if !custom_fields.is_empty() {
                doc["custom_fields"] = json!(custom_fields);
            }

            if !meta_fields.is_empty() {
                doc["meta"] = json!(meta_fields);
            }
        }

        Ok(doc)
    }

    /// Process a JSON post
    fn process_json_post(&self, post: &Value) -> Result<Option<Value>> {
        if !post.is_object() {
            return Ok(None);
        }

        let obj = post.as_object().unwrap();

        // Check if we should include this post
        if let Some(post_type) = obj
            .get("type")
            .or_else(|| obj.get("post_type"))
            .and_then(|v| v.as_str())
        {
            if !self.config.content_types.contains(&post_type.to_string()) {
                return Ok(None);
            }
        }

        // Basic processing - just normalize field names
        let mut processed = post.clone();

        // Apply field mappings
        if let Some(processed_obj) = processed.as_object_mut() {
            let mappings = self.config.field_mappings.clone();
            for (wp_field, target_field) in mappings {
                if let Some(value) = processed_obj.remove(&wp_field) {
                    processed_obj.insert(target_field, value);
                }
            }

            // Ensure we have a type field
            if !processed_obj.contains_key("type") {
                processed_obj.insert("type".to_string(), json!("post"));
            }
        }

        Ok(Some(processed))
    }
}

/// Represents a WordPress post from WXR
#[derive(Debug, Clone, Default)]
struct WXRPost {
    pub id: String,
    pub title: String,
    pub content: String,
    pub excerpt: String,
    pub status: PostStatus,
    pub post_type: String,
    pub slug: String,
    pub meta: HashMap<String, String>,
}

impl Default for PostStatus {
    fn default() -> Self {
        PostStatus::Draft
    }
}
