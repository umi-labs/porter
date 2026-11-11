use crate::sources::wordpress::config::*;
use crate::adapters::{AuthConfig, AuthType};
use anyhow::{Context, Result};
use log::{debug, info, warn};
use reqwest::Client;
use serde_json::{Value, json};

/// WordPress REST API connector
pub struct WordPressApiConnector {
    client: Client,
    pub base_url: String,
    config: WordPressConfig,
    auth: Option<AuthConfig>,
}

impl WordPressApiConnector {
    pub fn new(base_url: String, config: WordPressConfig, auth: Option<AuthConfig>) -> Result<Self> {
        let client = Client::builder()
            .user_agent("Porter/1.0.2 WordPress Connector")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            config,
            auth,
        })
    }

    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            Some(AuthConfig { auth_type: AuthType::Bearer, credentials }) => {
                if let Some(token) = credentials.get("token") {
                    return builder.bearer_auth(token);
                }
                builder
            }
            Some(AuthConfig { auth_type: AuthType::Basic, credentials }) => {
                let user = credentials.get("username").cloned().unwrap_or_default();
                let pass = credentials.get("password").cloned();
                builder.basic_auth(user, pass)
            }
            Some(AuthConfig { auth_type: AuthType::ApiKey, credentials }) => {
                let header = credentials.get("header").cloned().unwrap_or_else(|| "X-API-Key".to_string());
                let key = credentials.get("key").cloned().unwrap_or_default();
                builder.header(header, key)
            }
            Some(AuthConfig { auth_type: AuthType::OAuth2, credentials }) => {
                // Treat as bearer token if provided
                if let Some(token) = credentials.get("access_token") {
                    return builder.bearer_auth(token);
                }
                builder
            }
            None => builder,
        }
    }

    async fn get_with_retries(&self, url: &str) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        let max_retries = 3u32;
        let mut backoff_ms = 500u64;
        loop {
            let req = self.client.get(url);
            let req = self.apply_auth(req);
            let resp = req.send().await.context(format!("Failed to GET {}", url));
            match resp {
                Ok(r) => {
                    if r.status().as_u16() == 429 || r.status().is_server_error() {
                        if attempt < max_retries {
                            attempt += 1;
                            info!("HTTP {} on {}; retrying {}/{} after {}ms", r.status(), url, attempt, max_retries, backoff_ms);
                            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                            backoff_ms *= 2;
                            continue;
                        }
                    }
                    return Ok(r);
                }
                Err(e) => {
                    if attempt < max_retries {
                        attempt += 1;
                        info!("Request error on {}: {} - retrying {}/{} after {}ms", url, e, attempt, max_retries, backoff_ms);
                        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        backoff_ms *= 2;
                        continue;
                    }
                    return Err(anyhow::anyhow!(e));
                }
            }
        }
    }

    /// Fetch all posts from the WordPress API
    pub async fn fetch_posts(&self) -> Result<Vec<Value>> {
        let mut all_posts = Vec::new();
        let mut page = 1;
        let per_page = 100; // WordPress default max

        let mut total_pages: Option<u32> = None;
        loop {
            info!("Fetching posts page {} (per_page: {})", page, per_page);

            let url = format!(
                "{}/posts?page={}&per_page={}&status=publish&_embed",
                self.base_url, page, per_page
            );

            let response = self
                .get_with_retries(&url)
                .await
                .context(format!("Failed to fetch posts from {}", url))?;

            if !response.status().is_success() {
                return Err(anyhow::anyhow!(
                    "API request failed with status: {} for URL: {}",
                    response.status(),
                    url
                ));
            }

            if total_pages.is_none() {
                if let Some(tp) = response.headers().get("X-WP-TotalPages") {
                    if let Ok(s) = tp.to_str() { total_pages = s.parse::<u32>().ok(); }
                }
            }

            let posts: Vec<Value> = response
                .json()
                .await
                .context("Failed to parse posts JSON response")?;

            if posts.is_empty() {
                info!("No more posts found at page {}", page);
                break;
            }

            info!("Retrieved {} posts from page {}", posts.len(), page);

            // Process each post
            for post in posts {
                let processed_post = self.process_api_post(post)?;
                all_posts.push(processed_post);
            }

            page += 1;
            if let Some(tp) = total_pages { if (page as u32) > tp { break; } }
        }

        info!("Total posts retrieved: {}", all_posts.len());
        Ok(all_posts)
    }

    /// Fetch all pages from the WordPress API
    pub async fn fetch_pages(&self) -> Result<Vec<Value>> {
        let mut all_pages = Vec::new();
        let mut page = 1;
        let per_page = 100;

        let mut total_pages: Option<u32> = None;
        loop {
            info!("Fetching pages page {} (per_page: {})", page, per_page);

            let url = format!(
                "{}/pages?page={}&per_page={}&status=publish&_embed",
                self.base_url, page, per_page
            );

            let response = self
                .get_with_retries(&url)
                .await
                .context(format!("Failed to fetch pages from {}", url))?;

            if !response.status().is_success() {
                return Err(anyhow::anyhow!(
                    "API request failed with status: {} for URL: {}",
                    response.status(),
                    url
                ));
            }

            if total_pages.is_none() {
                if let Some(tp) = response.headers().get("X-WP-TotalPages") { if let Ok(s) = tp.to_str() { total_pages = s.parse::<u32>().ok(); } }
            }

            let pages: Vec<Value> = response
                .json()
                .await
                .context("Failed to parse pages JSON response")?;

            if pages.is_empty() {
                info!("No more pages found at page {}", page);
                break;
            }

            info!("Retrieved {} pages from page {}", pages.len(), page);

            // Process each page
            for page_item in pages {
                let processed_page = self.process_api_post(page_item)?;
                all_pages.push(processed_page);
            }

            page += 1;
            if let Some(tp) = total_pages { if (page as u32) > tp { break; } }
        }

        info!("Total pages retrieved: {}", all_pages.len());
        Ok(all_pages)
    }

    /// Fetch media items from the WordPress API
    pub async fn fetch_media(&self) -> Result<Vec<Value>> {
        let mut all_media = Vec::new();
        let mut page = 1;
        let per_page = 100;

        let mut total_pages: Option<u32> = None;
        loop {
            info!("Fetching media page {} (per_page: {})", page, per_page);

            let url = format!(
                "{}/media?page={}&per_page={}",
                self.base_url, page, per_page
            );

            let response = self
                .get_with_retries(&url)
                .await
                .context(format!("Failed to fetch media from {}", url))?;

            if !response.status().is_success() {
                return Err(anyhow::anyhow!(
                    "API request failed with status: {} for URL: {}",
                    response.status(),
                    url
                ));
            }

            if total_pages.is_none() {
                if let Some(tp) = response.headers().get("X-WP-TotalPages") { if let Ok(s) = tp.to_str() { total_pages = s.parse::<u32>().ok(); } }
            }

            let media_items: Vec<Value> = response
                .json()
                .await
                .context("Failed to parse media JSON response")?;

            if media_items.is_empty() {
                info!("No more media found at page {}", page);
                break;
            }

            info!(
                "Retrieved {} media items from page {}",
                media_items.len(),
                page
            );

            // Process each media item
            for media_item in media_items {
                let processed_media = self.process_api_media(media_item)?;
                all_media.push(processed_media);
            }

            page += 1;
            if let Some(tp) = total_pages { if (page as u32) > tp { break; } }
        }

        info!("Total media items retrieved: {}", all_media.len());
        Ok(all_media)
    }

    /// Fetch users from the WordPress API
    pub async fn fetch_users(&self) -> Result<Vec<Value>> {
        let mut all_users = Vec::new();
        let mut page = 1;
        let per_page = 100;

        let mut total_pages: Option<u32> = None;
        loop {
            info!("Fetching users page {} (per_page: {})", page, per_page);
            let url = format!(
                "{}/users?page={}&per_page={}",
                self.base_url, page, per_page
            );

            let response = self
                .get_with_retries(&url)
                .await
                .context(format!("Failed to fetch users from {}", url))?;

            if !response.status().is_success() {
                return Err(anyhow::anyhow!(
                    "API request failed with status: {} for URL: {}",
                    response.status(),
                    url
                ));
            }

            if total_pages.is_none() {
                if let Some(tp) = response.headers().get("X-WP-TotalPages") {
                    if let Ok(s) = tp.to_str() { total_pages = s.parse::<u32>().ok(); }
                }
            }

            let users: Vec<Value> = response
                .json()
                .await
                .context("Failed to parse users JSON response")?;

            if users.is_empty() { break; }

            for user in users {
                let processed = self.process_api_user(user)?;
                all_users.push(processed);
            }

            page += 1;
            if let Some(tp) = total_pages { if (page as u32) > tp { break; } }
        }

        info!("Total users retrieved: {}", all_users.len());
        Ok(all_users)
    }

    /// Fetch taxonomy terms (categories or tags)
    async fn fetch_taxonomy(&self, taxonomy: &str) -> Result<Vec<Value>> {
        let mut all_terms = Vec::new();
        let mut page = 1;
        let per_page = 100;

        let mut total_pages: Option<u32> = None;
        loop {
            info!("Fetching {} page {} (per_page: {})", taxonomy, page, per_page);
            let url = format!(
                "{}/{}?page={}&per_page={}",
                self.base_url, taxonomy, page, per_page
            );

            let response = self
                .get_with_retries(&url)
                .await
                .context(format!("Failed to fetch {} from {}", taxonomy, url))?;

            if !response.status().is_success() {
                return Err(anyhow::anyhow!(
                    "API request failed with status: {} for URL: {}",
                    response.status(),
                    url
                ));
            }

            if total_pages.is_none() {
                if let Some(tp) = response.headers().get("X-WP-TotalPages") {
                    if let Ok(s) = tp.to_str() { total_pages = s.parse::<u32>().ok(); }
                }
            }

            let terms: Vec<Value> = response
                .json()
                .await
                .context("Failed to parse taxonomy JSON response")?;

            if terms.is_empty() { break; }

            for term in terms {
                let processed = self.process_api_term(term, taxonomy)?;
                all_terms.push(processed);
            }

            page += 1;
            if let Some(tp) = total_pages { if (page as u32) > tp { break; } }
        }

        info!("Total {} retrieved: {}", taxonomy, all_terms.len());
        Ok(all_terms)
    }

    pub async fn fetch_categories(&self) -> Result<Vec<Value>> {
        self.fetch_taxonomy("categories").await
    }

    pub async fn fetch_tags(&self) -> Result<Vec<Value>> {
        self.fetch_taxonomy("tags").await
    }

    /// Fetch all content types requested in the configuration
    pub async fn fetch_all_content(&self) -> Result<Vec<Value>> {
        let mut all_content = Vec::new();

        for content_type in &self.config.content_types {
            match content_type.as_str() {
                "post" => {
                    info!("Fetching WordPress posts...");
                    let posts = self.fetch_posts().await?;
                    all_content.extend(posts);
                }
                "page" => {
                    info!("Fetching WordPress pages...");
                    let pages = self.fetch_pages().await?;
                    all_content.extend(pages);
                }
                "attachment" | "media" => {
                    if self.config.include_media {
                        info!("Fetching WordPress media...");
                        let media = self.fetch_media().await?;
                        all_content.extend(media);
                    }
                }
                custom_type => {
                    warn!(
                        "Custom post type '{}' not yet supported via API",
                        custom_type
                    );
                }
            }
        }

        info!("Total content items retrieved: {}", all_content.len());
        Ok(all_content)
    }

    /// Test the API connection
    pub async fn test_connection(&self) -> Result<()> {
        info!("Testing WordPress API connection to {}", self.base_url);

        let url = format!("{}/posts?per_page=1", self.base_url);

        let response = self
            .apply_auth(self.client.get(&url))
            .send()
            .await
            .context("Failed to connect to WordPress API")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "WordPress API returned error status: {}",
                response.status()
            ));
        }

        // Try to parse the response
        let _test_response: Value = response
            .json()
            .await
            .context("Failed to parse WordPress API response")?;

        info!("WordPress API connection test successful");
        Ok(())
    }

    /// Process a WordPress API post/page response into Porter format
    fn process_api_post(&self, post: Value) -> Result<Value> {
        let mut processed = json!({
            "type": "post",
        });

        if let Some(obj) = post.as_object() {
            // Basic fields
            if let Some(id) = obj.get("id") {
                processed["id"] = id.clone();
            }

            if let Some(title) = obj.get("title").and_then(|t| t.get("rendered")) {
                processed["title"] = title.clone();
            }

            if let Some(content) = obj.get("content").and_then(|c| c.get("rendered")) {
                processed["content"] = content.clone();
            }

            if let Some(excerpt) = obj.get("excerpt").and_then(|e| e.get("rendered")) {
                processed["excerpt"] = excerpt.clone();
            }

            if let Some(slug) = obj.get("slug") {
                processed["slug"] = slug.clone();
            }

            if let Some(status) = obj.get("status") {
                processed["status"] = status.clone();
            }

            if let Some(post_type) = obj.get("type") {
                processed["post_type"] = post_type.clone();
            }

            if let Some(date) = obj.get("date") {
                processed["created_at"] = date.clone();
            }

            if let Some(modified) = obj.get("modified") {
                processed["updated_at"] = modified.clone();
            }

            // Featured media
            if let Some(featured_media) = obj.get("featured_media") {
                if !featured_media.is_null() && featured_media.as_u64() != Some(0) {
                    processed["featured_media"] = featured_media.clone();
                }
            }

            // Categories and tags
            if let Some(categories) = obj.get("categories") {
                processed["categories"] = categories.clone();
            }

            if let Some(tags) = obj.get("tags") {
                processed["tags"] = tags.clone();
            }

            // Author
            if let Some(author) = obj.get("author") {
                processed["author"] = author.clone();
            }

            // ACF fields (if present)
            if let Some(acf) = obj.get("acf") {
                if !acf.is_null() {
                    processed["acf"] = acf.clone();
                }
            }

            // Meta fields (if present)
            if let Some(meta) = obj.get("meta") {
                if !meta.is_null() {
                    processed["meta"] = meta.clone();
                }
            }

            // Apply field mappings
            for (wp_field, target_field) in &self.config.field_mappings {
                if let Some(value) = obj.get(wp_field) {
                    processed[target_field] = value.clone();
                }
            }
        }

        debug!("Processed post: {}", processed["title"]);
        Ok(processed)
    }

    /// Process a WordPress API media response into Porter format
    fn process_api_media(&self, media: Value) -> Result<Value> {
        let mut processed = json!({
            "type": "media",
        });

        if let Some(obj) = media.as_object() {
            // Basic fields
            if let Some(id) = obj.get("id") {
                processed["id"] = id.clone();
            }

            if let Some(title) = obj.get("title").and_then(|t| t.get("rendered")) {
                processed["title"] = title.clone();
            }

            if let Some(alt_text) = obj.get("alt_text") {
                processed["alt"] = alt_text.clone();
            }

            if let Some(caption) = obj.get("caption").and_then(|c| c.get("rendered")) {
                processed["caption"] = caption.clone();
            }

            if let Some(description) = obj.get("description").and_then(|d| d.get("rendered")) {
                processed["description"] = description.clone();
            }

            if let Some(source_url) = obj.get("source_url") {
                processed["url"] = source_url.clone();
            }

            if let Some(mime_type) = obj.get("mime_type") {
                processed["mime_type"] = mime_type.clone();
            }

            if let Some(media_details) = obj.get("media_details") {
                if let Some(file_size) = media_details.get("filesize") {
                    processed["file_size"] = file_size.clone();
                }

                if let Some(width) = media_details.get("width") {
                    processed["width"] = width.clone();
                }

                if let Some(height) = media_details.get("height") {
                    processed["height"] = height.clone();
                }

                // Sizes/thumbnails
                if let Some(sizes) = media_details.get("sizes") {
                    processed["sizes"] = sizes.clone();
                }
            }

            if let Some(date) = obj.get("date") {
                processed["created_at"] = date.clone();
            }
        }

        debug!("Processed media: {}", processed["title"]);
        Ok(processed)
    }

    /// Process a WordPress API user into Porter format
    fn process_api_user(&self, user: Value) -> Result<Value> {
        let mut processed = json!({
            "type": "user",
        });

        if let Some(obj) = user.as_object() {
            if let Some(id) = obj.get("id") { processed["id"] = id.clone(); }
            if let Some(name) = obj.get("name") { processed["name"] = name.clone(); }
            if let Some(slug) = obj.get("slug") { processed["slug"] = slug.clone(); }
            if let Some(desc) = obj.get("description") { processed["description"] = desc.clone(); }
            if let Some(link) = obj.get("link") { processed["link"] = link.clone(); }
        }

        Ok(processed)
    }

    /// Process a taxonomy term (category/tag) into Porter format
    fn process_api_term(&self, term: Value, taxonomy: &str) -> Result<Value> {
        let mut processed = json!({
            "type": taxonomy,
        });

        if let Some(obj) = term.as_object() {
            if let Some(id) = obj.get("id") { processed["id"] = id.clone(); }
            if let Some(name) = obj.get("name") { processed["name"] = name.clone(); }
            if let Some(slug) = obj.get("slug") { processed["slug"] = slug.clone(); }
            if let Some(desc) = obj.get("description") { processed["description"] = desc.clone(); }
            if let Some(count) = obj.get("count") { processed["count"] = count.clone(); }
        }

        Ok(processed)
    }

    /// Fetch from a custom endpoint
    pub async fn fetch_custom_endpoint(&self, endpoint: &str) -> Result<Vec<Value>> {
        info!("Fetching from custom endpoint: {}", endpoint);

        let url = format!("{}/{}", self.base_url, endpoint.trim_start_matches('/'));

        let response = self
            .get_with_retries(&url)
            .await
            .context(format!("Failed to fetch from custom endpoint {}", url))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "API request failed with status: {} for URL: {}",
                response.status(),
                url
            ));
        }

        let data: Value = response
            .json()
            .await
            .context("Failed to parse JSON response from custom endpoint")?;

        // Convert to Vec<Value> if it's a single object
        let documents = if let Some(array) = data.as_array() {
            array.clone()
        } else {
            vec![data]
        };

        info!(
            "Retrieved {} documents from custom endpoint",
            documents.len()
        );
        Ok(documents)
    }

    /// Get information about a specific endpoint
    pub async fn get_endpoint_info(&self, endpoint: &str) -> Result<Value> {
        info!("Getting info for endpoint: {}", endpoint);

        let url = format!("{}/{}", self.base_url, endpoint.trim_start_matches('/'));

        let response = self
            .apply_auth(self.client.head(&url))
            .send()
            .await
            .context(format!("Failed to get endpoint info for {}", url))?;

        let mut info = json!({
            "endpoint": endpoint,
            "url": url,
            "status": response.status().as_u16(),
            "headers": {}
        });

        // Extract useful headers
        let headers = response.headers();
        for (name, value) in headers.iter() {
            if let Ok(value_str) = value.to_str() {
                info["headers"][name.as_str()] = json!(value_str);
            }
        }

        Ok(info)
    }

    /// Discover available WordPress API routes
    pub async fn discover_routes(&self) -> Result<Value> {
        info!("Discovering WordPress API routes from {}", self.base_url);

        let url = format!("{}", self.base_url);

        let response = self
            .get_with_retries(&url)
            .await
            .context(format!("Failed to discover routes from {}", url))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to discover routes with status: {}",
                response.status()
            ));
        }

        let routes: Value = response
            .json()
            .await
            .context("Failed to parse routes JSON response")?;

        info!("Successfully discovered WordPress API routes");
        Ok(routes)
    }

    /// Get available content types from WordPress API
    pub async fn get_available_content_types(&self) -> Result<Vec<String>> {
        info!("Getting available content types from WordPress API");

        let routes = self.discover_routes().await?;
        let mut content_types = Vec::new();

        if let Some(routes_obj) = routes.as_object() {
            for (route, _) in routes_obj {
                // Extract content type from route (e.g., "/wp/v2/posts" -> "posts")
                if route.starts_with("/wp/v2/") && route.len() > 7 {
                    let content_type = &route[7..]; // Remove "/wp/v2/" prefix
                    if !content_type.contains('/') && !content_type.contains('(') {
                        content_types.push(content_type.to_string());
                    }
                }
            }
        }

        // Sort and deduplicate
        content_types.sort();
        content_types.dedup();

        info!("Found {} available content types: {:?}", content_types.len(), content_types);
        Ok(content_types)
    }

    /// Test a specific endpoint and return sample data
    pub async fn test_endpoint(&self, endpoint: &str) -> Result<Value> {
        info!("Testing endpoint: {}", endpoint);

        let url = format!("{}/{}?per_page=1", self.base_url, endpoint.trim_start_matches('/'));

        let response = self
            .get_with_retries(&url)
            .await
            .context(format!("Failed to test endpoint {}", url))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Endpoint test failed with status: {} for URL: {}",
                response.status(),
                url
            ));
        }

        let data: Value = response
            .json()
            .await
            .context("Failed to parse endpoint test response")?;

        info!("Successfully tested endpoint: {}", endpoint);
        Ok(data)
    }
}
