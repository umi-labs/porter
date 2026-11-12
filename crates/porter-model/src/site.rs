use crate::{ItemId, RichText, Seo};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum MenuTarget {
    Doc(ItemId),
    Url(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub target: MenuTarget,
    pub children: Vec<MenuItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Menu {
    pub name: String,
    pub items: Vec<MenuItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Taxonomy {
    pub name: String,
    pub hierarchical: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Term {
    pub taxonomy: String,
    pub name: String,
    pub slug: String,
    pub parent: Option<ItemId>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub kind: String,
    pub required: bool,
    pub options: Vec<(String, String)>,
    pub conditions: serde_json::Value,
}

/// Notification config is source-specific; kept as JSON.
pub type FormNotification = serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Form {
    pub title: String,
    pub fields: Vec<FormField>,
    pub notifications: Vec<FormNotification>,
    pub confirmation: RichText,
    pub retired: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Submission {
    pub form: ItemId,
    /// None when the source's date is missing or doesn't parse.
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
    pub values: IndexMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Settings {
    pub site_name: String,
    pub front_page: Option<ItemId>,
    pub permalink_structure: Option<String>,
    pub seo_defaults: Seo,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Author {
    pub name: String,
    pub email: Option<String>,
    pub slug: String,
}
