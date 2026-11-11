use crate::ItemId;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Usage {
    pub item: ItemId,
    pub field: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Media {
    pub id: ItemId,
    pub hash: String,
    pub variants: Vec<String>,
    pub filename: String,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt: Option<String>,
    pub caption: Option<String>,
    pub title: Option<String>,
    pub credit: Option<String>,
    pub folder: Option<String>,
    pub usages: Vec<Usage>,
}
