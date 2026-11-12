use crate::config::ProjectConfig;
use crate::sources::umbraco::UmbracoSource;
use crate::sources::wordpress::WordPressSource;
use crate::stage::{Destination, Source};

// Adapters register here as they land (later tasks).
pub fn source_names() -> &'static [&'static str] {
    &["wordpress", "umbraco"]
}

pub fn destination_names() -> &'static [&'static str] {
    &[]
}

pub fn source(name: &str) -> Option<Box<dyn Source>> {
    match name {
        "wordpress" => Some(Box::new(WordPressSource)),
        "umbraco" => Some(Box::new(UmbracoSource)),
        _ => None,
    }
}

pub fn destination(_name: &str, _cfg: &ProjectConfig) -> Option<Box<dyn Destination>> {
    None
}
