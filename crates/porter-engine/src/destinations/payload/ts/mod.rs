//! Reads a Payload site's TypeScript config: collections, globals and blocks, with their
//! fields normalised to Payload's type names, via the swc parser.
mod config;
mod errors;
pub mod import_resolver;

pub use config::{CollectionSchema, DestField, DestSchema, read_payload_config};
pub use errors::{Result, SchemaParseError};
pub use import_resolver::PathAlias;
