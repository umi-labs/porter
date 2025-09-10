use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for WordPress source adapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPressConfig {
    /// WordPress export format (wxr, json)
    pub format: WordPressFormat,

    /// Content types to include in the migration
    pub content_types: Vec<String>,

    /// Whether to include draft posts
    pub include_drafts: bool,

    /// Whether to include private posts
    pub include_private: bool,

    /// Whether to include media attachments
    pub include_media: bool,

    /// Whether to include comments
    pub include_comments: bool,

    /// Whether to include user information
    pub include_users: bool,

    /// Whether to include taxonomies (categories, tags)
    pub include_taxonomies: bool,

    /// Whether to include Advanced Custom Fields (ACF) data
    pub include_acf_fields: bool,

    /// Field name mappings from WordPress to target format
    pub field_mappings: HashMap<String, String>,

    /// Custom field processors configuration
    pub custom_field_processors: HashMap<String, String>,

    /// Whether to process WordPress shortcodes
    pub shortcode_processing: bool,

    /// Media processing configuration
    pub media_processing_config: MediaProcessingConfig,
}

/// WordPress export format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WordPressFormat {
    /// WordPress Extended RSS (XML) format
    WXR,
    /// JSON dump format
    JSON,
    /// WordPress REST API
    Api,
    /// Direct database connection (future)
    Database,
}

/// Configuration for media processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaProcessingConfig {
    /// Whether to download media files locally
    pub download_media: bool,

    /// Base path for downloaded media
    pub media_base_path: String,

    /// Whether to resize images
    pub resize_images: bool,

    /// Maximum image width (pixels)
    pub max_image_width: Option<u32>,

    /// Maximum image height (pixels)
    pub max_image_height: Option<u32>,

    /// Whether to generate thumbnails
    pub generate_thumbnails: bool,

    /// Thumbnail sizes to generate
    pub thumbnail_sizes: Vec<ThumbnailSize>,
}

/// Thumbnail size configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThumbnailSize {
    /// Name of the thumbnail size
    pub name: String,

    /// Width in pixels
    pub width: u32,

    /// Height in pixels
    pub height: u32,

    /// Whether to crop the image
    pub crop: bool,
}

impl Default for WordPressConfig {
    fn default() -> Self {
        Self {
            format: WordPressFormat::WXR,
            content_types: vec![
                "post".to_string(),
                "page".to_string(),
                "attachment".to_string(),
            ],
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
                ThumbnailSize {
                    name: "thumbnail".to_string(),
                    width: 150,
                    height: 150,
                    crop: true,
                },
                ThumbnailSize {
                    name: "medium".to_string(),
                    width: 300,
                    height: 300,
                    crop: false,
                },
                ThumbnailSize {
                    name: "large".to_string(),
                    width: 1024,
                    height: 1024,
                    crop: false,
                },
            ],
        }
    }
}

impl WordPressConfig {
    /// Create a minimal configuration for basic WordPress exports
    pub fn minimal() -> Self {
        Self {
            content_types: vec!["post".to_string(), "page".to_string()],
            include_media: false,
            include_comments: false,
            include_users: false,
            include_taxonomies: false,
            include_acf_fields: false,
            shortcode_processing: false,
            ..Default::default()
        }
    }

    /// Create a comprehensive configuration for full WordPress exports
    pub fn comprehensive() -> Self {
        Self {
            content_types: vec![
                "post".to_string(),
                "page".to_string(),
                "attachment".to_string(),
                "nav_menu_item".to_string(),
                "custom_css".to_string(),
                "customize_changeset".to_string(),
            ],
            include_drafts: true,
            include_private: true,
            include_media: true,
            include_comments: true,
            include_users: true,
            include_taxonomies: true,
            include_acf_fields: true,
            shortcode_processing: true,
            media_processing_config: MediaProcessingConfig {
                download_media: true,
                resize_images: true,
                generate_thumbnails: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Add a field mapping
    pub fn with_field_mapping(mut self, wp_field: &str, target_field: &str) -> Self {
        self.field_mappings
            .insert(wp_field.to_string(), target_field.to_string());
        self
    }

    /// Add multiple field mappings
    pub fn with_field_mappings(mut self, mappings: HashMap<String, String>) -> Self {
        self.field_mappings.extend(mappings);
        self
    }

    /// Set content types to include
    pub fn with_content_types(mut self, types: Vec<String>) -> Self {
        self.content_types = types;
        self
    }

    /// Enable or disable ACF field processing
    pub fn with_acf_fields(mut self, enabled: bool) -> Self {
        self.include_acf_fields = enabled;
        self
    }

    /// Enable or disable shortcode processing
    pub fn with_shortcode_processing(mut self, enabled: bool) -> Self {
        self.shortcode_processing = enabled;
        self
    }
}

/// WordPress content status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl From<&str> for PostStatus {
    fn from(status: &str) -> Self {
        match status {
            "publish" => PostStatus::Publish,
            "draft" => PostStatus::Draft,
            "private" => PostStatus::Private,
            "pending" => PostStatus::Pending,
            "trash" => PostStatus::Trash,
            "auto-draft" => PostStatus::AutoDraft,
            "inherit" => PostStatus::Inherit,
            custom => PostStatus::Custom(custom.to_string()),
        }
    }
}

impl ToString for PostStatus {
    fn to_string(&self) -> String {
        match self {
            PostStatus::Publish => "publish".to_string(),
            PostStatus::Draft => "draft".to_string(),
            PostStatus::Private => "private".to_string(),
            PostStatus::Pending => "pending".to_string(),
            PostStatus::Trash => "trash".to_string(),
            PostStatus::AutoDraft => "auto-draft".to_string(),
            PostStatus::Inherit => "inherit".to_string(),
            PostStatus::Custom(status) => status.clone(),
        }
    }
}
