pub mod adapter;
pub mod batch;
pub mod cli;
pub mod config;
pub mod mapping;
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
pub use adapter::{SourceReader, TargetWriter, TargetOptions};
pub use sources::umbraco::UmbracoSource;
pub use targets::payload::PayloadTarget;
pub use plugin::{PluginManager, PluginRegistrar};
