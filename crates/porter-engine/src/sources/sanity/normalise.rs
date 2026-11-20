//! Sanity normalise: documents → `Document`s, submission types → `Form`s and
//! `Submission`s, plus routes, manual redirects and per-use media details (connector `sanity`).

use super::CANON;
use super::analyse::{MIN_SUBMISSION_DOCS, detect_submissions, is_plugin_type};
use super::assets::cdn_url;
use super::export::{self, Export};
use super::fields::{Cx, MediaUse, array_types, flatten};
use crate::html::PendingIssue;
use crate::mapping::paths::{PathCx, render};
use crate::mapping::transforms::slugify;
use crate::stage::StageCtx;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use porter_model::{
    DocStatus, Document, Form, FormField, IssueKind, ItemId, ItemKind, ItemStatus, Media, Redirect,
    RedirectOrigin, RichText, Route, RouteTarget, Seo, Severity, SourceRef, Submission,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Converters recurse deeply on nested content.
const STACK: usize = 16 * 1024 * 1024;

pub(super) fn run(ctx: &StageCtx) -> Result<()> {
    std::thread::scope(|s| {
        let h = std::thread::Builder::new()
            .name("sanity-normalise".into())
            .stack_size(STACK)
            .spawn_scoped(s, || normalise(ctx))?;
        h.join()
            .map_err(|e| anyhow!("normalise panicked: {}", panic_text(&*e)))?
    })
}

fn panic_text(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "converter panicked".into())
}

fn canon(kind: ItemKind, sid: &str, url: Option<String>) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: CANON.into(),
        source_id: sid.into(),
        url,
    };
    (ItemId::new(kind, &src), src)
}

fn text<'a>(d: &'a Value, k: &str) -> Option<&'a str> {
    d.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn date(d: &Value, k: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text(d, k)?)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// One document to load: the published version when there is one, else the draft.
struct Entry<'a> {
    sid: String,
    ty: String,
    doc: &'a Value,
    status: DocStatus,
    /// A draft exists beside the published version.
    has_draft: bool,
}

fn entries(ex: &Export) -> Vec<Entry<'_>> {
    let mut order: Vec<String> = vec![];
    let mut published: HashMap<String, &Value> = HashMap::new();
    let mut drafts: HashMap<String, &Value> = HashMap::new();
    for d in &ex.docs {
        let Some(id) = text(d, "_id") else { continue };
        if text(d, "_type").is_some_and(is_plugin_type) {
            continue;
        }
        let (sid, is_draft) = match id.strip_prefix("drafts.") {
            Some(s) => (s, true),
            None => (id, false),
        };
        if !published.contains_key(sid) && !drafts.contains_key(sid) {
            order.push(sid.to_string());
        }
        let map = if is_draft {
            &mut drafts
        } else {
            &mut published
        };
        map.insert(sid.to_string(), d);
    }
    order
        .into_iter()
        .filter_map(|sid| {
            let draft = drafts.get(&sid).copied();
            let (doc, status) = match published.get(&sid) {
                Some(p) => (*p, DocStatus::Published),
                None => (draft?, DocStatus::Draft),
            };
            Some(Entry {
                ty: text(doc, "_type").unwrap_or_default().to_string(),
                has_draft: status == DocStatus::Published && draft.is_some(),
                sid,
                doc,
                status,
            })
        })
        .collect()
}

/// `title_field` (per type) when it holds text, else the first of title/name/heading/label.
fn title_of(e: &Entry, title_field: Option<&str>) -> String {
    title_field
        .into_iter()
        .chain(["title", "name", "heading", "label"])
        .find_map(|k| text(e.doc, k))
        .unwrap_or(&e.sid)
        .to_string()
}

/// `Document.seo` from the first `seoGroup`-typed (or `seo`-named) object field.
fn seo_of(d: &Value, cx: &mut Cx) -> Seo {
    let mut seo = Seo::default();
    let Some(o) = d.as_object().into_iter().flatten().find_map(|(k, v)| {
        let o = v.as_object()?;
        (text(v, "_type") == Some("seoGroup") || k.eq_ignore_ascii_case("seo")).then_some(o)
    }) else {
        return seo;
    };
    for (k, v) in o {
        match k.to_ascii_lowercase().as_str() {
            "metatitle" | "title" => seo.title = v.as_str().map(str::to_string),
            "metadescription" | "description" => seo.description = v.as_str().map(str::to_string),
            "noindex" => seo.noindex = v.as_bool().unwrap_or(false),
            "ogimage" | "shareimage" => seo.og_image = cx.og_image(v),
            _ => {}
        }
    }
    seo
}

fn normalise(ctx: &StageCtx) -> Result<()> {
    let store = ctx.project.store();
    let cfg = &ctx.config;
    let ex = export::load(&ctx.project.raw_dir(CANON))?;
    let mut media: HashMap<String, ItemId> = HashMap::new();
    for a in ex.assets.values() {
        if let Some(id) = store.media_by_url(&cdn_url(a))? {
            media.insert(a.id.clone(), id);
        }
    }
    let asset_ids: BTreeSet<String> = ex.assets.keys().cloned().collect();
    let mut plugin: BTreeMap<&str, usize> = BTreeMap::new();
    for t in ex.docs.iter().filter_map(|d| text(d, "_type")) {
        if is_plugin_type(t) {
            *plugin.entry(t).or_default() += 1;
        }
    }
    for (t, n) in plugin {
        ctx.issue(
            IssueKind::UnmappedField,
            Severity::Info,
            None,
            format!("{t}: plugin or system documents ({n}) aren't content; skipped"),
            json!({"type": t, "count": n}),
        )?;
    }
    let arrays = array_types(
        &ex.docs
            .iter()
            .filter(|d| !text(d, "_type").is_some_and(is_plugin_type))
            .collect::<Vec<_>>(),
    );
    let all = entries(&ex);
    let mut sub_types: BTreeSet<String> = detect_submissions(&ex.docs, MIN_SUBMISSION_DOCS)
        .into_iter()
        .collect();
    sub_types.extend(cfg.mapping.forms.submission_types.iter().cloned());
    let (subs, docs): (Vec<_>, Vec<_>) = all.iter().partition(|e| sub_types.contains(&e.ty));
    let doc_ids: HashMap<String, ItemId> = docs
        .iter()
        .map(|e| (e.sid.clone(), canon(ItemKind::Document, &e.sid, None).0))
        .collect();
    let site = cfg
        .verify
        .site_url
        .as_deref()
        .map(|u| u.trim_end_matches('/'));

    let mut uses: BTreeMap<String, MediaUse> = BTreeMap::new();
    let mut conflicts: BTreeSet<String> = BTreeSet::new();
    let mut finished = vec![];
    let total = docs.len() as u64;
    for (i, e) in docs.iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        if i % 200 == 0 {
            ctx.progress(i as u64, Some(total), "documents");
        }
        let title_field = cfg
            .mapping
            .types
            .get(&e.ty)
            .and_then(|t| t.title_field.as_deref());
        let title = title_of(e, title_field);
        let mut cx = Cx {
            docs: &doc_ids,
            media: &media,
            assets: &asset_ids,
            arrays: &arrays,
            uses: &mut uses,
            conflicts: &mut conflicts,
            label: format!("{} \"{title}\"", e.ty),
            issues: vec![],
        };
        if e.has_draft {
            let msg = format!(
                "{} has unpublished changes in Sanity; only the published version is loaded",
                cx.label
            );
            cx.issues.push(PendingIssue {
                kind: IssueKind::UnmappedField,
                severity: Severity::Warning,
                message: msg,
                details: json!({"draft": format!("drafts.{}", e.sid)}),
            });
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let seo = seo_of(e.doc, &mut cx);
            let fields: IndexMap<_, _> = e
                .doc
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(k, _)| !k.starts_with('_') && k.as_str() != "slug")
                .map(|(k, v)| (k.clone(), cx.conv(v, &format!("{}.{k}", e.ty), 0, false)))
                .collect();
            (seo, fields)
        }));
        let (seo, fields, st) = match outcome {
            Ok((seo, fields)) => (seo, fields, ItemStatus::Done),
            Err(p) => {
                let error = panic_text(&*p);
                cx.issues.push(PendingIssue {
                    kind: IssueKind::UnsupportedBlock,
                    severity: Severity::Error,
                    message: format!("content could not be converted: {error}"),
                    details: json!({"id": e.sid}),
                });
                (
                    Seo::default(),
                    IndexMap::new(),
                    ItemStatus::Failed { error },
                )
            }
        };
        let slug = e
            .doc
            .pointer("/slug/current")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| slugify(&title).ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| e.sid.clone());
        let (id, src) = canon(ItemKind::Document, &e.sid, None);
        let d = Document {
            id,
            doc_type: e.ty.clone(),
            status: e.status,
            locale: None,
            translations: vec![],
            path: format!("/{}/{slug}/", e.ty),
            slug,
            parent: None,
            title,
            fields,
            seo,
            created: date(e.doc, "_createdAt"),
            updated: date(e.doc, "_updatedAt"),
            author: None,
            original: e.doc.clone(),
            source: src,
        };
        finished.push((d, std::mem::take(&mut cx.issues), st, cx.label.clone()));
    }

    // Paths are rendered once every document exists, so patterns can follow references.
    // ponytail: clones every document once; index by id if exports get huge.
    let by_id: HashMap<ItemId, Document> = finished.iter().map(|f| (f.0.id, f.0.clone())).collect();
    let pcx = PathCx {
        docs: &by_id,
        lookups: &cfg.mapping.lookups,
    };
    for (d, issues, _, label) in &mut finished {
        let mut routable = false;
        if let Some(p) = cfg
            .mapping
            .types
            .get(&d.doc_type)
            .and_then(|t| t.path.as_deref())
        {
            match render(p, d, Some(""), &pcx) {
                Ok(path) => {
                    d.path = path;
                    routable = true;
                }
                Err(why) => issues.push(PendingIssue {
                    kind: IssueKind::UnresolvedRelation,
                    severity: Severity::Warning,
                    message: format!("{label}: path pattern didn't render: {why}"),
                    details: json!({"pattern": p}),
                }),
            }
        }
        d.source.url = site
            .filter(|_| routable && d.status == DocStatus::Published)
            .map(|s| format!("{s}{}", d.path));
    }
    // Written after converting: a write resets the URL index media lookups use.
    let mut written: HashSet<ItemId> = HashSet::new();
    for (d, issues, st, _) in finished {
        store.put_item(ItemKind::Document, d.id, &d.source, &d)?;
        written.insert(d.id);
        for i in issues {
            ctx.issue(i.kind, i.severity, Some(d.id), i.message, i.details)?;
        }
        ctx.item_status(d.id, ItemKind::Document, st)?;
        if let Some(u) = &d.source.url {
            let (rid, rsrc) = canon(
                ItemKind::Route,
                &format!("route:{}", d.source.source_id),
                Some(u.clone()),
            );
            let route = Route {
                url: u.clone(),
                target: RouteTarget::Document(d.id),
                http_status: None,
                title: Some(d.title.clone()),
            };
            store.put_item(ItemKind::Route, rid, &rsrc, &route)?;
            ctx.item_status(rid, ItemKind::Route, ItemStatus::Done)?;
            written.insert(rid);
        }
    }
    ctx.progress(total, Some(total), "documents");

    describe_media(ctx, &ex, &media, &uses, &conflicts)?;
    submissions(ctx, &subs, &mut written)?;
    redirects(ctx, &mut written)?;
    drop_stale(ctx, &written)
}

/// First-use focal point, alt and caption onto each media item; the asset's `altText` wins.
fn describe_media(
    ctx: &StageCtx,
    ex: &Export,
    media: &HashMap<String, ItemId>,
    uses: &BTreeMap<String, MediaUse>,
    conflicts: &BTreeSet<String>,
) -> Result<()> {
    let store = ctx.project.store();
    for (aref, u) in uses {
        let Some(&id) = media.get(aref) else { continue };
        let Some(mut m) = store.get_item::<Media>(id)? else {
            continue;
        };
        let before = m.clone();
        m.focal_x = u.focal.map(|f| f.0);
        m.focal_y = u.focal.map(|f| f.1);
        m.alt = ex
            .assets
            .get(aref)
            .and_then(|a| a.alt.clone())
            .or_else(|| u.alt.clone());
        m.caption = u.caption.clone();
        if m != before {
            let src = SourceRef {
                connector: "media".into(),
                source_id: m.hash.clone(),
                url: m.variants.first().cloned(),
            };
            store.put_item(ItemKind::Media, id, &src, &m)?;
        }
    }
    for aref in conflicts {
        ctx.issue(
            IssueKind::UnmappedField,
            Severity::Warning,
            media.get(aref).copied(),
            format!(
                "{aref} is used with different focal points or alt text; the first use was kept"
            ),
            json!({"asset": aref}),
        )?;
    }
    Ok(())
}

fn field_kind(name: &str) -> &'static str {
    let last = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
    if last.contains("email") {
        "email"
    } else if last == "message" {
        "textarea"
    } else {
        "text"
    }
}

/// One `Form` per submission type (fields = union of submitted names), one `Submission` per doc.
fn submissions(ctx: &StageCtx, subs: &[&Entry], written: &mut HashSet<ItemId>) -> Result<()> {
    let store = ctx.project.store();
    let mut by_type: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in subs {
        by_type.entry(&e.ty).or_default().push(e);
    }
    for (ty, es) in by_type {
        let (fid, fsrc) = canon(ItemKind::Form, &format!("form:{ty}"), None);
        let mut names: IndexMap<String, ()> = IndexMap::new();
        let mut rows = vec![];
        for e in es {
            let mut values = IndexMap::new();
            if let Some(o) = e.doc.as_object() {
                flatten(o, "", &mut values);
            }
            names.extend(values.keys().map(|k| (k.clone(), ())));
            let (sid, ssrc) = canon(ItemKind::Submission, &e.sid, None);
            let s = Submission {
                form: fid,
                created: date(e.doc, "_createdAt"),
                values,
            };
            rows.push((sid, ssrc, s));
        }
        let form = Form {
            title: ty.to_string(),
            fields: names
                .into_keys()
                .map(|n| FormField {
                    label: n.clone(),
                    kind: field_kind(&n).into(),
                    name: n,
                    required: false,
                    options: vec![],
                    conditions: Value::Null,
                })
                .collect(),
            notifications: vec![],
            confirmation: RichText::default(),
            retired: false,
        };
        store.put_item(ItemKind::Form, fid, &fsrc, &form)?;
        ctx.item_status(fid, ItemKind::Form, ItemStatus::Done)?;
        written.insert(fid);
        for (sid, ssrc, s) in rows {
            store.put_item(ItemKind::Submission, sid, &ssrc, &s)?;
            ctx.item_status(sid, ItemKind::Submission, ItemStatus::Done)?;
            written.insert(sid);
        }
    }
    Ok(())
}

/// Manual `[[redirects]]` from porter.toml.
fn redirects(ctx: &StageCtx, written: &mut HashSet<ItemId>) -> Result<()> {
    let store = ctx.project.store();
    for m in &ctx.config.redirects {
        let (rid, src) = canon(ItemKind::Redirect, &format!("redirect:{}", m.from), None);
        let r = Redirect {
            from: m.from.clone(),
            to: m.to.clone(),
            code: m.code,
            origin: RedirectOrigin::Manual,
        };
        store.put_item(ItemKind::Redirect, rid, &src, &r)?;
        ctx.item_status(rid, ItemKind::Redirect, ItemStatus::Done)?;
        written.insert(rid);
    }
    Ok(())
}

/// Soft-deletes what an earlier run made and this one didn't.
fn drop_stale(ctx: &StageCtx, written: &HashSet<ItemId>) -> Result<()> {
    let store = ctx.project.store();
    for (id, kind, connector) in store.item_rows()? {
        let kind: ItemKind = serde_json::from_value(json!(kind))?;
        if connector != CANON
            || !matches!(
                kind,
                ItemKind::Document
                    | ItemKind::Route
                    | ItemKind::Form
                    | ItemKind::Submission
                    | ItemKind::Redirect
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_field_wins_when_it_holds_text() {
        let doc = json!({"_id":"a","_type":"t","title":"T","headline":"H","empty":""});
        let e = Entry {
            sid: "a".into(),
            ty: "t".into(),
            doc: &doc,
            status: DocStatus::Published,
            has_draft: false,
        };
        assert_eq!(title_of(&e, Some("headline")), "H");
        assert_eq!(title_of(&e, Some("empty")), "T");
        assert_eq!(title_of(&e, None), "T");
    }
}
