use crate::config::ProjectConfig;
use crate::stage::{Destination, Source};

// Adapters register here as they land (later tasks).
pub fn source(_name: &str) -> Option<Box<dyn Source>> {
    None
}

pub fn destination(_name: &str, _cfg: &ProjectConfig) -> Option<Box<dyn Destination>> {
    None
}
