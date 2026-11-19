use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchemaParseError {
    #[error("Failed to read file {path}: {source}")]
    IoError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Parse error in {file}: {message}")]
    ParseError { file: String, message: String },

    #[error("Unresolved import '{import}' in {file}")]
    UnresolvedImport { import: String, file: String },
}

pub type Result<T> = std::result::Result<T, SchemaParseError>;
