//! Per-client mapping decisions (`[mapping]` in porter.toml): which source types go where,
//! field renames and transforms, builder blocks, URL patterns. Platform-free.

pub mod paths;
pub mod summary;
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
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct MappingConfig {
    /// Source type -> how it maps. A type with no entry here is unmapped.
    pub types: IndexMap<String, TypeMap>,
    pub blocks: Vec<BlockMap>,
    /// Destination block slug for content that has no native shape (raw HTML, embeds,
    /// tables, code, unmatched builder blocks); its `html` field gets the markup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_block: Option<String>,
    pub locales: LocaleMode,
    pub forms: FormsMap,
    pub media: MediaMap,
    pub redirects: RedirectsMap,
    /// Taxonomy name -> destination collection for its terms. Unlisted taxonomies aren't loaded.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub taxonomies: IndexMap<String, String>,
    #[serde(skip_serializing_if = "MenusMap::is_empty")]
    pub menus: MenusMap,
    #[serde(skip_serializing_if = "SettingsMap::is_empty")]
    pub settings: SettingsMap,
}

/// Where menus go: a `collection` (one doc per menu: `title`, `items`), or a `global`
/// whose `fields` name, per source menu, the field that takes its items. Each item is
/// `{label, url, children}`. Neither set: menus aren't loaded.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct MenusMap {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    /// Source menu name -> field of `global`.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub fields: IndexMap<String, String>,
}

impl MenusMap {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Site settings into a global: each named field gets that setting. Unset: not loaded.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct SettingsMap {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global: Option<String>,
    /// Field for the front page (a relationship to the loaded document).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub front_page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
}

impl SettingsMap {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
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

/// `{ to = "dest.path" }`, or `{ drop = true, reason = "..." }` to leave a field behind
/// on purpose (no unmapped-field warning). Exactly one of the two; `validate` checks.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FieldMap {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub to: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drop: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// One inline table or a list of them.
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub transform: Vec<Transform>,
}

fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Transform>, D::Error> {
    use serde::de::Error;
    // Via toml::Value so a bad inline table errors naming the key, not "no variant matched".
    let one = |v: toml::Value| {
        // serde ignores extra keys on unit variants of tagged enums; check by hand.
        if let toml::Value::Table(t) = &v
            && matches!(
                t.get("op").and_then(|o| o.as_str()),
                Some("trim" | "lower" | "upper" | "slugify" | "number")
            )
            && let Some(k) = t.keys().find(|k| *k != "op")
        {
            return Err(D::Error::custom(format!(
                "unknown field `{k}` in transform"
            )));
        }
        Transform::deserialize(v).map_err(D::Error::custom)
    };
    match toml::Value::deserialize(d)? {
        toml::Value::Array(a) => a.into_iter().map(one).collect(),
        v => one(v).map(|t| vec![t]),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockMap {
    pub r#match: BlockMatch,
    /// Destination block slug.
    pub to: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub fields: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct BlockMatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
}

/// `locales = { mode = "skip" }` or `{ mode = "map", default = "en", others = ["fr"] }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct FormsMap {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    /// Collection for past submissions (keyed by `legacyId`); unset: not loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submissions: Option<String>,
    /// Form titles not to load.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retire: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    /// Empty means valid. Reports bad regex patterns with their key path.
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        for (ty, tm) in &self.types {
            for (field, fm) in &tm.fields {
                if fm.drop == !fm.to.is_empty() {
                    errs.push(format!(
                        "mapping.types.{ty}.fields.{field}: set exactly one of `to` or `drop = true`"
                    ));
                }
                for (i, t) in fm.transform.iter().enumerate() {
                    if let Transform::Regex { pattern, .. } = t
                        && let Err(e) = transforms::compiled(pattern)
                    {
                        errs.push(format!(
                            "mapping.types.{ty}.fields.{field}.transform[{i}].pattern: {e}"
                        ));
                    }
                }
            }
        }
        errs
    }
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
                    ..FieldMap::default()
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
    let mut base = text.clone();
    if let Some(m) = doc.get("mapping") {
        if m.as_table_like().is_none_or(|t| !t.is_empty()) {
            return Ok(false);
        }
        // An empty [mapping] header: drop it, the seed block replaces it.
        doc.remove("mapping");
        base = doc.to_string();
    }
    #[derive(Serialize)]
    struct Wrap<'a> {
        mapping: &'a MappingConfig,
    }
    let block = toml::to_string(&Wrap { mapping: seed }).context("serialise mapping")?;
    if !base.is_empty() && !base.ends_with('\n') {
        base.push('\n');
    }
    base.push_str(
        "\n# Seeded by `porter analyse`: every source type maps to itself. Edit `to`, set\n\
         # `drop_reason` (and remove `to`) to drop a type, rename fields, add transforms.\n",
    );
    base.push_str(&block);
    let mut tmp = tempfile::NamedTempFile::new_in(project_dir).context("temp file")?;
    std::io::Write::write_all(&mut tmp, base.as_bytes())?;
    tmp.persist(&file)
        .with_context(|| format!("write {}", file.display()))?;
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
submissions = "form-submissions"

[taxonomies]
category = "categories"

[menus]
global = "navigation"
fields = { "Main menu" = "main" }

[settings]
global = "settings"
front_page = "homePage"
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
        assert_eq!(m.menus.fields["Main menu"], "main");
        assert_eq!(m.settings.front_page.as_deref(), Some("homePage"));
        assert_eq!(m.taxonomies["category"], "categories");
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

    #[test]
    fn seed_is_one_contiguous_block_at_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("porter.toml");
        let orig = "# top\n[project]\nname = \"x\"\n\n[source]\nadapter = \"a\"\n\n[destination]\n# url = \"u\"\n\n# [mapping]\n\n# [[redirects]]\n# from = \"/a\"\n";
        std::fs::write(&f, orig).unwrap();
        let mut seed = MappingConfig::default();
        for t in ["post", "page"] {
            seed.types.insert(
                t.into(),
                TypeMap {
                    to: Some(t.into()),
                    ..TypeMap::default()
                },
            );
        }
        assert!(write_seed_if_empty(dir.path(), &seed).unwrap());
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.starts_with(orig));
        let tail = &text[orig.len()..];
        let first = tail
            .lines()
            .position(|l| l.starts_with("[mapping"))
            .unwrap();
        let lines: Vec<&str> = tail.lines().collect();
        assert!(lines[first - 1].starts_with('#'), "comment precedes block");
        assert!(
            lines[first..]
                .iter()
                .all(|l| l.is_empty() || l.starts_with("[mapping") || !l.starts_with('['))
        );
        assert!(!lines[..first].iter().any(|l| l.starts_with('[')));
    }

    #[test]
    fn empty_mapping_table_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("porter.toml");
        std::fs::write(&f, "[project]\nname = \"x\"\n\n[mapping]\n").unwrap();
        let mut seed = MappingConfig::default();
        seed.types.insert("p".into(), TypeMap::default());
        assert!(write_seed_if_empty(dir.path(), &seed).unwrap());
        let t = std::fs::read_to_string(&f).unwrap();
        assert_eq!(t.matches("[mapping]").count(), 1);
        toml::from_str::<toml::Table>(&t).unwrap();
    }

    #[test]
    fn typo_in_type_map_errors() {
        let e = toml::from_str::<MappingConfig>("[types.post]\ntoo = \"pages\"\n").unwrap_err();
        assert!(e.to_string().contains("too"), "{e}");
    }

    #[test]
    fn typo_in_transform_errors() {
        let src = "[types.p.fields.f]\nto = \"x\"\ntransform = [{ op = \"regex\", patern = \"a\", replace = \"b\" }]\n";
        let e = toml::from_str::<MappingConfig>(src).unwrap_err();
        assert!(e.to_string().contains("patern"), "{e}");
        let src = "[types.p.fields.f]\nto = \"x\"\ntransform = { op = \"trim\", extra = 1 }\n";
        let e = toml::from_str::<MappingConfig>(src).unwrap_err();
        assert!(e.to_string().contains("extra"), "{e}");
    }

    #[test]
    fn field_drop_parses_and_validates() {
        let m: MappingConfig = toml::from_str(
            "[types.p.fields.a]\ndrop = true\nreason = \"legacy\"\n[types.p.fields.b]\nto = \"x\"\n",
        )
        .unwrap();
        assert!(m.types["p"].fields["a"].drop);
        assert!(m.validate().is_empty(), "{:?}", m.validate());
        let back: MappingConfig = toml::from_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        for bad in [
            "[types.p.fields.a]\nreason = \"x\"\n",
            "[types.p.fields.a]\nto = \"x\"\ndrop = true\n",
        ] {
            let m: MappingConfig = toml::from_str(bad).unwrap();
            assert!(m.validate()[0].contains("exactly one"), "{bad}");
        }
    }

    #[test]
    fn validate_reports_bad_regex_path() {
        let m: MappingConfig = toml::from_str(
            "[types.p.fields.f]\nto = \"x\"\ntransform = [{ op = \"regex\", pattern = \"(\", replace = \"\" }]\n",
        )
        .unwrap();
        let e = m.validate();
        assert!(
            e[0].starts_with("mapping.types.p.fields.f.transform[0].pattern"),
            "{e:?}"
        );
    }
}
