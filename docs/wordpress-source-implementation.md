# WordPress Source Implementation

## Overview

This document details the implementation plan for WordPress as a source adapter in Porter. WordPress support includes multiple input methods (WXR XML exports, JSON dumps, and direct database connections) with comprehensive field processing capabilities.

## WordPress Data Structures

### Core Content Types

#### Posts and Pages
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPressPost {
    pub id: u64,
    pub title: String,
    pub content: String,
    pub excerpt: String,
    pub status: PostStatus,
    pub post_type: String,
    pub author: u64,
    pub date: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub featured_media: Option<u64>,
    pub categories: Vec<u64>,
    pub tags: Vec<u64>,
    pub meta: HashMap<String, Value>,
    pub acf_fields: Option<Value>,
    pub custom_fields: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PostStatus {
    Publish,
    Draft,
    Private,
    Pending,
    Trash,
    AutoDraft,
    Inherit,
    Custom(String),
}
```

#### Media Attachments
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPressAttachment {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub caption: String,
    pub alt_text: String,
    pub mime_type: String,
    pub file_url: String,
    pub file_path: Option<String>,
    pub file_size: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub upload_date: DateTime<Utc>,
    pub parent_post: Option<u64>,
    pub meta: HashMap<String, Value>,
}
```

#### Users and Authors
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPressUser {
    pub id: u64,
    pub username: String,
    pub email: String,
    pub display_name: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub description: Option<String>,
    pub website: Option<String>,
    pub roles: Vec<String>,
    pub capabilities: HashMap<String, bool>,
    pub meta: HashMap<String, Value>,
    pub registered_date: DateTime<Utc>,
}
```

#### Taxonomies (Categories, Tags, Custom)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPressTerm {
    pub id: u64,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub taxonomy: String,
    pub parent: Option<u64>,
    pub count: u64,
    pub meta: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPressTaxonomy {
    pub name: String,
    pub label: String,
    pub description: String,
    pub public: bool,
    pub hierarchical: bool,
    pub show_ui: bool,
    pub show_in_menu: bool,
    pub show_in_rest: bool,
}
```

#### Comments
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPressComment {
    pub id: u64,
    pub post_id: u64,
    pub author: String,
    pub author_email: String,
    pub author_url: Option<String>,
    pub author_ip: String,
    pub date: DateTime<Utc>,
    pub content: String,
    pub approved: bool,
    pub user_id: Option<u64>,
    pub parent: Option<u64>,
    pub meta: HashMap<String, Value>,
}
```

## Input Method Implementations

### 1. WXR (WordPress Extended RSS) Parser

#### WXR Format Structure
```xml
<?xml version="1.0" encoding="UTF-8" ?>
<rss version="2.0" 
     xmlns:excerpt="http://wordpress.org/export/1.2/excerpt/"
     xmlns:content="http://purl.org/rss/1.0/modules/content/"
     xmlns:wfw="http://wellformedweb.org/CommentAPI/"
     xmlns:dc="http://purl.org/dc/elements/1.1/"
     xmlns:wp="http://wordpress.org/export/1.2/">
    <channel>
        <wp:wxr_version>1.2</wp:wxr_version>
        <wp:base_site_url>https://example.com</wp:base_site_url>
        <wp:base_blog_url>https://example.com</wp:base_blog_url>
        
        <!-- Authors -->
        <wp:author>
            <wp:author_id>1</wp:author_id>
            <wp:author_login>admin</wp:author_login>
            <wp:author_email>admin@example.com</wp:author_email>
            <wp:author_display_name>Administrator</wp:author_display_name>
            <wp:author_first_name>John</wp:author_first_name>
            <wp:author_last_name>Doe</wp:author_last_name>
        </wp:author>
        
        <!-- Categories and Tags -->
        <wp:category>
            <wp:term_id>1</wp:term_id>
            <wp:category_nicename>uncategorized</wp:category_nicename>
            <wp:category_parent></wp:category_parent>
            <wp:cat_name><![CDATA[Uncategorized]]></wp:cat_name>
        </wp:category>
        
        <!-- Posts, Pages, Attachments -->
        <item>
            <title>Sample Post</title>
            <link>https://example.com/sample-post/</link>
            <pubDate>Wed, 15 Jun 2023 12:00:00 +0000</pubDate>
            <dc:creator><![CDATA[admin]]></dc:creator>
            <guid isPermaLink="false">https://example.com/?p=1</guid>
            <description></description>
            <content:encoded><![CDATA[Post content here]]></content:encoded>
            <excerpt:encoded><![CDATA[Post excerpt]]></excerpt:encoded>
            <wp:post_id>1</wp:post_id>
            <wp:post_date>2023-06-15 12:00:00</wp:post_date>
            <wp:post_date_gmt>2023-06-15 12:00:00</wp:post_date_gmt>
            <wp:comment_status>open</wp:comment_status>
            <wp:ping_status>open</wp:ping_status>
            <wp:post_name>sample-post</wp:post_name>
            <wp:status>publish</wp:status>
            <wp:post_parent>0</wp:post_parent>
            <wp:menu_order>0</wp:menu_order>
            <wp:post_type>post</wp:post_type>
            <wp:post_password></wp:post_password>
            <wp:is_sticky>0</wp:is_sticky>
            
            <!-- Categories and Tags -->
            <category domain="category" nicename="news"><![CDATA[News]]></category>
            <category domain="post_tag" nicename="important"><![CDATA[Important]]></category>
            
            <!-- Custom Fields -->
            <wp:postmeta>
                <wp:meta_key><![CDATA[_featured_image]]></wp:meta_key>
                <wp:meta_value><![CDATA[123]]></wp:meta_value>
            </wp:postmeta>
            
            <!-- Comments -->
            <wp:comment>
                <wp:comment_id>1</wp:comment_id>
                <wp:comment_author><![CDATA[John Commenter]]></wp:comment_author>
                <wp:comment_author_email><![CDATA[john@example.com]]></wp:comment_author_email>
                <wp:comment_author_url></wp:comment_author_url>
                <wp:comment_author_IP><![CDATA[192.168.1.1]]></wp:comment_author_IP>
                <wp:comment_date>2023-06-16 10:30:00</wp:comment_date>
                <wp:comment_date_gmt>2023-06-16 10:30:00</wp:comment_date_gmt>
                <wp:comment_content><![CDATA[Great post!]]></wp:comment_content>
                <wp:comment_approved>1</wp:comment_approved>
                <wp:comment_type></wp:comment_type>
                <wp:comment_parent>0</wp:comment_parent>
                <wp:comment_user_id>0</wp:comment_user_id>
            </wp:comment>
        </item>
    </channel>
</rss>
```

#### WXR Parser Implementation
```rust
// src/sources/wordpress/wxr_parser.rs
use quick_xml::{Reader, events::Event};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

pub struct WXRParser {
    field_processors: HashMap<String, Box<dyn FieldProcessor>>,
    config: WordPressConfig,
}

impl WXRParser {
    pub fn new(config: WordPressConfig) -> Self {
        let mut field_processors = HashMap::new();
        
        // Register WordPress-specific field processors
        field_processors.insert("_wp_attached_file".to_string(), 
            Box::new(AttachmentFileProcessor::new()) as Box<dyn FieldProcessor>);
        field_processors.insert("_wp_attachment_metadata".to_string(), 
            Box::new(AttachmentMetadataProcessor::new()) as Box<dyn FieldProcessor>);
        field_processors.insert("_featured_image".to_string(), 
            Box::new(FeaturedImageProcessor::new()) as Box<dyn FieldProcessor>);
        
        // ACF field processors
        if config.include_acf_fields {
            field_processors.insert("_acf".to_string(), 
                Box::new(ACFFieldProcessor::new()) as Box<dyn FieldProcessor>);
        }
        
        Self {
            field_processors,
            config,
        }
    }
    
    pub fn parse_file(&self, file_path: &str) -> Result<Vec<Value>> {
        let mut reader = Reader::from_file(file_path)?;
        reader.trim_text(true);
        
        let mut documents = Vec::new();
        let mut buf = Vec::new();
        
        // Track parsing state
        let mut current_item: Option<WXRItem> = None;
        let mut current_author: Option<WXRAuthor> = None;
        let mut current_category: Option<WXRCategory> = None;
        let mut authors: HashMap<u64, WXRAuthor> = HashMap::new();
        let mut categories: HashMap<u64, WXRCategory> = HashMap::new();
        let mut tags: HashMap<u64, WXRTag> = HashMap::new();
        
        loop {
            match reader.read_event(&mut buf)? {
                Event::Start(ref e) => {
                    match e.name() {
                        b"item" => {
                            current_item = Some(WXRItem::new());
                        }
                        b"wp:author" => {
                            current_author = Some(WXRAuthor::new());
                        }
                        b"wp:category" => {
                            current_category = Some(WXRCategory::new());
                        }
                        _ => {}
                    }
                }
                Event::End(ref e) => {
                    match e.name() {
                        b"item" => {
                            if let Some(item) = current_item.take() {
                                if self.should_include_item(&item) {
                                    let document = self.process_item(item, &authors, &categories, &tags)?;
                                    documents.push(document);
                                }
                            }
                        }
                        b"wp:author" => {
                            if let Some(author) = current_author.take() {
                                authors.insert(author.id, author);
                            }
                        }
                        b"wp:category" => {
                            if let Some(category) = current_category.take() {
                                categories.insert(category.id, category);
                            }
                        }
                        _ => {}
                    }
                }
                Event::Text(e) => {
                    // Handle text content for current element
                    self.handle_text_content(&e, &mut current_item, &mut current_author, &mut current_category)?;
                }
                Event::CData(e) => {
                    // Handle CDATA sections
                    self.handle_cdata_content(&e, &mut current_item, &mut current_author, &mut current_category)?;
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        
        Ok(documents)
    }
    
    fn should_include_item(&self, item: &WXRItem) -> bool {
        // Filter based on configuration
        if !self.config.content_types.contains(&item.post_type) {
            return false;
        }
        
        if !self.config.include_drafts && item.status != "publish" {
            return false;
        }
        
        if !self.config.include_private && item.status == "private" {
            return false;
        }
        
        true
    }
    
    fn process_item(
        &self, 
        item: WXRItem, 
        authors: &HashMap<u64, WXRAuthor>,
        categories: &HashMap<u64, WXRCategory>,
        tags: &HashMap<u64, WXRTag>
    ) -> Result<Value> {
        let mut doc = json!({
            "id": item.post_id,
            "title": item.title,
            "content": item.content,
            "excerpt": item.excerpt,
            "status": item.status,
            "post_type": item.post_type,
            "created_at": item.post_date,
            "updated_at": item.post_modified,
            "slug": item.post_name,
            "featured_media": item.featured_media,
        });
        
        // Add author information
        if let Some(author) = authors.get(&item.author_id) {
            doc["author"] = json!({
                "id": author.id,
                "username": author.login,
                "display_name": author.display_name,
                "email": author.email,
            });
        }
        
        // Process categories and tags
        doc["categories"] = json!(item.categories.iter()
            .filter_map(|cat_id| categories.get(cat_id))
            .map(|cat| json!({
                "id": cat.id,
                "name": cat.name,
                "slug": cat.slug,
            }))
            .collect::<Vec<_>>());
            
        doc["tags"] = json!(item.tags.iter()
            .filter_map(|tag_id| tags.get(tag_id))
            .map(|tag| json!({
                "id": tag.id,
                "name": tag.name,
                "slug": tag.slug,
            }))
            .collect::<Vec<_>>());
        
        // Process custom fields and meta
        if !item.postmeta.is_empty() {
            doc["custom_fields"] = Value::Object(serde_json::Map::new());
            doc["meta"] = Value::Object(serde_json::Map::new());
            
            for meta in item.postmeta {
                if let Some(processor) = self.field_processors.get(&meta.key) {
                    let context = ProcessingContext {
                        source_type: "wordpress".to_string(),
                        item_id: item.post_id.to_string(),
                        field_name: meta.key.clone(),
                        additional_data: HashMap::new(),
                    };
                    
                    let processed_value = processor.process_field(&Value::String(meta.value), &context)?;
                    doc["custom_fields"][&meta.key] = processed_value;
                } else {
                    // Handle meta fields vs custom fields
                    if meta.key.starts_with('_') {
                        doc["meta"][&meta.key] = Value::String(meta.value);
                    } else {
                        doc["custom_fields"][&meta.key] = Value::String(meta.value);
                    }
                }
            }
        }
        
        // Process comments if included
        if self.config.include_comments && !item.comments.is_empty() {
            doc["comments"] = json!(item.comments.iter()
                .map(|comment| json!({
                    "id": comment.id,
                    "author": comment.author,
                    "email": comment.author_email,
                    "content": comment.content,
                    "date": comment.date,
                    "approved": comment.approved,
                    "parent": comment.parent,
                }))
                .collect::<Vec<_>>());
        }
        
        Ok(doc)
    }
}

// Supporting structures for WXR parsing
#[derive(Debug, Clone)]
struct WXRItem {
    pub post_id: u64,
    pub title: String,
    pub content: String,
    pub excerpt: String,
    pub status: String,
    pub post_type: String,
    pub post_name: String,
    pub post_date: DateTime<Utc>,
    pub post_modified: DateTime<Utc>,
    pub author_id: u64,
    pub featured_media: Option<u64>,
    pub categories: Vec<u64>,
    pub tags: Vec<u64>,
    pub postmeta: Vec<WXRPostMeta>,
    pub comments: Vec<WXRComment>,
}

#[derive(Debug, Clone)]
struct WXRPostMeta {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
struct WXRComment {
    pub id: u64,
    pub author: String,
    pub author_email: String,
    pub content: String,
    pub date: DateTime<Utc>,
    pub approved: bool,
    pub parent: Option<u64>,
}

#[derive(Debug, Clone)]
struct WXRAuthor {
    pub id: u64,
    pub login: String,
    pub email: String,
    pub display_name: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Clone)]
struct WXRCategory {
    pub id: u64,
    pub name: String,
    pub slug: String,
    pub parent: Option<u64>,
}

#[derive(Debug, Clone)]
struct WXRTag {
    pub id: u64,
    pub name: String,
    pub slug: String,
}
```

### 2. JSON Dump Parser

```rust
// src/sources/wordpress/json_parser.rs
pub struct WordPressJSONParser {
    config: WordPressConfig,
    field_processors: HashMap<String, Box<dyn FieldProcessor>>,
}

impl WordPressJSONParser {
    pub fn parse_file(&self, file_path: &str) -> Result<Vec<Value>> {
        let content = std::fs::read_to_string(file_path)?;
        let data: Value = serde_json::from_str(&content)?;
        
        match data {
            Value::Array(items) => {
                items.into_iter()
                    .map(|item| self.process_json_item(item))
                    .collect()
            }
            Value::Object(obj) => {
                // Handle different JSON structures
                if let Some(posts) = obj.get("posts") {
                    self.parse_posts_array(posts.clone())
                } else if let Some(data) = obj.get("data") {
                    self.parse_posts_array(data.clone())
                } else {
                    // Single item
                    Ok(vec![self.process_json_item(Value::Object(obj))?])
                }
            }
            _ => Err(anyhow!("Invalid JSON structure")),
        }
    }
    
    fn process_json_item(&self, mut item: Value) -> Result<Value> {
        // Normalize field names
        self.normalize_field_names(&mut item)?;
        
        // Process custom fields
        if let Some(meta) = item.get("meta").cloned() {
            self.process_meta_fields(&mut item, meta)?;
        }
        
        // Process ACF fields
        if let Some(acf) = item.get("acf").cloned() {
            self.process_acf_fields(&mut item, acf)?;
        }
        
        // Process featured media
        if let Some(featured_media) = item.get("featured_media") {
            self.process_featured_media(&mut item, featured_media.clone())?;
        }
        
        Ok(item)
    }
    
    fn normalize_field_names(&self, item: &mut Value) -> Result<()> {
        if let Value::Object(obj) = item {
            // WordPress -> Porter field name mapping
            let field_mappings = &self.config.field_mappings;
            
            for (wp_field, porter_field) in field_mappings {
                if let Some(value) = obj.remove(wp_field) {
                    obj.insert(porter_field.clone(), value);
                }
            }
            
            // Standard field normalizations
            if let Some(post_date) = obj.remove("post_date") {
                obj.insert("created_at".to_string(), post_date);
            }
            if let Some(post_modified) = obj.remove("post_modified") {
                obj.insert("updated_at".to_string(), post_modified);
            }
            if let Some(post_content) = obj.remove("post_content") {
                obj.insert("content".to_string(), post_content);
            }
            if let Some(post_title) = obj.remove("post_title") {
                obj.insert("title".to_string(), post_title);
            }
        }
        
        Ok(())
    }
}
```

### 3. Database Connector

```rust
// src/sources/wordpress/database_connector.rs
use sqlx::{MySql, Pool, Row};

pub struct WordPressDatabaseConnector {
    pool: Pool<MySql>,
    config: WordPressDatabaseConfig,
    table_prefix: String,
}

impl WordPressDatabaseConnector {
    pub async fn new(connection_string: &str, config: WordPressDatabaseConfig) -> Result<Self> {
        let pool = Pool::<MySql>::connect(connection_string).await?;
        
        // Detect table prefix
        let prefix = Self::detect_table_prefix(&pool).await?;
        
        Ok(Self {
            pool,
            config,
            table_prefix: prefix,
        })
    }
    
    pub async fn read_posts(&self, query_config: &QueryConfig) -> Result<Vec<Value>> {
        let mut posts = Vec::new();
        
        // Build dynamic query based on configuration
        let base_query = format!(
            "SELECT p.*, u.display_name as author_name, u.user_email as author_email 
             FROM {}posts p 
             JOIN {}users u ON p.post_author = u.ID 
             WHERE p.post_status IN ({})",
            self.table_prefix,
            self.table_prefix,
            query_config.statuses.iter()
                .map(|s| format!("'{}'", s))
                .collect::<Vec<_>>()
                .join(", ")
        );
        
        // Add post type filter
        let query = if !query_config.post_types.is_empty() {
            format!("{} AND p.post_type IN ({})",
                base_query,
                query_config.post_types.iter()
                    .map(|pt| format!("'{}'", pt))
                    .collect::<Vec<_>>()
                    .join(", "))
        } else {
            base_query
        };
        
        let rows = sqlx::query(&query)
            .fetch_all(&self.pool)
            .await?;
            
        for row in rows {
            let post_id: u64 = row.get("ID");
            
            let mut post = json!({
                "id": post_id,
                "title": row.get::<String, _>("post_title"),
                "content": row.get::<String, _>("post_content"),
                "excerpt": row.get::<String, _>("post_excerpt"),
                "status": row.get::<String, _>("post_status"),
                "post_type": row.get::<String, _>("post_type"),
                "created_at": row.get::<chrono::DateTime<Utc>, _>("post_date"),
                "updated_at": row.get::<chrono::DateTime<Utc>, _>("post_modified"),
                "slug": row.get::<String, _>("post_name"),
                "author": {
                    "id": row.get::<u64, _>("post_author"),
                    "display_name": row.get::<String, _>("author_name"),
                    "email": row.get::<String, _>("author_email"),
                }
            });
            
            // Fetch and process meta fields
            if self.config.include_meta {
                let meta = self.fetch_post_meta(post_id).await?;
                post["meta"] = meta;
            }
            
            // Fetch and process taxonomies
            if self.config.include_taxonomies {
                let taxonomies = self.fetch_post_taxonomies(post_id).await?;
                post["taxonomies"] = taxonomies;
            }
            
            // Fetch and process comments
            if self.config.include_comments {
                let comments = self.fetch_post_comments(post_id).await?;
                post["comments"] = comments;
            }
            
            posts.push(post);
        }
        
        Ok(posts)
    }
    
    async fn fetch_post_meta(&self, post_id: u64) -> Result<Value> {
        let query = format!(
            "SELECT meta_key, meta_value FROM {}postmeta WHERE post_id = ?",
            self.table_prefix
        );
        
        let rows = sqlx::query(&query)
            .bind(post_id)
            .fetch_all(&self.pool)
            .await?;
            
        let mut meta = serde_json::Map::new();
        
        for row in rows {
            let key: String = row.get("meta_key");
            let value: String = row.get("meta_value");
            
            // Process specific meta keys
            if key == "_wp_attachment_metadata" {
                // Parse serialized PHP data
                if let Ok(parsed) = self.parse_php_serialized(&value) {
                    meta.insert(key, parsed);
                } else {
                    meta.insert(key, Value::String(value));
                }
            } else if key.starts_with("_acf") {
                // Handle ACF fields
                meta.insert(key, Value::String(value));
            } else {
                meta.insert(key, Value::String(value));
            }
        }
        
        Ok(Value::Object(meta))
    }
    
    async fn fetch_post_taxonomies(&self, post_id: u64) -> Result<Value> {
        let query = format!(
            "SELECT t.name, t.slug, tt.taxonomy, tt.term_id 
             FROM {}term_relationships tr
             JOIN {}term_taxonomy tt ON tr.term_taxonomy_id = tt.term_taxonomy_id
             JOIN {}terms t ON tt.term_id = t.term_id
             WHERE tr.object_id = ?",
            self.table_prefix, self.table_prefix, self.table_prefix
        );
        
        let rows = sqlx::query(&query)
            .bind(post_id)
            .fetch_all(&self.pool)
            .await?;
            
        let mut taxonomies: HashMap<String, Vec<Value>> = HashMap::new();
        
        for row in rows {
            let taxonomy: String = row.get("taxonomy");
            let term = json!({
                "id": row.get::<u64, _>("term_id"),
                "name": row.get::<String, _>("name"),
                "slug": row.get::<String, _>("slug"),
            });
            
            taxonomies.entry(taxonomy)
                .or_insert_with(Vec::new)
                .push(term);
        }
        
        Ok(json!(taxonomies))
    }
    
    async fn detect_table_prefix(pool: &Pool<MySql>) -> Result<String> {
        // Try common table names to detect prefix
        let test_queries = vec![
            "SHOW TABLES LIKE '%posts'",
            "SHOW TABLES LIKE 'wp_%'",
        ];
        
        for query in test_queries {
            let rows = sqlx::query(query)
                .fetch_all(pool)
                .await?;
                
            if !rows.is_empty() {
                if let Ok(table_name) = rows[0].try_get::<String, _>(0) {
                    if table_name.ends_with("posts") {
                        return Ok(table_name.replace("posts", ""));
                    }
                }
            }
        }
        
        // Default to wp_ if detection fails
        Ok("wp_".to_string())
    }
    
    fn parse_php_serialized(&self, data: &str) -> Result<Value> {
        // Basic PHP serialized data parser
        // This would need a proper implementation or external library
        // For now, return as string
        Ok(Value::String(data.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct WordPressDatabaseConfig {
    pub include_meta: bool,
    pub include_taxonomies: bool,
    pub include_comments: bool,
    pub batch_size: usize,
    pub table_prefix: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QueryConfig {
    pub post_types: Vec<String>,
    pub statuses: Vec<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub order_by: String,
    pub order_direction: String,
}
```

## WordPress Field Processors

### Core Field Processors

```rust
// src/sources/wordpress/field_processors/mod.rs
pub mod shortcode;
pub mod acf;
pub mod media;
pub mod gallery;
pub mod embed;
pub mod custom_fields;

use super::*;

// WordPress Shortcode Processor
pub struct WordPressShortcodeProcessor {
    shortcode_handlers: HashMap<String, Box<dyn ShortcodeHandler>>,
}

impl WordPressShortcodeProcessor {
    pub fn new() -> Self {
        let mut handlers: HashMap<String, Box<dyn ShortcodeHandler>> = HashMap::new();
        
        // Register built-in shortcode handlers
        handlers.insert("gallery".to_string(), Box::new(GalleryShortcodeHandler::new()));
        handlers.insert("caption".to_string(), Box::new(CaptionShortcodeHandler::new()));
        handlers.insert("embed".to_string(), Box::new(EmbedShortcodeHandler::new()));
        handlers.insert("audio".to_string(), Box::new(AudioShortcodeHandler::new()));
        handlers.insert("video".to_string(), Box::new(VideoShortcodeHandler::new()));
        
        Self {
            shortcode_handlers: handlers,
        }
    }
}

impl FieldProcessor for WordPressShortcodeProcessor {
    fn name(&self) -> &str {
        "wordpress_shortcode"
    }
    
    fn description(&self) -> &str {
        "Processes WordPress shortcodes in content"
    }
    
    fn supported_types(&self) -> Vec<FieldType> {
        vec![FieldType::String, FieldType::Text, FieldType::RichText]
    }
    
    fn process_field(&self, field_value: &Value, context: &ProcessingContext) -> Result<Value> {
        if let Some(content) = field_value.as_str() {
            let processed_content = self.process_shortcodes(content, context)?;
            Ok(Value::String(processed_content))
        } else {
            Ok(field_value.clone())
        }
    }
}

impl WordPressShortcodeProcessor {
    fn process_shortcodes(&self, content: &str, context: &ProcessingContext) -> Result<String> {
        let shortcode_regex = regex::Regex::new(r"\[(\w+)([^\]]*)\]([^\[]*)\[/\1\]|\[(\w+)([^\]]*)\]")?;
        let mut processed_content = content.to_string();
        
        for caps in shortcode_regex.captures_iter(content) {
            let shortcode_name = caps.get(1).or(caps.get(4)).unwrap().as_str();
            let attributes = caps.get(2).or(caps.get(5)).unwrap().as_str();
            let inner_content = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let full_match = caps.get(0).unwrap().as_str();
            
            if let Some(handler) = self.shortcode_handlers.get(shortcode_name) {
                let shortcode = Shortcode {
                    name: shortcode_name.to_string(),
                    attributes: self.parse_attributes(attributes)?,
                    content: inner_content.to_string(),
                };
                
                let replacement = handler.handle_shortcode(shortcode, context)?;
                processed_content = processed_content.replace(full_match, &replacement);
            }
        }
        
        Ok(processed_content)
    }
    
    fn parse_attributes(&self, attr_string: &str) -> Result<HashMap<String, String>> {
        let mut attributes = HashMap::new();
        let attr_regex = regex::Regex::new(r#"(\w+)=["']([^"']+)["']"#)?;
        
        for caps in attr_regex.captures_iter(attr_string) {
            let key = caps.get(1).unwrap().as_str().to_string();
            let value = caps.get(2).unwrap().as_str().to_string();
            attributes.insert(key, value);
        }
        
        Ok(attributes)
    }
}

// Shortcode handling traits and implementations
pub trait ShortcodeHandler: Send + Sync {
    fn handle_shortcode(&self, shortcode: Shortcode, context: &ProcessingContext) -> Result<String>;
}

#[derive(Debug, Clone)]
pub struct Shortcode {
    pub name: String,
    pub attributes: HashMap<String, String>,
    pub content: String,
}

// Gallery shortcode handler
pub struct GalleryShortcodeHandler;

impl GalleryShortcodeHandler {
    pub fn new() -> Self {
        Self
    }
}

impl ShortcodeHandler for GalleryShortcodeHandler {
    fn handle_shortcode(&self, shortcode: Shortcode, _context: &ProcessingContext) -> Result<String> {
        let ids = shortcode.attributes.get("ids")
            .ok_or_else(|| anyhow!("Gallery shortcode missing 'ids' attribute"))?;
            
        let image_ids: Vec<u64> = ids.split(',')
            .filter_map(|id| id.trim().parse().ok())
            .collect();
            
        // Convert to structured data for target processing
        let gallery_data = json!({
            "type": "gallery",
            "image_ids": image_ids,
            "columns": shortcode.attributes.get("columns").and_then(|c| c.parse::<u32>().ok()).unwrap_or(3),
            "size": shortcode.attributes.get("size").unwrap_or(&"medium".to_string()),
            "link": shortcode.attributes.get("link").unwrap_or(&"file".to_string()),
        });
        
        Ok(format!("{{{{GALLERY:{}}}}}", gallery_data.to_string()))
    }
}
```

### Advanced Custom Fields (ACF) Processor

```rust
// src/sources/wordpress/field_processors/acf.rs
pub struct ACFFieldProcessor {
    field_type_handlers: HashMap<String, Box<dyn ACFFieldHandler>>,
}

impl ACFFieldProcessor {
    pub fn new() -> Self {
        let mut handlers: HashMap<String, Box<dyn ACFFieldHandler>> = HashMap::new();
        
        // Register ACF field type handlers
        handlers.insert("text".to_string(), Box::new(ACFTextFieldHandler));
        handlers.insert("textarea".to_string(), Box::new(ACFTextAreaFieldHandler));
        handlers.insert("number".to_string(), Box::new(ACFNumberFieldHandler));
        handlers.insert("range".to_string(), Box::new(ACFRangeFieldHandler));
        handlers.insert("email".to_string(), Box::new(ACFEmailFieldHandler));
        handlers.insert("url".to_string(), Box::new(ACFUrlFieldHandler));
        handlers.insert("password".to_string(), Box::new(ACFPasswordFieldHandler));
        handlers.insert("image".to_string(), Box::new(ACFImageFieldHandler));
        handlers.insert("file".to_string(), Box::new(ACFFileFieldHandler));
        handlers.insert("wysiwyg".to_string(), Box::new(ACFWysiwygFieldHandler));
        handlers.insert("oembed".to_string(), Box::new(ACFOEmbedFieldHandler));
        handlers.insert("gallery".to_string(), Box::new(ACFGalleryFieldHandler));
        handlers.insert("select".to_string(), Box::new(ACFSelectFieldHandler));
        handlers.insert("checkbox".to_string(), Box::new(ACFCheckboxFieldHandler));
        handlers.insert("radio".to_string(), Box::new(ACFRadioFieldHandler));
        handlers.insert("button_group".to_string(), Box::new(ACFButtonGroupFieldHandler));
        handlers.insert("true_false".to_string(), Box::new(ACFTrueFalseFieldHandler));
        handlers.insert("link".to_string(), Box::new(ACFLinkFieldHandler));
        handlers.insert("post_object".to_string(), Box::new(ACFPostObjectFieldHandler));
        handlers.insert("page_link".to_string(), Box::new(ACFPageLinkFieldHandler));
        handlers.insert("relationship".to_string(), Box::new(ACFRelationshipFieldHandler));
        handlers.insert("taxonomy".to_string(), Box::new(ACFTaxonomyFieldHandler));
        handlers.insert("user".to_string(), Box::new(ACFUserFieldHandler));
        handlers.insert("google_map".to_string(), Box::new(ACFGoogleMapFieldHandler));
        handlers.insert("date_picker".to_string(), Box::new(ACFDatePickerFieldHandler));
        handlers.insert("date_time_picker".to_string(), Box::new(ACFDateTimePickerFieldHandler));
        handlers.insert("time_picker".to_string(), Box::new(ACFTimePickerFieldHandler));
        handlers.insert("color_picker".to_string(), Box::new(ACFColorPickerFieldHandler));
        handlers.insert("repeater".to_string(), Box::new(ACFRepeaterFieldHandler));
        handlers.insert("flexible_content".to_string(), Box::new(ACFFlexibleContentFieldHandler));
        handlers.insert("clone".to_string(), Box::new(ACFCloneFieldHandler));
        handlers.insert("group".to_string(), Box::new(ACFGroupFieldHandler));
        
        Self {
            field_type_handlers: handlers,
        }
    }
}

impl FieldProcessor for ACFFieldProcessor {
    fn name(&self) -> &str {
        "acf_field_processor"
    }
    
    fn description(&self) -> &str {
        "Processes Advanced Custom Fields (ACF) data"
    }
    
    fn supported_types(&self) -> Vec<FieldType> {
        vec![FieldType::Object, FieldType::Array, FieldType::String]
    }
    
    fn process_field(&self, field_value: &Value, context: &ProcessingContext) -> Result<Value> {
        match field_value {
            Value::Object(obj) => {
                let mut processed_fields = serde_json::Map::new();
                
                for (field_name, field_data) in obj {
                    if let Some(field_config) = self.get_field_config(field_name, context)? {
                        let field_type = field_config.get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("text");
                            
                        if let Some(handler) = self.field_type_handlers.get(field_type) {
                            let processed_value = handler.process_acf_field(field_data, &field_config, context)?;
                            processed_fields.insert(field_name.clone(), processed_value);
                        } else {
                            processed_fields.insert(field_name.clone(), field_data.clone());
                        }
                    } else {
                        processed_fields.insert(field_name.clone(), field_data.clone());
                    }
                }
                
                Ok(Value::Object(processed_fields))
            }
            _ => Ok(field_value.clone()),
        }
    }
}

pub trait ACFFieldHandler: Send + Sync {
    fn process_acf_field(&self, field_value: &Value, field_config: &Value, context: &ProcessingContext) -> Result<Value>;
}

// Example ACF field handlers
pub struct ACFImageFieldHandler;

impl ACFFieldHandler for ACFImageFieldHandler {
    fn process_acf_field(&self, field_value: &Value, _field_config: &Value, _context: &ProcessingContext) -> Result<Value> {
        match field_value {
            Value::String(image_id) => {
                // Convert image ID to structured data
                Ok(json!({
                    "type": "media",
                    "media_id": image_id.parse::<u64>().unwrap_or(0),
                }))
            }
            Value::Object(image_obj) => {
                // Already structured image data
                Ok(json!({
                    "type": "media",
                    "media_id": image_obj.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                    "url": image_obj.get("url"),
                    "alt": image_obj.get("alt"),
                    "title": image_obj.get("title"),
                    "width": image_obj.get("width"),
                    "height": image_obj.get("height"),
                }))
            }
            _ => Ok(field_value.clone()),
        }
    }
}

pub struct ACFRepeaterFieldHandler;

impl ACFFieldHandler for ACFRepeaterFieldHandler {
    fn process_acf_field(&self, field_value: &Value, field_config: &Value, context: &ProcessingContext) -> Result<Value> {
        if let Value::Array(repeater_items) = field_value {
            let processed_items: Result<Vec<Value>> = repeater_items
                .iter()
                .map(|item| {
                    if let Value::Object(item_obj) = item {
                        let mut processed_item = serde_json::Map::new();
                        
                        for (sub_field_name, sub_field_value) in item_obj {
                            // Process each sub-field based on its configuration
                            if let Some(sub_fields) = field_config.get("sub_fields").and_then(|sf| sf.as_array()) {
                                if let Some(sub_field_config) = sub_fields.iter()
                                    .find(|sf| sf.get("name").and_then(|n| n.as_str()) == Some(sub_field_name)) {
                                    
                                    let sub_field_type = sub_field_config.get("type")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("text");
                                        
                                    // Process sub-field recursively if needed
                                    processed_item.insert(sub_field_name.clone(), sub_field_value.clone());
                                } else {
                                    processed_item.insert(sub_field_name.clone(), sub_field_value.clone());
                                }
                            } else {
                                processed_item.insert(sub_field_name.clone(), sub_field_value.clone());
                            }
                        }
                        
                        Ok(Value::Object(processed_item))
                    } else {
                        Ok(item.clone())
                    }
                })
                .collect();
                
            Ok(Value::Array(processed_items?))
        } else {
            Ok(field_value.clone())
        }
    }
}
```

## WordPress Source Configuration

```rust
// src/sources/wordpress/config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPressConfig {
    pub format: WordPressFormat,
    pub content_types: Vec<String>,
    pub include_drafts: bool,
    pub include_private: bool,
    pub include_media: bool,
    pub include_comments: bool,
    pub include_users: bool,
    pub include_taxonomies: bool,
    pub include_acf_fields: bool,
    pub field_mappings: HashMap<String, String>,
    pub custom_field_processors: HashMap<String, String>,
    pub shortcode_processing: bool,
    pub media_processing_config: MediaProcessingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WordPressFormat {
    WXR,
    JSON,
    Api,
    Database,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WordPressFormat {
    WXR,
    JSON,
    Api,
    Database,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaProcessingConfig {
    pub download_media: bool,
    pub media_base_path: String,
    pub resize_images: bool,
    pub max_image_width: Option<u32>,
    pub max_image_height: Option<u32>,
    pub generate_thumbnails: bool,
    pub thumbnail_sizes: Vec<ThumbnailSize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThumbnailSize {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub crop: bool,
}

impl Default for WordPressConfig {
    fn default() -> Self {
        Self {
            format: WordPressFormat::WXR,
            content_types: vec!["post".to_string(), "page".to_string()],
            include_drafts: false,
            include_private: false,
            include_media: true,
            include_comments: false,
            include_users: true,
            include_taxonomies: true,
            include_acf_fields: true,
            field_mappings: HashMap::new(),
            custom_field_processors: HashMap::new(),
            shortcode_processing: true,
            media_processing_config: MediaProcessingConfig::default(),
        }
    }
}

impl Default for MediaProcessingConfig {
    fn default() -> Self {
        Self {
            download_media: false,
            media_base_path: "./media".to_string(),
            resize_images: false,
            max_image_width: Some(1920),
            max_image_height: Some(1080),
            generate_thumbnails: true,
            thumbnail_sizes: vec![
                ThumbnailSize { name: "thumbnail".to_string(), width: 150, height: 150, crop: true },
                ThumbnailSize { name: "medium".to_string(), width: 300, height: 300, crop: false },
                ThumbnailSize { name: "large".to_string(), width: 1024, height: 1024, crop: false },
            ],
        }
    }
}
```

## Example WordPress Configuration

```toml
# WordPress WXR Export Configuration
[source]
adapter_type = "wordpress"
connection_method = { type = "file", paths = ["./wp-export.xml"], format = "wxr" }

[source.adapter_config]
format = "wxr"
content_types = ["post", "page", "product", "event"]
include_drafts = false
include_private = false
include_media = true
include_comments = false
include_users = true
include_taxonomies = true
include_acf_fields = true
shortcode_processing = true

[source.adapter_config.field_mappings]
"_yoast_wpseo_title" = "seo_title"
"_yoast_wpseo_metadesc" = "seo_description"
"_featured_image" = "featured_media"
"custom_field_name" = "mapped_field_name"

[source.adapter_config.media_processing_config]
download_media = false
media_base_path = "./media"
resize_images = true
max_image_width = 1920
max_image_height = 1080
generate_thumbnails = true

[[source.adapter_config.media_processing_config.thumbnail_sizes]]
name = "thumbnail"
width = 150
height = 150
crop = true

[[source.adapter_config.media_processing_config.thumbnail_sizes]]
name = "medium"
width = 300
height = 300
crop = false
```

```toml
# WordPress REST API Configuration
[source]
adapter_type = "wordpress"
connection_method = { type = "api", endpoint = "https://example.com/wp-json/wp/v2" }

[source.adapter_config]
format = "api"
content_types = ["posts", "pages", "media"]
include_acf_fields = true
include_users = true
include_taxonomies = true

# Optional auth
[source.connection_method.auth_config]
# one of: "bearer", "basic", "apikey"
type = "bearer"
token = "<YOUR_TOKEN>"
```

This comprehensive WordPress source implementation provides robust support for all major WordPress export formats and data structures, with extensible field processing capabilities and flexible configuration options.