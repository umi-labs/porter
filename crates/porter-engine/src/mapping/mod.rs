//! Per-client mapping decisions (`[mapping]` in porter.toml): which source types go where,
//! field renames and transforms, builder blocks, URL patterns. Platform-free.

pub mod paths;
pub mod transforms;

use crate::connectors::RawItem;
use crate::store::Store;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use porter_model::{Document, ItemKind};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;
pub use transforms::{Transform, apply};

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MappingConfig {
    /// Source type -> how it maps. A type with no entry here is unmapped.
    pub types: IndexMap<String, TypeMap>,
    pub blocks: Vec<BlockMap>,
    pub locales: LocaleMode,
    pub forms: FormsMap,
    pub media: MediaMap,
    pub redirects: RedirectsMap,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TypeMap {
    /// Destination collection; omitted = drop this type (give `drop_reason`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_reason: Option<String>,
    /// URL pattern, e.g. `/{type}/{slug}/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub key_field: String,
    pub key_type: KeyType,
    /// Source status -> destination status, e.g. `private = "draft"`.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub status: IndexMap<String, String>,
    /// Neutral field path -> destination field.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub fields: IndexMap<String, FieldMap>,
}

impl Default for TypeMap {
    fn default() -> Self {
        Self {
            to: None,
            drop_reason: None,
            path: None,
            key_field: "legacyId".into(),
            key_type: KeyType::Text,
            status: IndexMap::new(),
            fields: IndexMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum KeyType {
    #[default]
    Text,
    Number,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldMap {
    pub to: String,
    /// One inline table or a list of them.
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub transform: Vec<Transform>,
}

fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Transform>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<Transform>),
        One(Transform),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::Many(v) => v,
        OneOrMany::One(t) => vec![t],
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockMap {
    pub r#match: BlockMatch,
    /// Destination block slug.
    pub to: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub fields: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BlockMatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
}

/// `locales = { mode = "skip" }` or `{ mode = "map", default = "en", others = ["fr"] }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum LocaleMode {
    #[default]
    Skip,
    Map {
        default: String,
        #[serde(default)]
        others: Vec<String>,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FormsMap {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retire: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MediaMap {
    pub collection: String,
    pub folders: bool,
}

impl Default for MediaMap {
    fn default() -> Self {
        Self {
            collection: "media".into(),
            folders: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RedirectsMap {
    pub collection: String,
}

impl Default for RedirectsMap {
    fn default() -> Self {
        Self {
            collection: "redirects".into(),
        }
    }
}

impl MappingConfig {
    /// One TypeMap per source type, mapped to itself. Uses normalised Documents when there
    /// are any (every observed field path maps to itself); before normalise, falls back to
    /// the connectors' raw labels (types only). Nothing is dropped; the user edits.
    pub fn seed_from(store: &Store) -> MappingConfig {
        let mut types: IndexMap<String, TypeMap> = IndexMap::new();
        let seeded = |name: &str| TypeMap {
            to: Some(name.to_string()),
            path: Some("/{slug}/".into()),
            ..TypeMap::default()
        };
        let docs = store
            .items_decodable::<Document>(ItemKind::Document)
            .unwrap_or_default();
        if docs.is_empty() {
            let raws = store
                .items_decodable::<RawItem>(ItemKind::Document)
                .unwrap_or_default();
            for (_, _, r) in raws {
                types
                    .entry(r.label.clone())
                    .or_insert_with(|| seeded(&r.label));
            }
        }
        for (_, _, d) in docs {
            let tm = types
                .entry(d.doc_type.clone())
                .or_insert_with(|| seeded(&d.doc_type));
            for k in d.fields.keys() {
                tm.fields.entry(k.clone()).or_insert_with(|| FieldMap {
                    to: k.clone(),
                    transform: vec![],
                });
            }
        }
        MappingConfig {
            types,
            ..MappingConfig::default()
        }
    }
}

/// Appends `[mapping]` to `<project_dir>/porter.toml` unless it already has a non-empty
/// mapping table. Edits in place with toml_edit so the user's comments survive.
/// Returns whether anything was written.
pub fn write_seed_if_empty(project_dir: &Path, seed: &MappingConfig) -> Result<bool> {
    if seed.types.is_empty() {
        return Ok(false);
    }
    let file = project_dir.join("porter.toml");
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
    let mut doc: toml_edit::DocumentMut = text.parse().context("parse porter.toml")?;
    if let Some(m) = doc.get("mapping")
        && m.as_table_like().is_none_or(|t| !t.is_empty())
    {
        return Ok(false);
    }
    let mut table = toml::to_string(seed)
        .context("serialise mapping")?
        .parse::<toml_edit::DocumentMut>()?
        .into_table();
    table.decor_mut().set_prefix(
        "\n# Seeded by `porter analyse`: every source type maps to itself. Edit `to`, set\n\
         # `drop_reason` (and remove `to`) to drop a type, rename fields, add transforms.\n",
    );
    doc["mapping"] = toml_edit::Item::Table(table);
    std::fs::write(&file, doc.to_string()).with_context(|| format!("write {}", file.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
locales = { mode = "map", default = "en", others = ["fr"] }

[types.post]
to = "pages"
path = "{parent.path}/{slug}/"
status = { private = "draft" }

[types.post.fields."meta.subtitle"]
to = "subtitle"
transform = [{ op = "trim" }, { op = "regex", pattern = "^x", replace = "y" }]

[types.event]
to = "meeting-rooms"
key_type = "number"

[types.event.fields.cap]
to = "capacity"
transform = { op = "number" }

[types.member]
drop_reason = "members are handled by the CRM"

[[blocks]]
to = "hero"
match = { module = "fullwidth_header" }
fields = { title = "heading" }

[forms]
retire = ["contact"]
"#;

    #[test]
    fn mapping_toml_roundtrip() {
        let m: MappingConfig = toml::from_str(SAMPLE).unwrap();
        assert_eq!(m.types["post"].to.as_deref(), Some("pages"));
        assert_eq!(m.types["member"].to, None);
        assert!(m.types["member"].drop_reason.is_some());
        assert_eq!(m.types["post"].key_field, "legacyId");
        assert_eq!(m.types["event"].key_type, KeyType::Number);
        assert_eq!(
            m.types["event"].fields["cap"].transform,
            vec![Transform::Number]
        );
        assert_eq!(m.types["post"].fields["meta.subtitle"].transform.len(), 2);
        assert_eq!(
            m.blocks[0].r#match.module.as_deref(),
            Some("fullwidth_header")
        );
        assert_eq!(m.media.collection, "media");
        assert!(matches!(m.locales, LocaleMode::Map { .. }));
        let back: MappingConfig = toml::from_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn empty_config_is_default() {
        assert_eq!(
            toml::from_str::<MappingConfig>("").unwrap(),
            MappingConfig::default()
        );
    }

    #[test]
    fn seed_roundtrips_and_write_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("porter.toml");
        std::fs::write(&f, "# my comment\nname = \"x\"\n\n# [mapping]\n").unwrap();
        let mut seed = MappingConfig::default();
        seed.types.insert(
            "post".into(),
            TypeMap {
                to: Some("post".into()),
                path: Some("/{slug}/".into()),
                ..TypeMap::default()
            },
        );
        assert!(write_seed_if_empty(dir.path(), &seed).unwrap());
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("# my comment"));
        let parsed: toml::Table = toml::from_str(&text).unwrap();
        let m: MappingConfig = parsed["mapping"].clone().try_into().unwrap();
        assert_eq!(m, seed);
        assert!(!write_seed_if_empty(dir.path(), &seed).unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), text);
    }
}
