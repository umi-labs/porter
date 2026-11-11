use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RichText {
    pub blocks: Vec<RtBlock>,
}

// Stub; Task 3 fills in the real node set.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RtBlock {
    Rule,
}
