use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Analyse,
    Extract,
    Normalise,
    Schema,
    Load,
    Verify,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ItemStatus {
    Done,
    Partial { reason: String },
    Skipped { reason: String },
    Failed { error: String },
}
