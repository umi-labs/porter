use thiserror::Error;
use std::path::PathBuf;

#[derive(Debug, Error)]
pub enum SchemaParseError {
    #[error("Failed to read file {path}: {source}")]
    IoError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    
    #[error("Parse error in {file}: {message}")]
    ParseError { 
        file: String, 
        message: String 
    },
    
    #[error("Circular dependency detected: {0} -> {1}")]
    CircularDependency(String, String),
    
    #[error("Unresolved import '{import}' in {file}")]
    UnresolvedImport {
        import: String,
        file: String,
    },
    
    #[error("Invalid spread operation: {0}")]
    InvalidSpread(String),
    
    #[error("Export '{name}' not found in {file}")]
    ExportNotFound {
        name: String,
        file: String,
    },

    #[error("Config file not found or invalid")]
    ConfigError,
}

pub type Result<T> = std::result::Result<T, SchemaParseError>;
