//! Sanity field values → neutral `FieldValue`s, by their shape.

use super::portable_text::{PtCx, pt_to_richtext};
use super::shape::{Shape, asset_ref, shape};
use crate::html::PendingIssue;
use indexmap::IndexMap;
use porter_model::{Block, BlockKind, FieldValue, IssueKind, ItemId, Original, Severity};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MAX_DEPTH: usize = 64;

/// Per-use image details; the first value of each wins.
#[derive(Default)]
pub(super) struct MediaUse {
    pub focal: Option<(f64, f64)>,
    pub alt: Option<String>,
    pub caption: Option<String>,
}

/// What converting one document needs, plus the run-wide media uses it adds to.
pub(super) struct Cx<'a> {
    /// Sanity `_id` (no `drafts.`) → document item.
    pub docs: &'a HashMap<String, ItemId>,
    /// Asset `_id` → downloaded media item.
    pub media: &'a HashMap<String, ItemId>,
    /// Asset ids in the export, downloaded or not.
    pub assets: &'a BTreeSet<String>,
    pub uses: &'a mut BTreeMap<String, MediaUse>,
    /// Field path → item `_type`s seen in that array across the export (see [`array_types`]).
    pub arrays: &'a HashMap<String, BTreeSet<String>>,
    /// Assets whose uses disagree.
    pub conflicts: &'a mut BTreeSet<String>,
    /// `<type> "<title>"`, for messages.
    pub label: String,
    pub issues: Vec<PendingIssue>,
}

impl Cx<'_> {
    fn issue(&mut self, kind: IssueKind, severity: Severity, message: String, details: Value) {
        self.issues.push(PendingIssue {
            kind,
            severity,
            message,
            details,
        });
    }

    /// Records one use of an image or file: focal point, alt, caption, crop.
    fn record_use(&mut self, aref: &str, v: &Value) {
        let pct = |x: f64| (x * 10000.0).round() / 100.0;
        let focal = v
            .get("hotspot")
            .and_then(|h| Some((pct(h.get("x")?.as_f64()?), pct(h.get("y")?.as_f64()?))));
        let text = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let (alt, caption) = (text("alt"), text("caption"));
        let cropped = v.get("crop").is_some_and(|c| {
            ["top", "bottom", "left", "right"]
                .iter()
                .any(|k| c.get(k).and_then(Value::as_f64).is_some_and(|n| n > 0.0))
        });
        if cropped {
            let msg = format!(
                "{} crops {aref}; Payload has no crop, so the full image is used",
                self.label
            );
            self.issue(
                IssueKind::UnmappedField,
                Severity::Info,
                msg,
                json!({"asset": aref}),
            );
        }
        let u = self.uses.entry(aref.to_string()).or_default();
        let clash = matches!((&u.focal, &focal), (Some(a), Some(b)) if a != b)
            || matches!((&u.alt, &alt), (Some(a), Some(b)) if a != b);
        u.focal = u.focal.or(focal);
        u.alt = u.alt.take().or(alt);
        u.caption = u.caption.take().or(caption);
        if clash {
            self.conflicts.insert(aref.to_string());
        }
    }

    fn media(&mut self, v: &Value) -> FieldValue {
        let Some(aref) = asset_ref(v) else {
            let msg = format!("{} has an image or file with no asset", self.label);
            self.issue(IssueKind::MissingMedia, Severity::Warning, msg, json!({}));
            return FieldValue::Null;
        };
        self.record_use(aref, v);
        if let Some(id) = self.media.get(aref) {
            return FieldValue::Media(*id);
        }
        let msg = if self.assets.contains(aref) {
            format!("{aref} was not downloaded; run extract")
        } else {
            format!("{aref} is not in the export")
        };
        self.issue(
            IssueKind::MissingMedia,
            Severity::Warning,
            msg,
            json!({"asset": aref}),
        );
        FieldValue::Null
    }

    fn reference(&mut self, id: &str, weak: bool) -> FieldValue {
        if let Some(d) = self.docs.get(id) {
            return FieldValue::Doc(*d);
        }
        let (severity, what) = if weak {
            (Severity::Warning, "weak reference")
        } else {
            (Severity::Error, "reference")
        };
        let msg = format!(
            "{} has a {what} to {id}, which is not in the export",
            self.label
        );
        self.issue(
            IssueKind::UnresolvedRelation,
            severity,
            msg,
            json!({"ref": id, "weak": weak}),
        );
        FieldValue::Null
    }

    fn portable_text(&mut self, items: &[Value]) -> FieldValue {
        for item in items {
            if item.get("_type").and_then(Value::as_str) == Some("image")
                && let Some(aref) = asset_ref(item)
            {
                self.record_use(aref, item);
            }
        }
        let (docs, media) = (self.docs, self.media);
        let resolve_doc = |id: &str| docs.get(id).copied();
        let resolve_asset = |r: &str| media.get(r).copied();
        let mut pt = PtCx {
            resolve_doc: &resolve_doc,
            resolve_asset: &resolve_asset,
            issues: &mut self.issues,
        };
        FieldValue::RichText(pt_to_richtext(items, &mut pt))
    }

    /// One page-builder item at `path` (the item's own path, see [`item_path`]).
    fn block(&mut self, item: &Value, path: &str, depth: usize) -> Block {
        let module = item
            .get("_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let kind = match module.as_str() {
            "imageItem" | "image" => BlockKind::Image,
            "video" => BlockKind::Video,
            "separator" => BlockKind::Divider,
            "accordion" => BlockKind::Accordion,
            _ => BlockKind::Raw,
        };
        let mut props = IndexMap::new();
        let is_media = matches!(shape(item), Shape::Image | Shape::File);
        if is_media {
            props.insert("image".to_string(), self.media(item));
        }
        for (k, v) in item.as_object().into_iter().flatten() {
            if k.starts_with('_')
                || (is_media && ["asset", "hotspot", "crop"].contains(&k.as_str()))
            {
                continue;
            }
            props.insert(
                k.clone(),
                self.conv(v, &format!("{path}.{k}"), depth + 1, false),
            );
        }
        Block {
            kind,
            props,
            children: vec![],
            original: Some(Original {
                builder: "sanity".into(),
                module,
                classes: vec![],
                settings: item.clone(),
            }),
        }
    }

    fn object(
        &mut self,
        o: &Map<String, Value>,
        path: &str,
        depth: usize,
        in_list: bool,
    ) -> FieldValue {
        FieldValue::Object(
            o.iter()
                .filter_map(|(k, v)| match k.as_str() {
                    "_type" if in_list => Some((k.clone(), FieldValue::Text(v.as_str()?.into()))),
                    _ if k.starts_with('_') => None,
                    _ => Some((
                        k.clone(),
                        self.conv(v, &format!("{path}.{k}"), depth + 1, false),
                    )),
                })
                .collect(),
        )
    }

    /// A field value typed by its shape. `path` locates it (see [`array_types`]);
    /// `in_list`: the value is a list item, so its `_type` is kept as the discriminator.
    pub(super) fn conv(
        &mut self,
        v: &Value,
        path: &str,
        depth: usize,
        in_list: bool,
    ) -> FieldValue {
        if depth > MAX_DEPTH {
            let msg = format!(
                "{} has a value nested deeper than {MAX_DEPTH}; kept as JSON text",
                self.label
            );
            self.issue(IssueKind::UnmappedField, Severity::Info, msg, json!({}));
            return FieldValue::Text(v.to_string());
        }
        match shape(v) {
            Shape::Reference { id, weak } => self.reference(&id, weak),
            Shape::Image | Shape::File => self.media(v),
            Shape::PortableText => self.portable_text(v.as_array().map_or(&[], Vec::as_slice)),
            // A builder when the field ever holds two item types, else a plain list.
            Shape::Builder if self.arrays.get(path).is_some_and(|t| t.len() >= 2) => {
                FieldValue::Layout(
                    v.as_array()
                        .into_iter()
                        .flatten()
                        .map(|x| self.block(x, &item_path(path, x), depth))
                        .collect(),
                )
            }
            Shape::Slug(s) => FieldValue::Text(s),
            Shape::Geopoint { lat, lng } => FieldValue::Object(IndexMap::from([
                ("lat".to_string(), FieldValue::Number(lat)),
                ("lng".to_string(), FieldValue::Number(lng)),
            ])),
            Shape::Date(d) => FieldValue::Date(d),
            Shape::List | Shape::Builder => FieldValue::List(
                v.as_array()
                    .into_iter()
                    .flatten()
                    .map(|x| self.conv(x, &item_path(path, x), depth + 1, true))
                    .collect(),
            ),
            Shape::Object => match v.as_object() {
                Some(o) => self.object(o, path, depth, in_list),
                None => FieldValue::Null,
            },
            Shape::Scalar => match v {
                Value::Bool(b) => FieldValue::Bool(*b),
                Value::Number(n) => n
                    .as_f64()
                    .map_or_else(|| FieldValue::Text(n.to_string()), FieldValue::Number),
                Value::String(s) => FieldValue::Text(s.clone()),
                _ => FieldValue::Null,
            },
        }
    }

    /// The image in an SEO object's `ogImage`/`shareImage`.
    pub(super) fn og_image(&mut self, v: &Value) -> Option<ItemId> {
        match self.media(v) {
            FieldValue::Media(id) => Some(id),
            _ => None,
        }
    }
}

/// An array item's path: `<array path>[<item _type>]`.
pub(super) fn item_path(path: &str, item: &Value) -> String {
    let ty = item
        .get("_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{path}[{ty}]")
}

/// Every typed-object array in the export (published and drafts), keyed by field path,
/// with the item `_type`s it ever holds. A path starts at the document type and adds
/// `.<key>` per object key and `[<item _type>]` per array item, e.g.
/// `hotelBreak.flexibleContent[cardCarousel].cards`.
pub(super) fn array_types(docs: &[&Value]) -> HashMap<String, BTreeSet<String>> {
    fn walk(v: &Value, path: &str, depth: usize, out: &mut HashMap<String, BTreeSet<String>>) {
        if depth > MAX_DEPTH {
            return;
        }
        match v {
            Value::Object(o) => {
                for (k, x) in o.iter().filter(|(k, _)| !k.starts_with('_')) {
                    walk(x, &format!("{path}.{k}"), depth + 1, out);
                }
            }
            Value::Array(a) => {
                if shape(v) == Shape::Builder {
                    let types = out.entry(path.to_string()).or_default();
                    types.extend(
                        a.iter()
                            .filter_map(|x| x.get("_type")?.as_str())
                            .map(str::to_string),
                    );
                }
                for x in a {
                    walk(x, &item_path(path, x), depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut out = HashMap::new();
    for d in docs {
        let ty = d.get("_type").and_then(Value::as_str).unwrap_or_default();
        walk(d, ty, 0, &mut out);
    }
    out
}

/// Flattens a submission into dotted keys with stringified scalars. Nulls and `_` keys
/// are left out; arrays are kept as JSON text.
pub(super) fn flatten(o: &Map<String, Value>, prefix: &str, out: &mut IndexMap<String, String>) {
    for (k, v) in o {
        if k.starts_with('_') {
            continue;
        }
        let key = format!("{prefix}{k}");
        match v {
            Value::Null => {}
            Value::Object(m) => flatten(m, &format!("{key}."), out),
            Value::String(s) => {
                out.insert(key, s.clone());
            }
            other => {
                out.insert(key, other.to_string());
            }
        }
    }
}
