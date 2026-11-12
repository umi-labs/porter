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

const PLUGIN: &str = " (defined by a plugin? plugin-added collections and fields aren't read)";

fn warning(message: String, details: Value) -> PendingIssue {
    issue(
        IssueKind::SchemaMismatch,
        Severity::Warning,
        message,
        details,
    )
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
                format!("collection `{coll}` (mapped from source type `{ty}`) is not defined in the Payload config{PLUGIN}"),
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
        let stats = model_types.types.get(ty).filter(|s| s.count > 0);
        if stats.is_none() {
            out.push(warning(
                format!("type `{ty}` is mapped but has no normalised documents"),
                json!({"type": ty}),
            ));
        }
        for (src, fm) in tm.fields.iter().filter(|(_, fm)| !fm.drop) {
            if let Some(s) = stats
                && !s.fields.contains_key(src)
            {
                out.push(warning(
                    format!(
                        "`{ty}.{src}` is mapped but appears in none of {} {ty} documents",
                        s.count
                    ),
                    json!({"type": ty, "field": src}),
                ));
            }
            let Some(f) = find(&c.fields, &fm.to) else {
                out.push(error(
                    format!(
                        "field `{coll}.{}` (mapped from `{ty}.{src}`) does not exist{PLUGIN}",
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
        let mapped: Vec<&str> = tm
            .fields
            .values()
            .filter(|fm| !fm.drop)
            .map(|fm| fm.to.as_str())
            .chain(IMPLICIT)
            .chain([key.as_str()])
            .collect();
        let mut gaps = Gaps::default();
        gaps.walk(&c.fields, "", &mapped);
        gaps.report(&mut out, coll, &format!("`{coll}."), "nothing maps to it");
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
        if !seen.insert(slug) {
            continue;
        }
        let Some(fields) = dest.blocks.get(slug) else {
            out.push(error(
                format!("block `{slug}` ({from}) is not defined in the Payload config{PLUGIN}"),
                json!({"block": slug}),
            ));
            continue;
        };
        let mut mapped: Vec<&str> = mapping
            .blocks
            .iter()
            .filter(|b| b.to == slug)
            .flat_map(|b| b.fields.values().map(String::as_str))
            .collect();
        if mapping.raw_block.as_deref() == Some(slug) {
            mapped.push("html");
        }
        let mut gaps = Gaps::default();
        gaps.walk(fields, "", &mapped);
        gaps.report(
            &mut out,
            slug,
            &format!("block `{slug}` field `"),
            "no [[mapping.blocks]] field maps to it",
        );
    }
    out
}

/// Required destination fields nothing writes to, found by walking every group or array
/// the mapping reaches into.
#[derive(Default)]
struct Gaps {
    missing: Vec<String>,
    unknown: Vec<String>,
    /// Required fields inside a group/array mapped as a whole: can't tell from here.
    unverifiable: Vec<String>,
}

impl Gaps {
    fn walk(&mut self, fields: &[DestField], prefix: &str, mapped: &[&str]) {
        for f in fields {
            let path = format!("{prefix}{}", f.name);
            let inside = format!("{path}.");
            let whole = mapped.contains(&path.as_str());
            let into = mapped.iter().any(|p| p.starts_with(&inside));
            if !whole && !into {
                if f.required {
                    self.missing.push(path.clone());
                } else if f.required_unknown {
                    self.unknown.push(path.clone());
                }
            }
            if !matches!(f.kind.as_str(), "group" | "array") {
                continue;
            }
            if into {
                self.walk(&f.fields, &inside, mapped);
            } else if whole {
                required_under(&f.fields, &inside, &mut self.unverifiable);
            }
        }
    }

    fn report(&self, out: &mut Vec<PendingIssue>, owner: &str, lead: &str, why: &str) {
        for p in &self.missing {
            out.push(error(
                format!("{lead}{p}` is required but {why}"),
                json!({"owner": owner, "field": p}),
            ));
        }
        for p in &self.unknown {
            out.push(warning(
                format!("could not determine if {lead}{p}` is required (`required` is an expression), and {why}"),
                json!({"owner": owner, "field": p}),
            ));
        }
        if !self.unverifiable.is_empty() {
            out.push(warning(
                format!(
                    "{owner}: required fields inside fields mapped as a whole can't be verified: {}",
                    self.unverifiable.join(", ")
                ),
                json!({"owner": owner, "fields": self.unverifiable}),
            ));
        }
    }
}

fn required_under(fields: &[DestField], prefix: &str, out: &mut Vec<String>) {
    for f in fields {
        let path = format!("{prefix}{}", f.name);
        if f.required || f.required_unknown {
            out.push(path.clone());
        }
        if matches!(f.kind.as_str(), "group" | "array") {
            required_under(&f.fields, &format!("{path}."), out);
        }
    }
}
