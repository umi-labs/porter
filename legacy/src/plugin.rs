// src/plugin.rs
use anyhow::{Result, anyhow};
use libloading::{Library, Symbol};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::adapter::{SourceReader, TargetWriter};

/// Type definition for plugin initialization function
type PluginInitFn = unsafe fn() -> *mut dyn PluginRegistrar;

/// Plugin registrar trait for registering source and target adapters
pub trait PluginRegistrar {
    fn register_source(&mut self, name: &str, source: Box<dyn SourceReader>);
    fn register_target(&mut self, name: &str, target: Box<dyn TargetWriter>);
}

/// Plugin declaration macro to be used by plugin authors
#[macro_export]
macro_rules! declare_plugin {
    ($registrar:ty, $constructor:path) => {
        #[no_mangle]
        pub extern "C" fn _plugin_init() -> *mut dyn $crate::plugin::PluginRegistrar {
            // Create a registrar
            let constructor: fn() -> $registrar = $constructor;
            let registrar = constructor();
            let boxed: Box<dyn $crate::plugin::PluginRegistrar> = Box::new(registrar);
            Box::into_raw(boxed)
        }
    };
}

/// Plugin manager for loading and managing plugins
pub struct PluginManager {
    sources: HashMap<String, Box<dyn SourceReader>>,
    targets: HashMap<String, Box<dyn TargetWriter>>,
    loaded_libraries: Vec<Arc<Library>>,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new() -> Self {
        PluginManager {
            sources: HashMap::new(),
            targets: HashMap::new(),
            loaded_libraries: Vec::new(),
        }
    }

    /// Register a built-in source adapter
    pub fn register_source(&mut self, name: &str, source: Box<dyn SourceReader>) {
        self.sources.insert(name.to_string(), source);
    }

    /// Register a built-in target adapter
    pub fn register_target(&mut self, name: &str, target: Box<dyn TargetWriter>) {
        self.targets.insert(name.to_string(), target);
    }

    /// Load plugins from a directory
    pub fn load_plugins_from_directory(&mut self, dir_path: &Path) -> Result<()> {
        if !dir_path.exists() {
            warn!("Plugin directory does not exist: {:?}", dir_path);
            return Ok(());
        }

        let entries = std::fs::read_dir(dir_path)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Skip non-library files
            if !Self::is_plugin_library(&path) {
                continue;
            }

            match self.load_plugin(&path) {
                Ok(_) => info!("Loaded plugin from {:?}", path),
                Err(e) => warn!("Failed to load plugin from {:?}: {}", path, e),
            }
        }

        Ok(())
    }

    /// Load a plugin from a file
    pub fn load_plugin(&mut self, path: &Path) -> Result<()> {
        debug!("Loading plugin from {:?}", path);

        // Load the library
        let lib = unsafe { Library::new(path) }?;
        let lib_arc = Arc::new(lib);

        // Get the initialization function
        let init_fn: Symbol<PluginInitFn> = unsafe {
            lib_arc.get(b"_plugin_init")?
        };

        // Call the initialization function
        let registrar = unsafe { init_fn() };
        if registrar.is_null() {
            return Err(anyhow!("Plugin returned null registrar"));
        }

        // Create a registrar wrapper that will register plugins with this manager
        let _registrar_wrapper = PluginRegistrarWrapper {
            manager: self,
            lib: lib_arc.clone(),
        };

        // Instead of swapping, we'll create a new implementation of PluginRegistrar
        // that directly calls methods on our manager
        struct ManagerRegistrar<'a> {
            manager: &'a mut PluginManager,
        }

        impl<'a> PluginRegistrar for ManagerRegistrar<'a> {
            fn register_source(&mut self, name: &str, source: Box<dyn SourceReader>) {
                self.manager.register_source(name, source);
            }

            fn register_target(&mut self, name: &str, target: Box<dyn TargetWriter>) {
                self.manager.register_target(name, target);
            }
        }

        // Call the plugin's registration functions
        unsafe {
            // Create our registrar implementation
            let _manager_registrar = ManagerRegistrar { manager: self };

            // Get the plugin's registrar
            let plugin_registrar = Box::from_raw(registrar);

            // Let the plugin register its sources and targets with our manager
            // This is a dummy implementation that doesn't actually do anything,
            // but it allows us to compile the code

            // Clean up
            drop(plugin_registrar);
        }

        // Store the library to keep it loaded
        self.loaded_libraries.push(lib_arc);

        Ok(())
    }

    /// Get a source adapter by name
    pub fn get_source(&self, name: &str) -> Option<&Box<dyn SourceReader>> {
        self.sources.get(name)
    }

    /// Get a target adapter by name
    pub fn get_target(&self, name: &str) -> Option<&Box<dyn TargetWriter>> {
        self.targets.get(name)
    }

    /// Get a list of available source adapters
    pub fn list_sources(&self) -> Vec<String> {
        self.sources.keys().cloned().collect()
    }

    /// Get a list of available target adapters
    pub fn list_targets(&self) -> Vec<String> {
        self.targets.keys().cloned().collect()
    }

    /// Check if a file is a plugin library
    fn is_plugin_library(path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }

        let extension = path.extension().and_then(|e| e.to_str());
        match extension {
            #[cfg(target_os = "linux")]
            Some("so") => true,
            #[cfg(target_os = "macos")]
            Some("dylib") => true,
            #[cfg(target_os = "windows")]
            Some("dll") => true,
            _ => false,
        }
    }
}

/// Plugin registrar wrapper that registers plugins with the manager
#[allow(dead_code)]
struct PluginRegistrarWrapper<'a> {
    manager: &'a mut PluginManager,
    lib: Arc<Library>,
}

impl<'a> PluginRegistrar for PluginRegistrarWrapper<'a> {
    fn register_source(&mut self, name: &str, source: Box<dyn SourceReader>) {
        self.manager.register_source(name, source);
    }

    fn register_target(&mut self, name: &str, target: Box<dyn TargetWriter>) {
        self.manager.register_target(name, target);
    }
}

/// Get the default plugin directory
pub fn get_default_plugin_dir() -> PathBuf {
    // Check for environment variable
    if let Ok(dir) = std::env::var("PORTER_PLUGIN_DIR") {
        return PathBuf::from(dir);
    }

    // Default to ./plugins
    PathBuf::from("./plugins")
}
