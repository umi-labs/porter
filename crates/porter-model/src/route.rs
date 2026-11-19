use crate::ItemId;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum RouteTarget {
    Document(ItemId),
    Media(ItemId),
    Redirect(ItemId),
    None,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Route {
    pub url: String,
    pub target: RouteTarget,
    pub http_status: Option<u16>,
    pub title: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RedirectOrigin {
    Plugin,
    Crawl,
    Field,
    Manual,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Redirect {
    pub from: String,
    pub to: String,
    pub code: u16,
    pub origin: RedirectOrigin,
}
