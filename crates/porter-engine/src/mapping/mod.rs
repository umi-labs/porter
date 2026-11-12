use serde::{Deserialize, Serialize};

/// Mapping decisions from `porter.toml`. Filled in by Task 15.
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MappingConfig {}
