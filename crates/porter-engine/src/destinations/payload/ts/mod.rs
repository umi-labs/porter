//! Reads a Payload site's TypeScript config: collections, globals and blocks, with their
//! fields normalised to Payload's type names. Built on the 1.x swc parser (kept below with
//! its tests); [`read_payload_config`] is the 2.0 entry point.
pub mod ast_analyzer;
mod config;
mod errors;
pub mod import_resolver;
pub mod schema;
pub mod template_generator;

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests;

pub use config::{CollectionSchema, DestField, DestSchema, read_payload_config};
pub use errors::{Result, SchemaParseError};
pub use import_resolver::PathAlias;
