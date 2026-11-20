//! Analyses a Sanity export: type census, builders, references, assets, and which types
//! are form submissions or singletons, from the data alone.

use super::assets;
use super::export::Export;
use super::fields::{array_types, item_path};
use super::shape::{Shape, shape};
use crate::mapping::{MappingConfig, TypeMap};
use crate::project::Project;
use crate::report::{ConnectorCoverage, Coverage, write_coverage};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// What porter can't learn from the data.
const UNKNOWNS: &[&str] = &["Studio field titles and validation", "frontend URL rules"];
const ANALYSIS_FILE: &str = "analysis.json";
const SUBMISSION_DROP: &str = "form submissions; loaded as submissions";

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TypeCount {
    pub published: u64,
    pub draft_only: u64,
    pub draft_of_published: u64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RefCount {
    pub from: String,
    pub field: String,
    pub to: String,
    pub count: u64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Dangling {
    pub from_id: String,
    pub field: String,
    pub to_id: String,
    pub weak: bool,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct AssetCounts {
    pub referenced: u64,
    pub referenced_bytes: u64,
    pub unreferenced: u64,
    pub unreferenced_bytes: u64,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Analysis {
    pub types: BTreeMap<String, TypeCount>,
    /// Plugin or system types (name contains `.`) and their document counts.
    pub skipped_types: BTreeMap<String, u64>,
    /// Item `_type` -> occurrences, in page-builder fields only.
    pub blocks: BTreeMap<String, u64>,
    /// Item `_type` -> field names seen on its items (seeds block field maps).
    pub block_fields: BTreeMap<String, BTreeSet<String>>,
    pub references: Vec<RefCount>,
    pub dangling: Vec<Dangling>,
    pub submission_types: Vec<String>,
    pub singletons: Vec<String>,
    pub assets: AssetCounts,
    pub unknowns: Vec<String>,
}

fn base_id(id: &str) -> &str {
    id.strip_prefix("drafts.").unwrap_or(id)
}

/// Walks one document: counts builder items and collects reference edges.
struct Scan<'a> {
    ex: &'a Export,
    builders: HashSet<String>,
    target_type: HashMap<&'a str, &'a str>,
    out: &'a mut Analysis,
    refs: BTreeMap<(String, String, String), u64>,
}

impl Scan<'_> {
    fn walk(&mut self, v: &Value, path: &str, field: &str, doc: &Value) {
        match v {
            Value::Object(o) => {
                if let Shape::Reference { id: rid, weak } = shape(v) {
                    if self.ex.assets.contains_key(&rid) {
                        return;
                    }
                    let from = doc_type(doc).to_string();
                    match self.target_type.get(rid.as_str()) {
                        Some(to) => {
                            *self
                                .refs
                                .entry((from, field.to_string(), to.to_string()))
                                .or_default() += 1
                        }
                        None => self.out.dangling.push(Dangling {
                            from_id: base_id(id(doc)).to_string(),
                            field: field.to_string(),
                            to_id: rid,
                            weak,
                        }),
                    }
                    return;
                }
                for (k, x) in o.iter().filter(|(k, _)| !k.starts_with('_')) {
                    let f = if field.is_empty() {
                        k.clone()
                    } else {
                        format!("{field}.{k}")
                    };
                    self.walk(x, &format!("{path}.{k}"), &f, doc);
                }
            }
            Value::Array(a) => {
                let builder = self.builders.contains(path);
                for x in a {
                    if builder && let Some(t) = ty(x) {
                        *self.out.blocks.entry(t.to_string()).or_default() += 1;
                        let fs = self.out.block_fields.entry(t.to_string()).or_default();
                        if let Some(o) = x.as_object() {
                            fs.extend(o.keys().filter(|k| !k.starts_with('_')).cloned());
                        }
                    }
                    self.walk(x, &item_path(path, x), field, doc);
                }
            }
            _ => {}
        }
    }
}

fn doc_type(d: &Value) -> &str {
    ty(d).unwrap_or_default()
}

/// The census of an export. `min_docs` is the submission threshold (see [`MIN_SUBMISSION_DOCS`]).
pub(crate) fn analyse(ex: &Export, min_docs: usize) -> Analysis {
    let mut out = Analysis {
        unknowns: UNKNOWNS.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let mut content: Vec<&Value> = vec![];
    for d in &ex.docs {
        match ty(d) {
            Some(t) if is_plugin_type(t) => {
                *out.skipped_types.entry(t.to_string()).or_default() += 1
            }
            Some(_) => content.push(d),
            None => {}
        }
    }
    // Type census, and the document each id resolves to (published beats draft).
    let published_ids: HashSet<&str> = content
        .iter()
        .filter(|d| published(d))
        .map(|d| id(d))
        .collect();
    let mut effective: BTreeMap<&str, &Value> = BTreeMap::new();
    for d in &content {
        let c = out.types.entry(doc_type(d).to_string()).or_default();
        if published(d) {
            c.published += 1;
        } else if published_ids.contains(base_id(id(d))) {
            c.draft_of_published += 1;
        } else {
            c.draft_only += 1;
        }
        let e = effective.entry(base_id(id(d))).or_insert(d);
        if published(d) {
            *e = d;
        }
    }
    let builders = array_types(&content)
        .into_iter()
        .filter(|(_, t)| t.len() >= 2)
        .map(|(p, _)| p)
        .collect();
    let target_type = effective.iter().map(|(i, d)| (*i, doc_type(d))).collect();
    let mut scan = Scan {
        ex,
        builders,
        target_type,
        out: &mut out,
        refs: BTreeMap::new(),
    };
    for d in effective.values() {
        scan.walk(d, doc_type(d), "", d);
    }
    let refs = std::mem::take(&mut scan.refs);
    out.references = refs
        .into_iter()
        .map(|((from, field, to), count)| RefCount {
            from,
            field,
            to,
            count,
        })
        .collect();
    out.dangling
        .sort_by(|a, b| (&a.from_id, &a.field, &a.to_id).cmp(&(&b.from_id, &b.field, &b.to_id)));
    out.submission_types = detect_submissions(&ex.docs, min_docs);
    out.singletons = detect_singletons(&ex.docs);
    let used = assets::referenced(ex);
    for (aid, a) in &ex.assets {
        let (n, b) = if used.contains(aid) {
            (&mut out.assets.referenced, &mut out.assets.referenced_bytes)
        } else {
            (
                &mut out.assets.unreferenced,
                &mut out.assets.unreferenced_bytes,
            )
        };
        *n += 1;
        *b += a.size;
    }
    out
}

/// Writes `raw/sanity/analysis.json` and the coverage report.
pub(crate) fn write(project: &Project, ex: &Export) -> Result<()> {
    let a = analyse(ex, MIN_SUBMISSION_DOCS);
    let dir = project.raw_dir(super::CANON);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(ANALYSIS_FILE), serde_json::to_vec_pretty(&a)?)?;
    let n = |f: fn(&TypeCount) -> u64| a.types.values().map(f).sum::<u64>();
    write_coverage(
        project,
        &Coverage {
            platform: Some("sanity".into()),
            connectors: vec![ConnectorCoverage {
                name: super::CANON.into(),
                contributed: BTreeMap::from([
                    ("documents".into(), n(|t| t.published)),
                    ("drafts".into(), n(|t| t.draft_only + t.draft_of_published)),
                    ("assets".into(), ex.assets.len() as u64),
                ]),
                gaps: a.unknowns.clone(),
            }],
            ..Default::default()
        },
    )
}

fn kebab(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 && !out.ends_with('-') {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

impl Analysis {
    /// Folds this analysis into a seed made from raw rows: drops plugin types, drops
    /// submission types (listed under `forms`), and maps every builder block to a kebab slug.
    fn seed(&self, m: &mut MappingConfig) {
        m.types.retain(|t, _| self.types.contains_key(t));
        // Sanity holds no URLs: no seeded pattern, so normalise writes no routes until the
        // user enters a real one.
        m.types.values_mut().for_each(|t| t.path = None);
        for t in &self.submission_types {
            m.types.insert(
                t.clone(),
                TypeMap {
                    drop_reason: Some(SUBMISSION_DROP.into()),
                    ..TypeMap::default()
                },
            );
        }
        m.forms.submission_types = self.submission_types.clone();
        for t in self.blocks.keys() {
            m.blocks.push(crate::mapping::BlockMap {
                r#match: crate::mapping::BlockMatch {
                    kind: None,
                    module: Some(t.clone()),
                },
                to: kebab(t),
                fields: self.block_fields[t]
                    .iter()
                    .map(|f| (f.clone(), f.clone()))
                    .collect(),
            });
        }
    }
}

/// Sanity-specific additions to the seed; does nothing unless `raw/sanity/analysis.json` exists.
pub fn seed(project: &Project, m: &mut MappingConfig) -> Result<()> {
    let p = project.raw_dir(super::CANON).join(ANALYSIS_FILE);
    if !p.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    let a: Analysis =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", p.display()))?;
    a.seed(m);
    Ok(())
}

/// Fewest documents a type needs before it can be detected as submissions.
pub(crate) const MIN_SUBMISSION_DOCS: usize = 50;

/// Field names (lower-cased) that mark a contact form.
const CONTACT: &[&str] = &["email", "firstname", "lastname", "phone", "phonenumber"];

fn ty(d: &Value) -> Option<&str> {
    d.get("_type")?.as_str()
}

fn id(d: &Value) -> &str {
    d.get("_id").and_then(Value::as_str).unwrap_or("")
}

/// Plugin or system document types (`media.tag`, `sanity.*`) aren't content.
pub(crate) fn is_plugin_type(t: &str) -> bool {
    t.contains('.')
}

fn published(d: &Value) -> bool {
    !id(d).starts_with("drafts.")
}

/// Content documents grouped by type; plugin types are left out.
fn by_type<'a>(docs: impl Iterator<Item = &'a Value>) -> BTreeMap<&'a str, Vec<&'a Value>> {
    let mut m: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for d in docs {
        if let Some(t) = ty(d).filter(|t| !is_plugin_type(t)) {
            m.entry(t).or_default().push(d);
        }
    }
    m
}

fn has_contact_key(o: &serde_json::Map<String, Value>) -> bool {
    o.keys()
        .any(|k| CONTACT.contains(&k.to_ascii_lowercase().as_str()))
}

/// A contact field at the top level, or inside a nested object whose key ends in `Details`.
fn contact_like(d: &Value) -> bool {
    let Some(o) = d.as_object() else {
        return false;
    };
    has_contact_key(o)
        || o.iter()
            .any(|(k, v)| k.ends_with("Details") && v.as_object().is_some_and(has_contact_key))
}

/// Types with at least `min_docs` published documents, none with a `slug`, and
/// contact-like fields.
pub(crate) fn detect_submissions(docs: &[Value], min_docs: usize) -> Vec<String> {
    by_type(docs.iter())
        .into_iter()
        .filter(|(_, ds)| {
            ds.iter().filter(|d| published(d)).count() >= min_docs
                && ds.iter().all(|d| d.get("slug").is_none())
                && ds.iter().any(|d| contact_like(d))
        })
        .map(|(t, _)| t.to_string())
        .collect()
}

/// Types with exactly one published document, whose `_id` is the type name.
pub(crate) fn detect_singletons(docs: &[Value]) -> Vec<String> {
    by_type(docs.iter().filter(|d| published(d)))
        .into_iter()
        .filter(|(t, ds)| ds.len() == 1 && id(ds[0]) == *t)
        .map(|(t, _)| t.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::TypeMap;
    use serde_json::json;

    fn fixture_analysis(min_docs: usize) -> Analysis {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sanity");
        analyse(&super::super::export::load(&dir).unwrap(), min_docs)
    }

    fn tc(published: u64, draft_only: u64, draft_of_published: u64) -> TypeCount {
        TypeCount {
            published,
            draft_only,
            draft_of_published,
        }
    }

    #[test]
    fn analysis_counts_drafts_and_assets() {
        let a = fixture_analysis(2);
        assert_eq!(
            a.types,
            BTreeMap::from([
                ("cityPage".into(), tc(3, 0, 0)),
                ("countryPage".into(), tc(2, 0, 0)),
                ("enquiryForm".into(), tc(3, 0, 0)),
                ("homepage".into(), tc(1, 0, 0)),
                ("hotelBreak".into(), tc(2, 1, 1)),
                ("regionPage".into(), tc(1, 0, 0)),
                ("siteSettings".into(), tc(1, 0, 0)),
            ])
        );
        assert_eq!(a.skipped_types, BTreeMap::from([("media.tag".into(), 1)]));
        // Builder fields only: single-type arrays (`cards`, `items`) aren't blocks. Counted
        // over the published doc, or the draft when nothing is published (3 hotel breaks).
        assert_eq!(
            a.blocks,
            BTreeMap::from([
                ("accordion".into(), 3),
                ("cardCarousel".into(), 3),
                ("customCardCarousel".into(), 3),
                ("hero".into(), 3),
            ])
        );
        assert_eq!(
            a.block_fields["hero"],
            BTreeSet::from(["heading".to_string(), "image".to_string()])
        );
        let r = |from: &str, field: &str, to: &str, count| RefCount {
            from: from.into(),
            field: field.into(),
            to: to.into(),
            count,
        };
        assert_eq!(
            a.references,
            [
                r("cityPage", "country", "countryPage", 3),
                r("countryPage", "region", "regionPage", 2),
                r("hotelBreak", "city", "cityPage", 3),
                r("hotelBreak", "highlights.city", "cityPage", 1),
                r("hotelBreak", "related", "cityPage", 1),
            ]
        );
        let d = |field: &str, to_id: &str, weak| Dangling {
            from_id: "hb-2".into(),
            field: field.into(),
            to_id: to_id.into(),
            weak,
        };
        assert_eq!(
            a.dangling,
            [
                d("dangling", "missing-doc", false),
                d("weakDangling", "gone-doc", true)
            ]
        );
        assert_eq!(a.submission_types, ["enquiryForm"]);
        assert_eq!(a.singletons, ["homepage", "siteSettings"]);
        // Three of the four assets are referenced; every fixture asset is 7 bytes.
        assert_eq!(
            a.assets,
            AssetCounts {
                referenced: 3,
                referenced_bytes: 21,
                unreferenced: 1,
                unreferenced_bytes: 7
            }
        );
        assert_eq!(a.unknowns, UNKNOWNS);
        // The real threshold is 50 documents.
        assert!(
            fixture_analysis(MIN_SUBMISSION_DOCS)
                .submission_types
                .is_empty()
        );
    }

    #[test]
    fn seed_maps_every_block_type() {
        let a = fixture_analysis(2);
        let mut m = MappingConfig::default();
        for t in ["hotelBreak", "enquiryForm", "media.tag"] {
            m.types.insert(
                t.into(),
                TypeMap {
                    path: Some("/{slug}/".into()),
                    ..TypeMap::default()
                },
            );
        }
        a.seed(&mut m);
        assert!(!m.types.contains_key("media.tag"));
        assert!(m.types.values().all(|t| t.path.is_none()));
        let enq = &m.types["enquiryForm"];
        assert_eq!(enq.to, None);
        assert_eq!(enq.drop_reason.as_deref(), Some(SUBMISSION_DROP));
        assert_eq!(m.forms.submission_types, ["enquiryForm"]);
        let blocks: Vec<_> = m
            .blocks
            .iter()
            .map(|b| (b.r#match.module.clone().unwrap(), b.to.clone()))
            .collect();
        assert_eq!(
            blocks,
            [
                ("accordion".into(), "accordion".into()),
                ("cardCarousel".into(), "card-carousel".into()),
                ("customCardCarousel".into(), "custom-card-carousel".into()),
                ("hero".into(), "hero".into()),
            ]
        );
        let hero = m.blocks.iter().find(|b| b.to == "hero").unwrap();
        assert_eq!(hero.fields["heading"], "heading");
        assert_eq!(m.raw_block, None);
    }

    #[test]
    fn submissions_need_count_no_slug_and_contact_fields() {
        let docs = vec![
            json!({"_id":"a1","_type":"enquiry","name":"A","email":"a@example.com"}),
            json!({"_id":"a2","_type":"enquiry","name":"B"}),
            json!({"_id":"b1","_type":"lead","leadPassengerDetails":{"firstName":"C"}}),
            json!({"_id":"b2","_type":"lead","leadPassengerDetails":{"firstName":"D"}}),
            json!({"_id":"c1","_type":"page","email":"x","slug":{"current":"a"}}),
            json!({"_id":"c2","_type":"page","email":"y"}),
            json!({"_id":"d1","_type":"note","body":"x"}),
            json!({"_id":"d2","_type":"note","body":"y"}),
            json!({"_id":"e1","_type":"single","email":"z"}),
            // Drafts don't count toward min_docs.
            json!({"_id":"f1","_type":"draftOnly","email":"a"}),
            json!({"_id":"drafts.f2","_type":"draftOnly","email":"b"}),
        ];
        assert_eq!(detect_submissions(&docs, 2), ["enquiry", "lead"]);
        assert!(detect_submissions(&docs, MIN_SUBMISSION_DOCS).is_empty());
    }

    #[test]
    fn singletons_have_one_published_doc_and_a_fixed_id() {
        let docs = vec![
            json!({"_id":"siteSettings","_type":"siteSettings"}),
            json!({"_id":"drafts.siteSettings","_type":"siteSettings"}),
            json!({"_id":"homepage","_type":"homepage"}),
            // One doc, but its id isn't its type.
            json!({"_id":"regionPage-r1","_type":"regionPage"}),
            json!({"_id":"0f8fad5b-d9cb-469f-a165-70867728950e","_type":"blogCategories"}),
            json!({"_id":"p1","_type":"page"}),
            json!({"_id":"p2","_type":"page"}),
            json!({"_id":"media.tag","_type":"media.tag"}),
        ];
        assert_eq!(detect_singletons(&docs), ["homepage", "siteSettings"]);
    }
}
