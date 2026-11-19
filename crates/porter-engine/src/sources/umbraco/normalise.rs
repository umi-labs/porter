//! Umbraco normalise: nodes → `Document`s (connector `umbraco`). Pass A writes document
//! skeletons, routes and media details so links and media resolve; pass B types the property
//! values and writes the final documents.

use super::{
    CANON, Entry, MAX_DEPTH, MediaRef, absolute, alias_of, culture_of, entries, exports, hosts_of,
    id_str, looks_html, looks_media, media_ref, raw_ref, read_file, text, url_of, variants,
    walk_media,
};
use crate::html::{HtmlCtx, PendingIssue, html_to_richtext, no_alternates};
use crate::mapping::transforms::slugify;
use crate::stage::StageCtx;
use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, NaiveDateTime, Utc};
use indexmap::IndexMap;
use porter_model::{
    DocStatus, Document, FieldValue, IssueKind, ItemId, ItemKind, ItemStatus, Media, Route,
    RouteTarget, Seo, Severity, SourceRef,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Converters recurse deeply on nested HTML.
const STACK: usize = 16 * 1024 * 1024;

/// Keys that describe the node rather than hold content (flat exports mix both).
const STRUCT: &[&str] = &[
    "id",
    "key",
    "udi",
    "name",
    "contentTypeAlias",
    "contentType",
    "url",
    "urlSegment",
    "slug",
    "route",
    "path",
    "createDate",
    "updateDate",
    "created",
    "updated",
    "published",
    "status",
    "state",
    "trashed",
    "parentId",
    "parent",
    "variants",
    "_variants",
    "culture",
    "language",
    "cultureCode",
    "locale",
    "segment",
    "level",
    "sortOrder",
    "templateId",
    "template",
    "properties",
    "cultures",
];

pub fn run(ctx: &StageCtx) -> Result<()> {
    std::thread::scope(|s| {
        let h = std::thread::Builder::new()
            .name("umbraco-normalise".into())
            .stack_size(STACK)
            .spawn_scoped(s, || normalise(ctx))?;
        h.join().map_err(|e| {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            anyhow!("normalise panicked: {msg}")
        })?
    })
}

fn canon(sid: &str, url: Option<String>) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: CANON.into(),
        source_id: sid.into(),
        url,
    };
    (ItemId::new(ItemKind::Document, &src), src)
}

/// One document to write: a node, or one variant of it.
struct Plan<'a> {
    sid: String,
    id: ItemId,
    entry: &'a Entry,
    node: &'a Map<String, Value>,
    variant: Option<&'a Map<String, Value>>,
    culture: Option<String>,
    primary: bool,
    issues: Vec<PendingIssue>,
}

impl Plan<'_> {
    /// Variant first, then the node.
    fn maps(&self) -> Vec<&Map<String, Value>> {
        self.variant.into_iter().chain([self.node]).collect()
    }

    fn text(&self, keys: &[&str]) -> Option<&str> {
        self.maps().into_iter().find_map(|m| text(m, keys))
    }

    fn url(&self) -> Option<String> {
        self.maps().into_iter().find_map(url_of)
    }
}

/// Content properties of a node or variant: its `properties` object, else its non-structural keys.
fn props_of(m: &Map<String, Value>) -> Vec<(&String, &Value)> {
    match m.get("properties").and_then(Value::as_object) {
        Some(p) => p.iter().collect(),
        None => m
            .iter()
            .filter(|(k, _)| !STRUCT.contains(&k.as_str()))
            .collect(),
    }
}

fn structural(m: &Map<String, Value>) -> Value {
    Value::Object(
        m.iter()
            .filter(|(k, _)| {
                STRUCT.contains(&k.as_str())
                    && !["variants", "_variants", "properties"].contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

fn doc_status(maps: &[&Map<String, Value>]) -> DocStatus {
    for m in maps {
        if let Some(b) = m.get("published").and_then(Value::as_bool) {
            return if b {
                DocStatus::Published
            } else {
                DocStatus::Draft
            };
        }
        if let Some(s) = text(m, &["status", "state"]) {
            let s = s.to_lowercase();
            return if s.contains("unpublish") || s.starts_with("draft") || s == "notcreated" {
                DocStatus::Draft
            } else if s.starts_with("schedul") {
                DocStatus::Scheduled
            } else if s.starts_with("private") {
                DocStatus::Private
            } else {
                DocStatus::Published
            };
        }
    }
    DocStatus::Published
}

fn date(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?;
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"]
                .iter()
                .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
                .map(|n| n.and_utc())
        })
}

/// The path part of a URL, with a leading `/`.
fn path_of(u: &str) -> String {
    let p = match url::Url::parse(u) {
        Ok(u) => u.path().to_string(),
        Err(_) => u.split(['?', '#']).next().unwrap_or(u).to_string(),
    };
    if p.starts_with('/') {
        p
    } else {
        format!("/{p}")
    }
}

fn plans<'a>(entries: &'a [Entry], used: &mut HashSet<String>) -> Vec<Plan<'a>> {
    let mut out = vec![];
    for e in entries {
        let Ok(node) = &e.node else { continue };
        if node.get("trashed").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let vs = variants(node);
        let make = |i: usize, v: Option<&'a Map<String, Value>>, used: &mut HashSet<String>| {
            let culture = match v {
                Some(v) => culture_of(v),
                None => text(node, &["culture", "language", "cultureCode", "locale"])
                    .map(str::to_string),
            };
            let mut sid = match (i, &culture) {
                (0, _) => format!("node:{}", e.sid),
                (_, Some(c)) => format!("node:{}:{c}", e.sid),
                (_, None) => format!("node:{}:v{i}", e.sid),
            };
            if !used.insert(sid.clone()) {
                sid = format!("{sid}~{i}");
                used.insert(sid.clone());
            }
            Plan {
                id: canon(&sid, None).0,
                sid,
                entry: e,
                node,
                variant: v,
                culture,
                primary: i == 0,
                issues: vec![],
            }
        };
        if vs.is_empty() {
            out.push(make(0, None, used));
        } else {
            for (i, v) in vs.into_iter().enumerate() {
                out.push(make(i, Some(v), used));
            }
        }
    }
    out
}

fn normalise(ctx: &StageCtx) -> Result<()> {
    let store = ctx.project.store();
    let dir = ctx.project.raw_dir(CANON);
    let files = if dir.exists() { exports(&dir)? } else { vec![] };
    if files.is_empty() {
        bail!("no exported files in {}; run extract first", dir.display());
    }
    let entries = entries(files.iter().map(|f| read_file(f)).collect::<Result<_>>()?);
    let hosts = hosts_of(&entries);
    let mut written: HashSet<ItemId> = HashSet::new();
    let mut prov: BTreeMap<String, Vec<ItemId>> = BTreeMap::new();

    // Entries that are not documents get their own status.
    for e in &entries {
        let (rid, _) = raw_ref(&e.sid, None);
        match &e.node {
            Err(error) => ctx.item_status(
                rid,
                ItemKind::Document,
                ItemStatus::Failed {
                    error: error.clone(),
                },
            )?,
            Ok(m) if m.get("trashed").and_then(Value::as_bool) == Some(true) => ctx.item_status(
                rid,
                ItemKind::Document,
                ItemStatus::Skipped {
                    reason: "in the recycle bin".into(),
                },
            )?,
            Ok(_) => {}
        }
    }

    let mut used = HashSet::new();
    let mut plans = plans(&entries, &mut used);
    let primary: HashMap<&str, ItemId> = plans
        .iter()
        .filter(|p| p.primary)
        .map(|p| (p.entry.sid.as_str(), p.id))
        .collect();
    let group: HashMap<&str, Vec<ItemId>> = plans.iter().fold(HashMap::new(), |mut m, p| {
        m.entry(p.entry.sid.as_str()).or_default().push(p.id);
        m
    });
    let parent_of: HashMap<&str, String> = plans
        .iter()
        .filter(|p| p.primary)
        .filter_map(|p| Some((p.entry.sid.as_str(), id_str(p.node.get("parentId"))?)))
        .collect();
    let in_cycle = |sid: &str| {
        let mut cur = sid;
        for _ in 0..=parent_of.len() {
            match parent_of.get(cur) {
                Some(p) if p == sid => return true,
                Some(p) => cur = p,
                None => return false,
            }
        }
        false
    };

    // Pass A: skeletons, routes, media details.
    let mut skeletons: Vec<(Document, usize)> = vec![];
    for (n, p) in plans.iter_mut().enumerate() {
        let status = doc_status(&p.maps());
        let title = p.text(&["name"]).map(str::to_string).unwrap_or_default();
        let url = p.url();
        let url_path = url.as_deref().map(path_of);
        let slug = p
            .text(&["urlSegment", "slug"])
            .map(str::to_string)
            .or_else(|| {
                url_path
                    .as_deref()
                    .and_then(|u| u.rsplit('/').find(|s| !s.is_empty()))
                    .map(str::to_string)
            })
            .or_else(|| slugify(&title).ok())
            .unwrap_or_else(|| p.entry.sid.clone());
        let mut path = url_path.clone().unwrap_or_else(|| format!("/{slug}/"));
        // Two translations can't share a path.
        if !p.primary && p.variant.and_then(url_of).is_none() && url_path.is_some() {
            path = format!(
                "/{}{path}",
                p.culture.clone().unwrap_or_else(|| p.sid.clone())
            );
        }
        let parent = match id_str(p.node.get("parentId")) {
            Some(pid) if pid.parse::<i64>().is_ok_and(|n| n <= 0) => None,
            Some(pid) if pid == p.entry.sid => None,
            Some(pid) => match primary.get(pid.as_str()) {
                Some(id) if !in_cycle(&p.entry.sid) => Some(*id),
                found => {
                    let why = if found.is_some() {
                        format!("parent chain through node {pid} loops back to this node")
                    } else {
                        format!("parent node {pid} is not in the export")
                    };
                    p.issues.push(PendingIssue {
                        kind: IssueKind::ParentMissing,
                        severity: Severity::Warning,
                        message: why,
                        details: json!({ "parent": pid }),
                    });
                    None
                }
            },
            None => None,
        };
        let abs = url.as_deref().and_then(|u| absolute(&hosts, u));
        let (id, src) = canon(&p.sid, abs.clone());
        let d = Document {
            id,
            doc_type: alias_of(p.node),
            status,
            locale: p.culture.clone(),
            translations: group[p.entry.sid.as_str()]
                .iter()
                .filter(|g| **g != id)
                .copied()
                .collect(),
            slug,
            path,
            parent,
            title: if title.is_empty() {
                p.entry.sid.clone()
            } else {
                title
            },
            fields: IndexMap::new(),
            seo: Seo::default(),
            created: date(p.text(&["createDate", "created"])),
            updated: date(p.text(&["updateDate", "updated"])),
            author: None,
            original: json!({
                "node": structural(p.node),
                "variant": p.variant.map(structural),
            }),
            source: src.clone(),
        };
        store.put_item(ItemKind::Document, id, &src, &d)?;
        written.insert(id);
        if let (Some(u), DocStatus::Published) = (&abs, status) {
            let rsrc = SourceRef {
                connector: CANON.into(),
                source_id: format!("route:{}", p.sid),
                url: Some(u.clone()),
            };
            let rid = ItemId::new(ItemKind::Route, &rsrc);
            let route = Route {
                url: u.clone(),
                target: RouteTarget::Document(id),
                http_status: None,
                title: Some(d.title.clone()),
            };
            store.put_item(ItemKind::Route, rid, &rsrc, &route)?;
            written.insert(rid);
        }
        skeletons.push((d, n));
    }
    for e in entries
        .iter()
        .filter_map(|e| Some((e, e.node.as_ref().ok()?)))
    {
        let mut refs = vec![];
        e.1.values().for_each(|v| walk_media(v, &mut refs, 0));
        for r in refs {
            describe_media(ctx, &hosts, &r)?;
        }
    }
    // Before content is converted, so links can't resolve to what is gone.
    drop_stale(ctx, &written)?;
    if ctx.cancel.is_cancelled() {
        return Ok(());
    }

    // Pass B: property values.
    let total = skeletons.len() as u64;
    let mut finished = vec![];
    for (i, (mut d, n)) in skeletons.into_iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        if i % 200 == 0 {
            ctx.progress(i as u64, Some(total), "documents");
        }
        let p = &mut plans[n];
        let mut issues = std::mem::take(&mut p.issues);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut local = vec![];
            let mut hc = HtmlCtx {
                store,
                site_hosts: if hosts.is_empty() {
                    vec!["umbraco.invalid".into()]
                } else {
                    hosts.clone()
                },
                issues: &mut local,
                media_alternates: no_alternates,
            };
            let node_props = props_of(p.node);
            let variant_props = p.variant.map(props_of).unwrap_or_default();
            for (k, v) in node_props.into_iter().chain(variant_props) {
                d.fields.insert(k.clone(), conv(v, &mut hc, 0));
            }
            local
        }));
        let st = match outcome {
            Ok(local) => {
                issues.extend(local);
                ItemStatus::Done
            }
            Err(e) => {
                let error = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "converter panicked".into());
                issues.push(PendingIssue {
                    kind: IssueKind::UnsupportedBlock,
                    severity: Severity::Error,
                    message: format!("content could not be converted: {error}"),
                    details: json!({ "node": p.entry.sid }),
                });
                ItemStatus::Failed { error }
            }
        };
        finished.push((d, issues, st, n));
    }
    // Written after converting: a write resets the URL index lookups use.
    for (d, issues, st, n) in finished {
        store.put_item(ItemKind::Document, d.id, &d.source, &d)?;
        for i in issues {
            ctx.issue(i.kind, i.severity, Some(d.id), i.message, i.details)?;
        }
        ctx.item_status(d.id, ItemKind::Document, st)?;
        prov.entry(plans[n].entry.sid.clone())
            .or_default()
            .push(d.id);
    }
    let pdir = ctx.project.raw_dir("normalise");
    std::fs::create_dir_all(&pdir)?;
    std::fs::write(
        pdir.join("provenance.json"),
        serde_json::to_vec_pretty(&prov)?,
    )?;
    ctx.progress(total, Some(total), "documents");
    Ok(())
}

/// Fills a stored file's title, alt text and size from the picker object that named it.
fn describe_media(ctx: &StageCtx, hosts: &[String], r: &MediaRef) -> Result<()> {
    let store = ctx.project.store();
    let Some(id) = absolute(hosts, &r.url).and_then(|u| store.media_by_url(&u).ok().flatten())
    else {
        return Ok(());
    };
    let Some(mut m) = store.get_item::<Media>(id)? else {
        return Ok(());
    };
    let before = m.clone();
    m.title = m.title.or_else(|| r.name.clone());
    m.alt = m.alt.or_else(|| r.alt.clone());
    m.width = m.width.or(r.width);
    m.height = m.height.or(r.height);
    if m != before {
        let src = SourceRef {
            connector: "media".into(),
            source_id: m.hash.clone(),
            url: m.variants.first().cloned(),
        };
        store.put_item(ItemKind::Media, id, &src, &m)?;
    }
    Ok(())
}

/// Soft-deletes documents and routes an earlier run made and this one didn't.
fn drop_stale(ctx: &StageCtx, written: &HashSet<ItemId>) -> Result<()> {
    let store = ctx.project.store();
    for (id, kind, connector) in store.item_rows()? {
        let kind: ItemKind = serde_json::from_value(json!(kind))?;
        if connector != CANON
            || !matches!(kind, ItemKind::Document | ItemKind::Route)
            || written.contains(&id)
        {
            continue;
        }
        store.soft_delete(id)?;
        let reason = "no longer in the source".to_string();
        ctx.item_status(id, kind, ItemStatus::Skipped { reason })?;
    }
    Ok(())
}

fn media(url: &str, hc: &mut HtmlCtx) -> FieldValue {
    match hc.media_or_issue(url, None) {
        Some(id) => FieldValue::Media(id),
        None => FieldValue::Text(url.trim().to_string()),
    }
}

/// A property value typed by its JSON shape.
fn conv(v: &Value, hc: &mut HtmlCtx, depth: usize) -> FieldValue {
    if depth > MAX_DEPTH {
        hc.issue(
            IssueKind::UnmappedField,
            Severity::Info,
            format!("value nested deeper than {MAX_DEPTH}; kept as JSON text"),
            json!({}),
        );
        return FieldValue::Text(v.to_string());
    }
    match v {
        Value::Null => FieldValue::Null,
        Value::Bool(b) => FieldValue::Bool(*b),
        Value::Number(n) => n
            .as_f64()
            .map(FieldValue::Number)
            .unwrap_or_else(|| FieldValue::Text(n.to_string())),
        Value::String(s) if looks_media(s) => media(s, hc),
        Value::String(s) if looks_html(s) => FieldValue::RichText(html_to_richtext(s, hc)),
        Value::String(s) => FieldValue::Text(s.clone()),
        Value::Array(a) => FieldValue::List(a.iter().map(|x| conv(x, hc, depth + 1)).collect()),
        Value::Object(m) => match media_ref(m) {
            Some(r) => media(&r.url, hc),
            None => FieldValue::Object(
                m.iter()
                    .map(|(k, x)| (k.clone(), conv(x, hc, depth + 1)))
                    .collect(),
            ),
        },
    }
}
