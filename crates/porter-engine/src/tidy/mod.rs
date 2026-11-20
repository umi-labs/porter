//! Tidy-ups (`[[tidy]]` in porter.toml): rules that merge several source types into one
//! collection, or several block types into one block. They are compiled into the mapping
//! (`effective_mapping`), so they change where things go in schema and load, never what
//! normalise recorded. `account` checks, before load, that every field of every merged
//! item is carried, renamed or dropped on purpose.

mod account;
mod compile;
pub mod suggest;

pub(crate) use account::blocks;
pub use account::{Problem, REPORT, RuleOutcome, account, check_before_load};
pub use compile::{DISCRIMINATOR, effective_mapping};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TidyRule {
    pub id: String,
    /// Off by default: suggested rules are written disabled.
    #[serde(default)]
    pub enabled: bool,
    /// Turned down (`porter tidy dismiss`): kept so it is never suggested again, off, and
    /// left out of the flow.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dismissed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(flatten)]
    pub kind: TidyKind,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TidyKind {
    /// Several document types into one collection, each keeping what it was in
    /// `discriminator.field`; a type's parent can come from one of its reference fields.
    MergeTypes {
        from: Vec<String>,
        into: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        discriminator: Option<Discriminator>,
        /// Source type -> its reference field that names the parent.
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        parent: IndexMap<String, String>,
        /// `"<type>.<field>"` -> why it is left behind.
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        drop: IndexMap<String, String>,
    },
    /// Several builder block types into one block, with field names aligned.
    MergeBlocks {
        from: Vec<String>,
        into: String,
        /// `"<block type>.<field>"` -> destination field.
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        fields: IndexMap<String, String>,
        /// `"<block type>.<field>"` -> why it is left behind.
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        drop: IndexMap<String, String>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Discriminator {
    pub field: String,
    /// Source type -> the value its documents get.
    pub values: IndexMap<String, String>,
}

/// `"<type>.<field>"` split at the first dot.
fn split_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('.')
        .filter(|(t, f)| !t.is_empty() && !f.is_empty())
}
