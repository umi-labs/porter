//! What the normalised documents actually hold, per source type and field: which
//! `FieldValue` variants appear, how often a field is filled, and a few sample values.
//! The schema check, generator and prompts work from this.

use crate::store::Store;
use anyhow::Result;
use indexmap::IndexMap;
use porter_model::{Block, BlockKind, Document, FieldValue, ItemKind};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};

/// Samples kept per field (raw: callers strip, mask and truncate before showing them).
pub const SAMPLES: usize = 3;
const SAMPLE_MAX: usize = 400;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct TypeSummary {
    pub types: IndexMap<String, TypeStats>,
    /// Layout blocks seen, by kind (and builder module, when there is one).
    pub blocks: Vec<BlockStats>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct TypeStats {
    /// Documents of this type.
    pub count: usize,
    /// `Document.fields` key -> stats.
    pub fields: IndexMap<String, FieldStats>,
    /// The document's own fields a mapping can name (`seo.*`, `created`, `updated`; see
    /// `mapping::DOC_PATHS`), where no custom field shadows them. Never reported unmapped.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub doc_fields: IndexMap<String, FieldStats>,
}

impl TypeStats {
    /// A mappable path's stats: a custom field, else the document's own.
    pub fn field(&self, path: &str) -> Option<&FieldStats> {
        self.fields.get(path).or_else(|| self.doc_fields.get(path))
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct FieldStats {
    /// Non-null `FieldValue` variant names seen (`Text`, `RichText`, `Layout`, ...).
    pub variants: BTreeSet<String>,
    /// For `List`s: the variants of their items.
    pub item_variants: BTreeSet<String>,
    /// For `Doc` references (direct or in lists): the source types they point at.
    pub relation_types: BTreeSet<String>,
    /// Documents where the field is present and not null.
    pub filled: usize,
    /// `filled / count` of the type.
    pub fill_rate: f64,
    pub samples: Vec<String>,
    /// For `Object`s and `List`s of `Object`: stats per key, merged over every object seen.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub children: IndexMap<String, FieldStats>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct BlockStats {
    /// Kebab-case kind, what `[[mapping.blocks]] match.kind` names.
    pub kind: String,
    pub module: Option<String>,
    pub count: usize,
    pub props: IndexMap<String, FieldStats>,
}

pub fn variant(v: &FieldValue) -> &'static str {
    match v {
        FieldValue::Null => "Null",
        FieldValue::Text(_) => "Text",
        FieldValue::Number(_) => "Number",
        FieldValue::Bool(_) => "Bool",
        FieldValue::Date(_) => "Date",
        FieldValue::RichText(_) => "RichText",
        FieldValue::Layout(_) => "Layout",
        FieldValue::Media(_) => "Media",
        FieldValue::Doc(_) => "Doc",
        FieldValue::List(_) => "List",
        FieldValue::Object(_) => "Object",
    }
}

/// The kind's kebab-case wire name (`rich-text`, `image`, ...).
pub fn block_kind(k: &BlockKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v["type"].as_str().map(String::from))
        })
        .unwrap_or_default()
}

/// A short human rendering of a value, or None when there's nothing worth showing (ids).
fn sample(v: &FieldValue) -> Option<String> {
    let s = match v {
        FieldValue::Text(t) => t.clone(),
        FieldValue::Number(n) => n.to_string(),
        FieldValue::Bool(b) => b.to_string(),
        FieldValue::Date(d) => d.to_rfc3339(),
        FieldValue::RichText(r) => r.plain_text(),
        FieldValue::Layout(bs) => bs
            .iter()
            .map(|b| block_kind(&b.kind))
            .collect::<Vec<_>>()
            .join(", "),
        FieldValue::List(items) => items
            .iter()
            .filter_map(sample)
            .collect::<Vec<_>>()
            .join(", "),
        _ => return None,
    };
    let s = s.trim();
    (!s.is_empty()).then(|| s.chars().take(SAMPLE_MAX).collect())
}

impl FieldStats {
    /// Folds `o` into these stats (the same field seen in several block kinds).
    pub fn merge(&mut self, o: &FieldStats) {
        self.variants.extend(o.variants.iter().cloned());
        self.item_variants.extend(o.item_variants.iter().cloned());
        self.relation_types.extend(o.relation_types.iter().cloned());
        self.samples.extend(o.samples.iter().cloned());
        for (k, c) in &o.children {
            self.children.entry(k.clone()).or_default().merge(c);
        }
    }

    fn observe(&mut self, v: &FieldValue, types: &HashMap<porter_model::ItemId, String>) {
        if matches!(v, FieldValue::Null) {
            return;
        }
        self.filled += 1;
        self.variants.insert(variant(v).into());
        let mut doc = |v: &FieldValue| {
            if let FieldValue::Doc(id) = v
                && let Some(t) = types.get(id)
            {
                self.relation_types.insert(t.clone());
            }
        };
        doc(v);
        if let FieldValue::List(items) = v {
            for i in items {
                doc(i);
            }
            self.item_variants
                .extend(items.iter().map(|i| variant(i).to_string()));
            self.item_variants.remove("Null");
        }
        // Objects, alone or as list rows: per-key stats, so a `Doc` at any depth is a
        // relation at its own path.
        let rows: Vec<&IndexMap<String, FieldValue>> = match v {
            FieldValue::Object(m) => vec![m],
            FieldValue::List(items) => items
                .iter()
                .filter_map(|i| match i {
                    FieldValue::Object(m) => Some(m),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
        for (k, x) in rows.into_iter().flatten() {
            self.children
                .entry(k.clone())
                .or_default()
                .observe(x, types);
        }
        if self.samples.len() < SAMPLES
            && let Some(s) = sample(v)
            && !self.samples.contains(&s)
        {
            self.samples.push(s);
        }
    }
}

impl TypeSummary {
    /// From the normalised documents in the store (rows that don't decode as `Document`,
    /// such as connector raw items, are skipped).
    pub fn from_store(store: &Store) -> Result<TypeSummary> {
        let docs = store.items_decodable::<Document>(ItemKind::Document)?;
        let types: HashMap<_, _> = docs
            .iter()
            .map(|(id, _, d)| (*id, d.doc_type.clone()))
            .collect();
        let mut s = TypeSummary::default();
        let mut blocks: IndexMap<(String, Option<String>), BlockStats> = IndexMap::new();
        for (_, _, d) in &docs {
            let t = s.types.entry(d.doc_type.clone()).or_default();
            t.count += 1;
            for p in super::DOC_PATHS
                .iter()
                .filter(|p| !d.fields.contains_key(**p))
            {
                let st = t.doc_fields.entry(p.to_string()).or_default();
                st.observe(&super::lookup(d, p), &types);
            }
            for (k, v) in &d.fields {
                t.fields.entry(k.clone()).or_default().observe(v, &types);
                if let FieldValue::Layout(bs) = v {
                    for b in bs {
                        b.walk(&mut |b: &Block| {
                            let kind = block_kind(&b.kind);
                            let module = b.original.as_ref().map(|o| o.module.clone());
                            let st =
                                blocks
                                    .entry((kind.clone(), module.clone()))
                                    .or_insert_with(|| BlockStats {
                                        kind,
                                        module,
                                        ..BlockStats::default()
                                    });
                            st.count += 1;
                            for (pk, pv) in &b.props {
                                st.props.entry(pk.clone()).or_default().observe(pv, &types);
                            }
                        });
                    }
                }
            }
        }
        for t in s.types.values_mut() {
            for f in t.fields.values_mut().chain(t.doc_fields.values_mut()) {
                f.fill_rate = f.filled as f64 / t.count as f64;
            }
        }
        for b in blocks.values_mut() {
            for p in b.props.values_mut() {
                p.fill_rate = p.filled as f64 / b.count as f64;
            }
        }
        s.blocks = blocks.into_values().collect();
        Ok(s)
    }
}
