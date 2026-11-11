use crate::FieldValue;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// Stub; Task 3 fills in the real kinds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum BlockKind {
    Section,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Original {
    pub builder: String,
    pub module: String,
    pub classes: Vec<String>,
    pub settings: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub props: IndexMap<String, FieldValue>,
    pub children: Vec<Block>,
    pub original: Option<Original>,
}
