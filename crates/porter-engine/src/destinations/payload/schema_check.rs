//! `porter schema check`: does the mapping fit the client's existing Payload config?

use super::ts::{DestField, DestSchema};
use crate::html::PendingIssue;
use crate::mapping::summary::TypeSummary;
use crate::mapping::{KeyType, MappingConfig, Transform};
use porter_model::{IssueKind, Severity};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Destination fields every load writes from the document itself, besides the key field.
const IMPLICIT: [&str; 2] = ["title", "slug"];

/// Whether a value of this `FieldValue` variant can be written to `f`.
pub fn compatible(variant: &str, f: &DestField) -> bool {
    let k = f.kind.as_str();
    k == "json"
        || match variant {
            "Text" => matches!(k, "text" | "textarea" | "email" | "select" | "code"),
            "RichText" => k == "richText",
            "Layout" => k == "blocks",
            "Media" => k == "upload",
            "Doc" => k == "relationship",
            "List" => k == "array" || f.many,
            "Date" => k == "date",
            "Bool" => k == "checkbox",
            "Number" => k == "number",
            "Object" => k == "group",
            _ => false,
        }
}

/// The Payload kind a variant needs, for messages.
pub fn needs(variant: &str) -> &'static str {
    match variant {
        "Text" => "text/textarea/email/select/code",
        "RichText" => "richText",
        "Layout" => "blocks",
        "Media" => "upload",
        "Doc" => "relationship",
        "List" => "array or hasMany",
        "Date" => "date",
        "Bool" => "checkbox",
        "Number" => "number",
        "Object" => "group",
        _ => "json",
    }
}

/// The variants a field holds after its transforms run.
pub fn effective(variants: &BTreeSet<String>, ts: &[Transform]) -> BTreeSet<String> {
    ts.iter().fold(variants.clone(), |acc, t| {
        let to = match t {
            Transform::Number => "Number",
            Transform::DateParse { .. } => "Date",
            Transform::Split { .. } => "List",
            Transform::Default { .. } => return acc,
            _ => "Text",
        };
        [to.to_string()].into()
    })
}

/// A dotted destination path through group/array fields.
pub fn find<'a>(fields: &'a [DestField], path: &str) -> Option<&'a DestField> {
    let mut cur = fields;
    let mut found = None;
    for seg in path.split('.') {
        let f = cur.iter().find(|f| f.name == seg)?;
        cur = &f.fields;
        found = Some(f);
    }
    found
}

fn issue(kind: IssueKind, severity: Severity, message: String, details: Value) -> PendingIssue {
    PendingIssue {
        kind,
        severity,
        message,
        details,
    }
}

fn error(message: String, details: Value) -> PendingIssue {
    issue(IssueKind::SchemaMismatch, Severity::Error, message, details)
}

pub fn check(
    model_types: &TypeSummary,
    mapping: &MappingConfig,
    dest: &DestSchema,
) -> Vec<PendingIssue> {
    let mut out: Vec<PendingIssue> = dest
        .warnings
        .iter()
        .map(|w| {
            issue(
                IssueKind::SchemaMismatch,
                Severity::Warning,
                w.clone(),
                json!({}),
            )
        })
        .collect();
    for (ty, tm) in &mapping.types {
        let Some(coll) = &tm.to else { continue };
        let Some(c) = dest.collections.get(coll) else {
            out.push(error(
                format!("collection `{coll}` (mapped from source type `{ty}`) is not defined in the Payload config"),
                json!({"type": ty, "collection": coll}),
            ));
            continue;
        };
        let want = match tm.key_type {
            KeyType::Text => "text",
            KeyType::Number => "number",
        };
        let key = &tm.key_field;
        match c.fields.iter().find(|f| &f.name == key) {
            None => out.push(error(
                format!("collection `{coll}` has no `{key}` field: the upsert key, add it as a {want} field with `index: true, unique: true`"),
                json!({"collection": coll, "field": key, "key_type": want}),
            )),
            Some(f) if f.kind != want => out.push(error(
                format!("`{coll}.{key}` is {} but key_type is {want}", f.kind),
                json!({"collection": coll, "field": key, "kind": f.kind, "key_type": want}),
            )),
            Some(_) => {}
        }
        let stats = model_types.types.get(ty);
        for (src, fm) in &tm.fields {
            let Some(f) = find(&c.fields, &fm.to) else {
                out.push(error(
                    format!(
                        "field `{coll}.{}` (mapped from `{ty}.{src}`) does not exist",
                        fm.to
                    ),
                    json!({"collection": coll, "field": fm.to, "source": src, "type": ty}),
                ));
                continue;
            };
            let Some(fs) = stats.and_then(|s| s.fields.get(src)) else {
                continue;
            };
            for v in effective(&fs.variants, &fm.transform) {
                if !compatible(&v, f) {
                    out.push(error(
                        format!(
                            "field `{coll}.{}` is {} but `{ty}.{src}` holds {v} (needs {})",
                            fm.to,
                            f.kind,
                            needs(&v)
                        ),
                        json!({"collection": coll, "field": fm.to, "kind": f.kind, "source": src, "variant": v}),
                    ));
                }
            }
        }
        // ponytail: top-level required fields only; nested required ones surface at load.
        for f in c.fields.iter().filter(|f| f.required) {
            let covered = IMPLICIT.contains(&f.name.as_str())
                || &f.name == key
                || tm
                    .fields
                    .values()
                    .any(|fm| fm.to.split('.').next() == Some(f.name.as_str()));
            if !covered {
                out.push(error(
                    format!("`{coll}.{}` is required but nothing maps to it", f.name),
                    json!({"collection": coll, "field": f.name}),
                ));
            }
        }
        if let Some(s) = stats {
            for (k, fs) in &s.fields {
                if !tm.fields.contains_key(k) {
                    out.push(issue(
                        IssueKind::UnmappedField,
                        Severity::Warning,
                        format!(
                            "`{ty}.{k}` has no mapping (filled in {:.0}% of {ty})",
                            fs.fill_rate * 100.0
                        ),
                        json!({"type": ty, "field": k, "fill_rate": fs.fill_rate}),
                    ));
                }
            }
        }
    }
    for ty in model_types.types.keys() {
        if !mapping.types.contains_key(ty) {
            out.push(issue(
                IssueKind::UnmappedField,
                Severity::Warning,
                format!("source type `{ty}` has no mapping"),
                json!({"type": ty}),
            ));
        }
    }
    let slugs = mapping
        .blocks
        .iter()
        .map(|b| (b.to.as_str(), "mapping.blocks"))
        .chain(
            mapping
                .raw_block
                .as_deref()
                .map(|r| (r, "mapping.raw_block")),
        );
    let mut seen = BTreeSet::new();
    for (slug, from) in slugs {
        if seen.insert(slug) && !dest.blocks.contains_key(slug) {
            out.push(error(
                format!("block `{slug}` ({from}) is not defined in the Payload config"),
                json!({"block": slug}),
            ));
        }
    }
    out
}
