use crate::config::ProjectConfig;
use crate::destinations::payload::PayloadDestination;
use crate::sources::umbraco::UmbracoSource;
use crate::sources::wordpress::WordPressSource;
use crate::stage::{Destination, Source};

// Adapters register here as they land (later tasks).
pub fn source_names() -> &'static [&'static str] {
    &["wordpress", "umbraco"]
}

pub fn destination_names() -> &'static [&'static str] {
    &["payload"]
}

pub fn source(name: &str) -> Option<Box<dyn Source>> {
    match name {
        "wordpress" => Some(Box::new(WordPressSource)),
        "umbraco" => Some(Box::new(UmbracoSource)),
        _ => None,
    }
}

pub fn destination(name: &str, cfg: &ProjectConfig) -> Option<Box<dyn Destination>> {
    match name {
        "payload" => Some(Box::new(PayloadDestination::new(&cfg.destination))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    /// Load and verify find canonical items by `source.adapter`: a source whose normalise
    /// wrote them under another connector name would load nothing, silently.
    #[test]
    fn canonical_connector_is_the_adapter_name() {
        for name in super::source_names() {
            let src = super::source(name).unwrap_or_else(|| panic!("{name} not registered"));
            assert_eq!(src.canonical(), *name);
        }
    }
}
