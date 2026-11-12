pub mod crawl;
pub mod fetch;

use serde::{Deserialize, Serialize};

/// Connectors write a RawItem row for every item they give a status; normalise later
/// overwrites the row at the same ItemId with the neutral model type.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RawItem {
    pub label: String,
    pub title: Option<String>,
    pub status: Option<String>,
    pub url: Option<String>,
    pub raw_path: Option<String>,
}
