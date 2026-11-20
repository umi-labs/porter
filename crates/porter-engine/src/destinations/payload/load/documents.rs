//! Documents: pass 1 creates (or updates) each with its scalar fields, parents first;
//! pass 2, once every id is known, writes references, rich text, layouts and `parent`.

use super::{Loader, NOT_MAPPED, required_gap, set_path, settle_nulls};
use crate::destinations::payload::blocks::{settle_relations, to_blocks};
use crate::destinations::payload::lexical::{IdMap, to_lexical};
use crate::destinations::payload::schema_check::find;
use crate::destinations::payload::ts::DestField;
use crate::html::PendingIssue;
use crate::mapping::{KeyType, TypeMap, apply, lookup, paths};
use anyhow::Result;
use porter_model::{DocStatus, Document, FieldValue, IssueKind, ItemId, ItemKind, Severity};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

const KIND: ItemKind = ItemKind::Document;

/// One document's body, plus which top-level keys need other items' ids.
struct Built {
    data: Map<String, Value>,
    refs: HashSet<String>,
    issues: Vec<PendingIssue>,
}

pub(super) async fn load(l: &mut Loader<'_>) -> Result<()> {
    let docs: Vec<Document> = l
        .items::<Document>(KIND)?
        .into_iter()
        .map(|(.., d)| d)
        .collect();
    let mapping = l.ctx.config.mapping.clone();
    let mut todo: Vec<(&Document, &TypeMap, &str)> = vec![];
    let mut unmapped: HashMap<&str, usize> = HashMap::new();
    for d in &docs {
        match mapping.types.get(&d.doc_type) {
            Some(tm) => match &tm.to {
                Some(_) if tm.status.get(&wire_status(d)).is_some_and(|s| s == "skip") => {
                    let why = format!("status `{}` skipped in the mapping", wire_status(d));
                    l.skip(d.id, KIND, &why)?
                }
                Some(to) => todo.push((d, tm, to)),
                None => l.skip(
                    d.id,
                    KIND,
                    tm.drop_reason
                        .as_deref()
                        .unwrap_or("dropped in the mapping"),
                )?,
            },
            None => {
                *unmapped.entry(&d.doc_type).or_default() += 1;
                l.skip(d.id, KIND, NOT_MAPPED)?;
            }
        }
    }
    // Payload drops fields it doesn't have without a word: say so once per field.
    for (ty, tm) in &mapping.types {
        let Some(c) = tm.to.as_deref().and_then(|to| l.coll(to).map(|c| (to, c))) else {
            continue;
        };
        for fm in tm.fields.values().filter(|fm| !fm.drop) {
            if find(&c.1.fields, &fm.to).is_none() {
                l.ctx.issue(
                    IssueKind::SchemaMismatch,
                    Severity::Warning,
                    None,
                    format!("`{ty}` maps to `{}.{}`, which the Payload config doesn't define: Payload will drop it", c.0, fm.to),
                    json!({"type": ty, "collection": c.0, "field": fm.to}),
                )?;
            }
        }
    }
    for (ty, n) in unmapped {
        l.ctx.issue(
            IssueKind::UnmappedField,
            Severity::Warning,
            None,
            format!("source type `{ty}` has no mapping: {n} documents not loaded"),
            json!({"type": ty, "count": n}),
        )?;
    }

    // New paths (parents first, so `{parent.path}` resolves) and old URLs.
    for d in &docs {
        if let Some(u) = &d.source.url {
            l.ids.urls.insert(d.id, u.clone());
        }
    }
    let depth = paths::depths(&docs);
    // Sort by tree depth (topological: a parent is always shallower), then path depth.
    todo.sort_by_key(|(d, ..)| (depth[&d.id], d.path.matches('/').count()));
    for (id, r) in paths::render_all(&docs, &mapping) {
        match r {
            Ok(p) => {
                l.ids.paths.insert(id, p.clone());
                if let Some(r) = l.ids.refs.get_mut(&id) {
                    r.path = Some(p);
                }
            }
            Err(e) => {
                let pattern = mapping
                    .types
                    .get(&by_type(&docs, id))
                    .and_then(|tm| tm.path.clone());
                l.ctx.issue(
                    IssueKind::SchemaMismatch,
                    Severity::Warning,
                    Some(id),
                    format!("no new URL path: {e}"),
                    json!({"pattern": pattern}),
                )?;
            }
        }
    }

    fail_clashes(l, &mut todo)?;

    // Pass 1.
    let total = todo.len() as u64;
    let mut created: Vec<(&Document, &TypeMap, &str, Value)> = vec![];
    for (i, (d, tm, coll)) in todo.iter().enumerate() {
        if l.cancelled() {
            return Ok(());
        }
        l.ctx
            .progress(i as u64 + 1, Some(total), format!("create {coll}"));
        let key = match key_of(tm, d) {
            Ok(k) => k,
            Err(e) => {
                schema_fail(l, d.id, e)?;
                continue;
            }
        };
        let draft = is_draft(d, tm);
        let full = build(l, d, tm, coll);
        if !draft
            && let Some(fields) = l.coll(coll).map(|c| &c.fields)
            && let Some(gap) = required_gap(fields, &full.data, "")
        {
            // Empty only because its target is created later in this pass?
            let top = gap.split('.').next().unwrap_or(&gap);
            let later: HashSet<String> = todo[i + 1..]
                .iter()
                .map(|(d, ..)| d.id.to_string())
                .collect();
            let forward = full.refs.contains(top).then(|| {
                full.issues.iter().find_map(|x| {
                    let t = x.details["target"].as_str()?;
                    later.contains(t).then(|| t.to_string())
                })
            });
            let msg = match forward.flatten() {
                Some(t) => format!(
                    "required field `{coll}.{gap}` is empty: unresolved forward reference to {t} (re-run will fix)"
                ),
                None => format!("required field `{coll}.{gap}` is empty after mapping"),
            };
            schema_fail(l, d.id, msg)?;
            continue;
        }
        let required = |k: &str| {
            l.coll(coll)
                .is_some_and(|c| c.fields.iter().any(|f| f.name == k && f.required))
        };
        let mut first: Map<String, Value> = full
            .data
            .into_iter()
            .filter(|(k, _)| !full.refs.contains(k) || required(k))
            .collect();
        settle_nulls(l.coll(coll).map(|c| &c.fields[..]), &mut first);
        if !l.apply() {
            let id = l.placeholder(d.id, coll);
            created.push((d, tm, coll, id));
            continue;
        }
        match l
            .upsert(d.id, coll, &tm.key_field, &key, Value::Object(first), draft)
            .await?
        {
            Ok(id) => created.push((d, tm, coll, id)),
            Err(e) => l.fail(d.id, KIND, coll, e)?,
        }
    }

    // Pass 2.
    let total = created.len() as u64;
    for (i, (d, tm, coll, id)) in created.into_iter().enumerate() {
        if l.cancelled() {
            return Ok(());
        }
        l.ctx
            .progress(i as u64 + 1, Some(total), format!("fill {coll}"));
        let draft = is_draft(d, tm);
        let Built {
            mut data, issues, ..
        } = build(l, d, tm, coll);
        settle_nulls(l.coll(coll).map(|c| &c.fields[..]), &mut data);
        // A parent removed at the source must be cleared on re-run. nested-docs adds
        // `parent` as a plugin field the schema reader can't see, so send it whatever the
        // schema says: null for an optional relation is safe, and Payload drops unknown keys.
        if d.parent.is_none() {
            data.insert("parent".into(), Value::Null);
        }
        let partial = l.emit(d.id, issues)?;
        let body = Value::Object(data);
        let Some(client) = l.client.clone() else {
            let mut body = body;
            body[&tm.key_field] = key_of(tm, d).unwrap_or_default();
            l.preview(
                coll,
                d.id,
                &crate::destinations::payload::client::prepare(&body, draft),
            )?;
            l.finish(d.id, KIND, partial)?;
            continue;
        };
        match client.update(coll, &id, &body, draft).await {
            Ok(_) => l.finish(d.id, KIND, partial)?,
            Err(e) => l.fail(d.id, KIND, coll, format!("{e:#}"))?,
        }
    }
    Ok(())
}

fn by_type(docs: &[Document], id: ItemId) -> String {
    docs.iter()
        .find(|d| d.id == id)
        .map(|d| d.doc_type.clone())
        .unwrap_or_default()
}

fn schema_fail(l: &Loader, id: ItemId, error: String) -> Result<()> {
    l.ctx.issue(
        IssueKind::SchemaMismatch,
        Severity::Error,
        Some(id),
        error.clone(),
        json!({}),
    )?;
    l.status(id, KIND, porter_model::ItemStatus::Failed { error })
}

/// Published writes the live doc; anything else maps through `status` (default draft).
fn is_draft(d: &Document, tm: &TypeMap) -> bool {
    if d.status == DocStatus::Published {
        return false;
    }
    tm.status
        .get(&wire_status(d))
        .is_none_or(|s| s != "published")
}

/// The status as `[mapping.types.*.status]` keys name it (`draft`, `private`, ...).
fn wire_status(d: &Document) -> String {
    serde_json::to_value(d.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// The upsert key: the item id, or for `key_type = "number"` the source id's number. Only
/// an id whose `:`-segments hold exactly one number, as the last segment (`123`,
/// `posts:123`), has one: variant (`node:5:v1`), dedupe (`5~2`) and positional (`item-3`)
/// ids would share a number with another document and overwrite it.
fn key_of(tm: &TypeMap, d: &Document) -> Result<Value, String> {
    match tm.key_type {
        KeyType::Text => Ok(json!(d.id.to_string())),
        KeyType::Number => {
            let sid = &d.source.source_id;
            let segs: Vec<&str> = sid.split(':').collect();
            let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            let (last, rest) = segs.split_last().unwrap_or((&"", &[]));
            if numeric(last)
                && rest.iter().all(|s| !s.bytes().any(|b| b.is_ascii_digit()))
                && let Ok(n) = last.parse::<u64>()
            {
                return Ok(json!(n));
            }
            Err(format!(
                "source id `{sid}` has no unique numeric key; use key_type = text"
            ))
        }
    }
}

/// Drops from `todo` every document whose `(collection, key)` another one shares, failing
/// each with a schema-mismatch naming them all: an upsert on a shared key would overwrite
/// one document with another. Text keys are item ids, so only numeric keys can clash.
fn fail_clashes(l: &Loader, todo: &mut Vec<(&Document, &TypeMap, &str)>) -> Result<()> {
    let mut by_key: HashMap<(String, String), Vec<&str>> = HashMap::new();
    for (d, tm, coll) in todo.iter() {
        if let Ok(k) = key_of(tm, d) {
            by_key
                .entry((coll.to_string(), k.to_string()))
                .or_default()
                .push(&d.source.source_id);
        }
    }
    let clash = |d: &Document, tm: &TypeMap, coll: &str| {
        let k = key_of(tm, d).ok()?;
        let sids = by_key.get(&(coll.to_string(), k.to_string()))?;
        (sids.len() > 1).then(|| (k, sids.join(", ")))
    };
    let mut keep = vec![];
    for (d, tm, coll) in todo.drain(..) {
        match clash(d, tm, coll) {
            Some((k, sids)) => schema_fail(
                l,
                d.id,
                format!(
                    "key {k} in {coll} is shared by source ids {sids}: none of them written; use key_type = text"
                ),
            )?,
            None => keep.push((d, tm, coll)),
        }
    }
    *todo = keep;
    Ok(())
}

fn has_refs(v: &FieldValue) -> bool {
    match v {
        FieldValue::RichText(_)
        | FieldValue::Layout(_)
        | FieldValue::Media(_)
        | FieldValue::Doc(_) => true,
        FieldValue::List(items) => items.iter().any(has_refs),
        FieldValue::Object(m) => m.values().any(has_refs),
        _ => false,
    }
}

fn build(l: &Loader, d: &Document, tm: &TypeMap, coll: &str) -> Built {
    let fields = l.coll(coll).map(|c| &c.fields[..]);
    let mut b = Built {
        data: Map::new(),
        refs: HashSet::new(),
        issues: vec![],
    };
    b.data.insert("title".into(), json!(d.title));
    b.data.insert("slug".into(), json!(d.slug));
    // The new URL path (the mapping's pattern, else the source's), for the generated
    // `path` field; sent unless the read schema shows the collection has none.
    let path = l.ids.paths.get(&d.id).unwrap_or(&d.path);
    if !path.is_empty() && fields.is_none_or(|fs| fs.iter().any(|f| f.name == "path")) {
        b.data.insert("path".into(), json!(path));
    }
    for (src, fm) in tm.fields.iter().filter(|(_, fm)| !fm.drop) {
        let v = match apply(&lookup(d, src), &fm.transform) {
            Ok(v) => v,
            Err(e) => {
                b.issues.push(PendingIssue {
                    kind: IssueKind::SchemaMismatch,
                    severity: Severity::Warning,
                    message: format!("`{}.{src}` not written: {e}", d.doc_type),
                    details: json!({"field": src, "to": fm.to}),
                });
                FieldValue::Null
            }
        };
        if has_refs(&v) {
            let top = fm.to.split('.').next().unwrap_or(&fm.to);
            b.refs.insert(top.to_string());
        }
        let f = fields.and_then(|fs| find(fs, &fm.to));
        let j = value(l, &v, f, &mut b.issues);
        set_path(&mut b.data, &fm.to, j);
    }
    // nested-docs' `parent`: plugin-added, so usually not in the read schema.
    let pf = fields.and_then(|fs| find(fs, "parent"));
    // No parent: pass 2 sends `parent: null` (see there).
    if let Some(p) = d.parent {
        b.refs.insert("parent".into());
        // nested-docs parents are in the item's own collection; another collection's id
        // would point at an unrelated doc, so the parent is left unset instead.
        let miss = match l.ids.get(p) {
            Some(r) if r.collection == coll => None,
            Some(r) => Some(format!(
                "parent {p} was loaded into {}, not {coll}",
                r.collection
            )),
            None => Some(format!("parent {p} is not loaded")),
        };
        match miss {
            None => {
                let v = l.relation(p, pf, IssueKind::ParentMissing, &mut b.issues);
                b.data.insert("parent".into(), v);
            }
            Some(message) => b.issues.push(PendingIssue {
                kind: IssueKind::ParentMissing,
                severity: Severity::Warning,
                message,
                details: json!({"parent": p, "old_url": l.ids.old_url(p)}),
            }),
        }
    }
    b
}

/// A neutral value as Payload JSON for field `f` (when the schema knows it).
fn value(l: &Loader, v: &FieldValue, f: Option<&DestField>, out: &mut Vec<PendingIssue>) -> Value {
    let raw = l.ctx.config.mapping.raw_block.as_deref();
    match v {
        FieldValue::Null => Value::Null,
        FieldValue::Text(s) => json!(s),
        FieldValue::Number(n) => json!(n),
        FieldValue::Bool(b) => json!(b),
        FieldValue::Date(d) => json!(d.to_rfc3339()),
        // Rich text into a blocks field (a page without a builder layout): one rich-text
        // block, which `[[mapping.blocks]]` maps like any other.
        FieldValue::RichText(_) if f.is_some_and(|f| f.kind == "blocks") => {
            let block = porter_model::Block {
                kind: porter_model::BlockKind::RichText,
                props: [("text".to_string(), v.clone())].into_iter().collect(),
                children: vec![],
                original: None,
            };
            value(l, &FieldValue::Layout(vec![block]), f, out)
        }
        FieldValue::RichText(rt) => {
            let (j, issues) = to_lexical(rt, &l.ids, raw);
            out.extend(issues);
            j
        }
        FieldValue::Layout(blocks) => {
            let (arr, issues) = to_blocks(blocks, &l.ctx.config.mapping.blocks, &l.ids, raw);
            out.extend(issues);
            let mut v = Value::Array(arr);
            out.extend(settle_relations(
                &mut v,
                l.schema.as_ref().map(|d| &d.blocks),
            ));
            v
        }
        FieldValue::Media(id) => l.relation(*id, f, IssueKind::MissingMedia, out),
        FieldValue::Doc(id) => l.relation(*id, f, IssueKind::UnresolvedRelation, out),
        // Array rows and hasMany items both take `f`: rows read its sub-fields.
        FieldValue::List(items) => {
            Value::Array(items.iter().map(|i| value(l, i, f, out)).collect())
        }
        FieldValue::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let sub = f.and_then(|f| f.fields.iter().find(|x| &x.name == k));
                    (k.clone(), value(l, v, sub, out))
                })
                .collect(),
        ),
    }
}
