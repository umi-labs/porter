pub mod adapter;
pub mod adapters; // New trait system
pub mod batch;
pub mod cli;
pub mod config;
pub mod mapping;
pub mod parser;
pub mod performance;
pub mod plugin;
pub mod sources;
pub mod targets;
pub mod util;

#[cfg(test)]
mod tests {

    #[test]
    fn test_library_compiles() {
        // Basic test to ensure the library compiles and can be imported
        assert!(true);
    }
}

// Re-export commonly used items
pub use adapter::{SourceReader, TargetOptions, TargetWriter};
pub use plugin::{PluginManager, PluginRegistrar};
pub use sources::umbraco::UmbracoSource;
pub use sources::wordpress::WordPressSource;
pub use targets::payload::PayloadTarget;

// Debug macros are automatically exported at crate root due to #[macro_export]
