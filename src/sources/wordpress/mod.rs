use crate::adapters::{
    ApiSourceAdapter, ConnectionMethod, FileSourceAdapter, SourceAdapter, SourceCapabilities,
    SourceConfig, SourceMetadata, SourceQuery,
};
use anyhow::{Context, Result};
use futures::future::BoxFuture;
use futures::stream::{BoxStream, Stream};
use log::info;
use serde_json::{Value, json};
use std::pin::Pin;

pub mod api_connector;
pub mod config;
pub mod wxr_parser;

pub use api_connector::WordPressApiConnector;
pub use config::*;
pub use wxr_parser::WXRParser;

/// WordPress source adapter for reading WordPress exports
pub struct WordPressSource {
    /// WordPress-specific configuration
    wp_config: WordPressConfig,
    /// General source configuration
    config: Option<SourceConfig>,
    /// Whether the adapter is initialized
    initialized: bool,
    /// WXR parser instance
    wxr_parser: Option<WXRParser>,
    /// API connector instance
    api_connector: Option<WordPressApiConnector>,
}

impl Default for WordPressSource {
    fn default() -> Self {
        Self {
            wp_config: WordPressConfig::default(),
            config: None,
            initialized: false,
            wxr_parser: None,
            api_connector: None,
        }
    }
}

impl WordPressSource {
    /// Create a new WordPress source with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new WordPress source with custom configuration
    pub fn with_config(config: WordPressConfig) -> Self {
        Self {
            wp_config: config,
            ..Default::default()
        }
    }

    /// Update WordPress-specific configuration
    pub fn set_wp_config(&mut self, config: WordPressConfig) {
        self.wp_config = config;
    }
}

impl SourceAdapter for WordPressSource {
    fn metadata(&self) -> SourceMetadata {
        SourceMetadata::new(
            "WordPress",
            "1.0.0",
            "Reads and processes WordPress WXR (WordPress Extended RSS) export files with support for posts, pages, media, and custom fields"
        )
        .with_formats(vec!["wxr".to_string(), "xml".to_string(), "json".to_string()])
        .with_author("Porter Team".to_string())
        .with_homepage("https://github.com/umi-labs/porter".to_string())
        .with_example_config(json!({
            "format": "wxr",
            "content_types": ["post", "page", "media"],
            "include_drafts": false,
            "include_media": true,
            "include_comments": false,
            "include_users": true,
            "include_taxonomies": true,
            "field_mappings": {
                "_featured_image": "featured_media",
                "_yoast_wpseo_title": "seo_title"
            }
        }))
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::new()
            .with_streaming(false) // TODO: Implement streaming
            .with_pagination(false)
            .with_content_types(vec![
                "post".to_string(),
                "page".to_string(),
                "attachment".to_string(),
                "custom_post_type".to_string(),
                "user".to_string(),
                "term".to_string(),
                "comment".to_string(),
            ])
            .with_file_formats(vec![
                "wxr".to_string(),
                "xml".to_string(),
                "json".to_string(),
            ])
            .with_batch_size(Some(1000))
    }

    fn init(&mut self, config: &SourceConfig) -> Result<()> {
        // Parse adapter-specific configuration
        if !config.adapter_config.is_null() {
            self.wp_config = serde_json::from_value(config.adapter_config.clone())
                .context("Failed to parse WordPress configuration")?;
        }

        match &config.connection_method {
            ConnectionMethod::File { .. } => {
                // Initialize WXR parser with configuration
                self.wxr_parser = Some(WXRParser::new(self.wp_config.clone()));
            }
            ConnectionMethod::Api { endpoint, .. } => {
                // Initialize API connector with configuration
                self.api_connector = Some(
                    WordPressApiConnector::new(endpoint.clone(), self.wp_config.clone())
                        .context("Failed to create WordPressApiConnector")?,
                );
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "WordPress source only supports File and Api connection methods"
                ));
            }
        }

        self.config = Some(config.clone());
        self.initialized = true;
        Ok(())
    }

    fn validate_config(&self, config: &SourceConfig) -> Result<()> {
        // Validate WordPress-specific configuration if provided
        if !config.adapter_config.is_null() {
            let wp_config: Result<WordPressConfig, _> =
                serde_json::from_value(config.adapter_config.clone());

            match wp_config {
                Ok(_) => {} // Configuration is valid
                Err(e) => {
                    return Err(anyhow::anyhow!("Invalid WordPress configuration: {}", e));
                }
            }
        }

        match &config.connection_method {
            ConnectionMethod::File { paths, format, .. } => {
                if paths.is_empty() {
                    return Err(anyhow::anyhow!("At least one file path must be provided"));
                }

                let supported_formats = vec!["wxr", "xml", "json"];
                if !supported_formats.contains(&format.as_str()) {
                    return Err(anyhow::anyhow!(
                        "Unsupported format '{}'. Supported formats: {:?}",
                        format,
                        supported_formats
                    ));
                }

                // Check if files exist and are readable
                for path in paths {
                    if !std::path::Path::new(path).exists() {
                        return Err(anyhow::anyhow!("File does not exist: {}", path));
                    }
                }

                Ok(())
            }
            ConnectionMethod::Api { endpoint, .. } => {
                if endpoint.is_empty() {
                    return Err(anyhow::anyhow!("API endpoint must be provided"));
                }

                // Basic URL validation
                if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                    return Err(anyhow::anyhow!(
                        "API endpoint must start with http:// or https://"
                    ));
                }

                Ok(())
            }
            _ => Err(anyhow::anyhow!(
                "WordPress source only supports File and Api connection methods"
            )),
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        self.config = None;
        self.initialized = false;
        self.wxr_parser = None;
        self.api_connector = None;
        Ok(())
    }
}

// Temporary: implement old trait for compatibility
use crate::SourceReader;

impl SourceReader for WordPressSource {
    fn read_documents(&self, inputs: &[String]) -> Result<Vec<Value>> {
        if !self.initialized {
            return Err(anyhow::anyhow!("WordPress source not initialized"));
        }

        // Use the FileSourceAdapter implementation
        FileSourceAdapter::read_documents(self, inputs)
    }
}

impl FileSourceAdapter for WordPressSource {
    fn read_documents(&self, file_paths: &[String]) -> Result<Vec<Value>> {
        if !self.initialized {
            return Err(anyhow::anyhow!("WordPress source not initialized"));
        }

        let parser = self
            .wxr_parser
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WXR parser not initialized"))?;

        let mut all_documents = Vec::new();

        for path in file_paths {
            info!("Reading WordPress data from {}", path);

            // Determine format based on file extension or config
            let format = self.detect_file_format(path)?;

            let documents = match format.as_str() {
                "wxr" | "xml" => parser.parse_wxr_file(path)?,
                "json" => parser.parse_json_file(path)?,
                _ => return Err(anyhow::anyhow!("Unsupported file format: {}", format)),
            };

            info!("Extracted {} documents from {}", documents.len(), path);
            all_documents.extend(documents);
        }

        info!(
            "Total WordPress documents processed: {}",
            all_documents.len()
        );
        Ok(all_documents)
    }

    fn stream_documents(
        &self,
        _file_paths: &[String],
    ) -> Result<Box<dyn Iterator<Item = Result<Value>>>> {
        // TODO: Implement streaming for large WordPress exports
        Err(anyhow::anyhow!(
            "Streaming not yet implemented for WordPress source"
        ))
    }

    fn supported_formats(&self) -> Vec<String> {
        vec!["wxr".to_string(), "xml".to_string(), "json".to_string()]
    }

    fn validate_files(&self, file_paths: &[String]) -> Result<()> {
        for path in file_paths {
            let path_obj = std::path::Path::new(path);

            if !path_obj.exists() {
                return Err(anyhow::anyhow!("File does not exist: {}", path));
            }

            if !path_obj.is_file() {
                return Err(anyhow::anyhow!("Path is not a file: {}", path));
            }

            // Check if we can read the file
            match std::fs::File::open(path) {
                Ok(_) => {}
                Err(e) => return Err(anyhow::anyhow!("Cannot read file {}: {}", path, e)),
            }

            // Basic format validation
            let format = self.detect_file_format(path)?;
            match format.as_str() {
                "wxr" | "xml" => {
                    // Check if it looks like XML
                    let content = std::fs::read_to_string(path)
                        .map_err(|e| anyhow::anyhow!("Cannot read file content {}: {}", path, e))?;

                    let trimmed = content.trim();
                    if !trimmed.starts_with("<?xml") && !trimmed.starts_with('<') {
                        return Err(anyhow::anyhow!("File does not appear to be XML: {}", path));
                    }

                    // Check for WordPress WXR markers
                    if format == "wxr" && !content.contains("http://wordpress.org/export/") {
                        return Err(anyhow::anyhow!(
                            "File does not appear to be a WordPress WXR export: {}",
                            path
                        ));
                    }
                }
                "json" => {
                    // Check if it's valid JSON
                    let content = std::fs::read_to_string(path)
                        .map_err(|e| anyhow::anyhow!("Cannot read file content {}: {}", path, e))?;

                    let trimmed = content.trim();
                    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
                        return Err(anyhow::anyhow!("File does not appear to be JSON: {}", path));
                    }
                }
                _ => {
                    return Err(anyhow::anyhow!("Unsupported file format: {}", format));
                }
            }
        }

        Ok(())
    }
}

impl WordPressSource {
    /// Detect file format based on extension and content
    fn detect_file_format(&self, path: &str) -> Result<String> {
        let path_obj = std::path::Path::new(path);

        // First try to determine from extension
        if let Some(extension) = path_obj.extension().and_then(|ext| ext.to_str()) {
            match extension.to_lowercase().as_str() {
                "xml" => {
                    // Check if it's a WXR file by looking for WordPress export markers
                    let content = std::fs::read_to_string(path)?;
                    if content.contains("http://wordpress.org/export/")
                        || content.contains("wp:wxr_version")
                    {
                        return Ok("wxr".to_string());
                    } else {
                        return Ok("xml".to_string());
                    }
                }
                "json" => return Ok("json".to_string()),
                "wxr" => return Ok("wxr".to_string()),
                _ => {}
            }
        }

        // Fall back to content detection
        let content = std::fs::read_to_string(path)?;
        let trimmed = content.trim();

        if trimmed.starts_with("<?xml") || trimmed.starts_with('<') {
            if content.contains("http://wordpress.org/export/") {
                Ok("wxr".to_string())
            } else {
                Ok("xml".to_string())
            }
        } else if trimmed.starts_with('{') || trimmed.starts_with('[') {
            Ok("json".to_string())
        } else {
            Err(anyhow::anyhow!(
                "Cannot determine file format for: {}",
                path
            ))
        }
    }
}

impl ApiSourceAdapter for WordPressSource {
    fn authenticate(&mut self) -> Pin<Box<dyn futures::Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            // WordPress REST API typically doesn't require authentication for read operations
            // For now, we'll just test the connection
            if !self.initialized {
                return Err(anyhow::anyhow!("WordPress source not initialized"));
            }

            let connector = self
                .api_connector
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("API connector not initialized"))?;

            connector.test_connection().await
        })
    }

    fn fetch_documents(
        &self,
        query: &SourceQuery,
    ) -> Pin<Box<dyn futures::Future<Output = Result<Vec<Value>>> + Send + '_>> {
        let endpoint = query.query.clone();
        let include_media = self.wp_config.include_media;

        Box::pin(async move {
            if !self.initialized {
                return Err(anyhow::anyhow!("WordPress source not initialized"));
            }

            let connector = self
                .api_connector
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("API connector not initialized"))?;

            // Use the query parameter to determine what to fetch
            let endpoint = endpoint.as_deref();

            match endpoint {
                Some("posts") => connector.fetch_posts().await,
                Some("pages") => connector.fetch_pages().await,
                Some("media") => connector.fetch_media().await,
                None => {
                    // Fetch all content types and combine them
                    let mut all_documents = Vec::new();

                    // Fetch posts
                    match connector.fetch_posts().await {
                        Ok(mut posts) => all_documents.append(&mut posts),
                        Err(e) => info!("Failed to fetch posts: {}", e),
                    }

                    // Fetch pages
                    match connector.fetch_pages().await {
                        Ok(mut pages) => all_documents.append(&mut pages),
                        Err(e) => info!("Failed to fetch pages: {}", e),
                    }

                    // Fetch media if configured
                    if self.wp_config.include_media {
                        match connector.fetch_media().await {
                            Ok(mut media) => all_documents.append(&mut media),
                            Err(e) => info!("Failed to fetch media: {}", e),
                        }
                    }

                    Ok(all_documents)
                }
                Some(custom_endpoint) => {
                    // For custom endpoints, try to fetch directly
                    connector.fetch_custom_endpoint(custom_endpoint).await
                }
            }
        })
    }

    fn stream_documents(
        &self,
        _query: &SourceQuery,
    ) -> Pin<
        Box<
            dyn futures::Future<Output = Result<Pin<Box<dyn Stream<Item = Result<Value>> + Send>>>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            // TODO: Implement streaming for WordPress API
            Err(anyhow::anyhow!(
                "Streaming not yet implemented for WordPress API"
            ))
        })
    }

    fn test_connection(&self) -> Pin<Box<dyn futures::Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if !self.initialized {
                return Err(anyhow::anyhow!("WordPress source not initialized"));
            }

            let connector = self
                .api_connector
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("API connector not initialized"))?;

            connector.test_connection().await
        })
    }

    fn get_api_info(&self) -> Pin<Box<dyn futures::Future<Output = Result<Value>> + Send + '_>> {
        Box::pin(async move {
            if !self.initialized {
                return Err(anyhow::anyhow!("WordPress source not initialized"));
            }

            let base_url = self
                .api_connector
                .as_ref()
                .map(|c| c.base_url.clone())
                .unwrap_or_default();

            // Return basic API info
            Ok(json!({
                "api_type": "wordpress_rest",
                "base_url": base_url,
                "supported_endpoints": [
                    "posts",
                    "pages",
                    "media",
                    "users",
                    "categories",
                    "tags"
                ]
            }))
        })
    }
}
