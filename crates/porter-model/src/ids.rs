use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    Document,
    Media,
    Route,
    Redirect,
    Menu,
    Taxonomy,
    Term,
    Form,
    Submission,
    Settings,
    Author,
}

impl ItemKind {
    /// The kebab-case wire name, also used in the ItemId hash input.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Media => "media",
            Self::Route => "route",
            Self::Redirect => "redirect",
            Self::Menu => "menu",
            Self::Taxonomy => "taxonomy",
            Self::Term => "term",
            Self::Form => "form",
            Self::Submission => "submission",
            Self::Settings => "settings",
            Self::Author => "author",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceRef {
    pub connector: String,
    pub source_id: String,
    pub url: Option<String>,
}

const NAMESPACE: Uuid = Uuid::from_u128(0x6f1c2b0e_4d8a_4c1e_9b7a_0b6f2e9d5a11);

/// Stable item identity: serialises as the plain UUID string.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct ItemId(pub Uuid);

impl ItemId {
    pub fn new(kind: ItemKind, src: &SourceRef) -> ItemId {
        let name = format!("{}|{}|{}", kind.as_str(), src.connector, src.source_id);
        ItemId(Uuid::new_v5(&NAMESPACE, name.as_bytes()))
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ItemId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(ItemId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: &str) -> SourceRef {
        SourceRef { connector: "conn-a".into(), source_id: id.into(), url: None }
    }

    #[test]
    fn item_id_is_stable() {
        let a = ItemId::new(ItemKind::Document, &src("1"));
        assert_eq!(a, ItemId::new(ItemKind::Document, &src("1")));
        assert_ne!(a, ItemId::new(ItemKind::Document, &src("2")));
        assert_ne!(a, ItemId::new(ItemKind::Media, &src("1")));
        assert_eq!(a.to_string().parse::<ItemId>().unwrap(), a);
        assert_eq!(serde_json::to_string(&a).unwrap(), format!("\"{a}\""));
    }

    // python3 -c "import uuid; print(uuid.uuid5(uuid.UUID('6f1c2b0e-4d8a-4c1e-9b7a-0b6f2e9d5a11'), 'document|conn-a|42'))"
    #[test]
    fn item_id_golden_vector() {
        let id = ItemId::new(ItemKind::Document, &src("42"));
        assert_eq!(id.to_string(), "1f0c4c33-b3e1-5d93-a50c-32360d7ec5c9");
    }

    #[test]
    fn kind_wire_name_matches_serde() {
        for k in [ItemKind::Document, ItemKind::Submission, ItemKind::Settings] {
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{}\"", k.as_str()));
        }
    }
}
