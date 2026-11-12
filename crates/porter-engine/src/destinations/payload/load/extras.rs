//! Everything but media and documents: terms, menus, forms, submissions, redirects,
//! settings. Each loads only where the mapping names a destination.

use super::{Loader, NOT_MAPPED};
use crate::destinations::payload::lexical::{IdMap, to_lexical};
use crate::html::PendingIssue;
use anyhow::Result;
use porter_model::{
    Author, Form, IssueKind, ItemId, ItemKind, Menu, MenuItem, MenuTarget, Redirect, RouteTarget,
    Settings, Severity, Submission, Taxonomy, Term,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

const KEY: &str = "legacyId";

fn key(id: ItemId) -> Value {
    json!(id.to_string())
}

/// Upserts one item's `data` and gives it its status.
#[allow(clippy::too_many_arguments)]
async fn put(
    l: &mut Loader<'_>,
    id: ItemId,
    kind: ItemKind,
    coll: &str,
    key_field: &str,
    k: &Value,
    data: Value,
    issues: Vec<PendingIssue>,
) -> Result<()> {
    let partial = l.emit(id, issues)?;
    match l.upsert(id, coll, key_field, k, data, false).await? {
        Ok(_) => l.finish(id, kind, partial),
        Err(e) => l.fail(id, kind, coll, e),
    }
}

/// Terms into `mapping.taxonomies[taxonomy]`, parents first.
pub(super) async fn terms(l: &mut Loader<'_>) -> Result<()> {
    let map = l.ctx.config.mapping.taxonomies.clone();
    for (id, _, t) in l.items::<Taxonomy>(ItemKind::Taxonomy)? {
        let why = if map.contains_key(&t.name) {
            "taxonomy definitions aren't written: their terms are"
        } else {
            NOT_MAPPED
        };
        l.skip(id, ItemKind::Taxonomy, why)?;
    }
    let mut terms: Vec<(ItemId, Term)> = l
        .items::<Term>(ItemKind::Term)?
        .into_iter()
        .map(|(id, _, t)| (id, t))
        .collect();
    let parents: HashMap<ItemId, Option<ItemId>> =
        terms.iter().map(|(id, t)| (*id, t.parent)).collect();
    let depth = |id: ItemId| {
        let (mut n, mut cur, mut seen) = (0, parents.get(&id).copied().flatten(), HashSet::new());
        while let Some(p) = cur.filter(|p| seen.insert(*p)) {
            n += 1;
            cur = parents.get(&p).copied().flatten();
        }
        n
    };
    terms.sort_by_key(|(id, _)| depth(*id));
    let mut unmapped: HashMap<String, usize> = HashMap::new();
    for (id, t) in terms {
        if l.cancelled() {
            return Ok(());
        }
        let Some(coll) = map.get(&t.taxonomy) else {
            *unmapped.entry(t.taxonomy.clone()).or_default() += 1;
            l.skip(id, ItemKind::Term, NOT_MAPPED)?;
            continue;
        };
        let mut issues = vec![];
        // `parent: null` unless set below, so a parent removed at the source is cleared.
        let mut data = json!({"title": t.name, "slug": t.slug, "parent": null});
        if let Some(p) = t.parent {
            // Only a parent in this term's own collection; otherwise leave it unset.
            match l.ids.get(p) {
                Some(r) if r.collection == *coll => data["parent"] = r.id,
                found => {
                    // Leave the parent as it is rather than clear it.
                    if let Some(o) = data.as_object_mut() {
                        o.remove("parent");
                    }
                    issues.push(PendingIssue {
                        kind: IssueKind::ParentMissing,
                        severity: Severity::Warning,
                        message: match found {
                            Some(r) => format!(
                                "parent term {p} was loaded into {}, not {coll}",
                                r.collection
                            ),
                            None => format!("parent term {p} is not loaded"),
                        },
                        details: json!({"parent": p}),
                    });
                }
            }
        }
        put(l, id, ItemKind::Term, coll, KEY, &key(id), data, issues).await?;
    }
    for (tax, n) in unmapped {
        l.ctx.issue(
            IssueKind::UnmappedField,
            Severity::Info,
            None,
            format!("{n} `{tax}` terms not loaded: no destination mapped (set mapping.taxonomies)"),
            json!({"taxonomy": tax, "count": n}),
        )?;
    }
    Ok(())
}

/// A menu item as `{label, url, children}`; a document target becomes its new path.
fn menu_item(l: &Loader, i: &MenuItem, issues: &mut Vec<PendingIssue>) -> Value {
    let url = match &i.target {
        MenuTarget::Url(u) => u.clone(),
        MenuTarget::Doc(id) => match l.ids.get(*id).and_then(|r| r.path) {
            Some(p) => p,
            None => {
                let old = l.ids.old_url(*id).unwrap_or_default();
                issues.push(PendingIssue {
                    kind: IssueKind::UnresolvedLink,
                    severity: Severity::Warning,
                    message: format!(
                        "menu item `{}`: document {id} has no new path, kept the old URL",
                        i.label
                    ),
                    details: json!({"target": id, "old_url": old}),
                });
                old
            }
        },
    };
    let children: Vec<Value> = i.children.iter().map(|c| menu_item(l, c, issues)).collect();
    json!({"label": i.label, "url": url, "children": children})
}

pub(super) async fn menus(l: &mut Loader<'_>) -> Result<()> {
    let m = l.ctx.config.mapping.menus.clone();
    let menus = l.items::<Menu>(ItemKind::Menu)?;
    if m.collection.is_none() && m.global.is_none() {
        for (id, ..) in &menus {
            l.skip(*id, ItemKind::Menu, NOT_MAPPED)?;
        }
        return l.unmapped(&menus, "menus", "mapping.menus");
    }
    for (id, _, menu) in menus {
        if l.cancelled() {
            return Ok(());
        }
        let mut issues = vec![];
        let items: Vec<Value> = menu
            .items
            .iter()
            .map(|i| menu_item(l, i, &mut issues))
            .collect();
        if let Some(coll) = &m.collection {
            let data = json!({"title": menu.name, "items": items});
            put(l, id, ItemKind::Menu, coll, KEY, &key(id), data, issues).await?;
            continue;
        }
        let Some((slug, field)) = m.global.as_ref().zip(m.fields.get(&menu.name)) else {
            l.skip(id, ItemKind::Menu, NOT_MAPPED)?;
            l.ctx.issue(
                IssueKind::UnmappedField,
                Severity::Info,
                Some(id),
                format!(
                    "menu `{}` not loaded: no field for it in mapping.menus.fields",
                    menu.name
                ),
                json!({"menu": menu.name}),
            )?;
            continue;
        };
        let target = format!("globals/{slug}");
        let data = json!({ field.as_str(): items });
        global(
            l,
            id,
            ItemKind::Menu,
            slug,
            &target,
            json!(field),
            data,
            issues,
        )
        .await?;
    }
    Ok(())
}

/// Writes part of a global and records it as `globals/<slug>` in dest_ids.
#[allow(clippy::too_many_arguments)]
async fn global(
    l: &mut Loader<'_>,
    id: ItemId,
    kind: ItemKind,
    slug: &str,
    target: &str,
    dest: Value,
    data: Value,
    issues: Vec<PendingIssue>,
) -> Result<()> {
    let partial = l.emit(id, issues)?;
    let Some(client) = l.client.clone() else {
        l.preview(target, id, &data)?;
        return l.finish(id, kind, partial);
    };
    match client.update_global(slug, &data).await {
        Ok(_) => {
            l.landed(id, target, dest)?;
            l.finish(id, kind, partial)
        }
        Err(e) => l.fail(id, kind, target, format!("{e:#}")),
    }
}

/// form-builder field block types; others write as `text` with a warning.
const FORM_BLOCKS: [&str; 9] = [
    "text", "textarea", "email", "number", "select", "checkbox", "country", "state", "message",
];

pub(super) async fn forms(l: &mut Loader<'_>) -> Result<()> {
    let fm = l.ctx.config.mapping.forms.clone();
    let forms = l.items::<Form>(ItemKind::Form)?;
    let Some(coll) = fm.collection.clone() else {
        for (id, ..) in &forms {
            l.skip(*id, ItemKind::Form, NOT_MAPPED)?;
        }
        return l.unmapped(&forms, "forms", "mapping.forms.collection");
    };
    let raw = l.ctx.config.mapping.raw_block.clone();
    for (id, _, f) in forms {
        if l.cancelled() {
            return Ok(());
        }
        if f.retired || fm.retire.contains(&f.title) {
            l.skip(id, ItemKind::Form, "retired")?;
            continue;
        }
        let mut issues = vec![];
        let fields: Vec<Value> = f
            .fields
            .iter()
            .map(|x| {
                let kind = match x.kind.as_str() {
                    k if FORM_BLOCKS.contains(&k) => k,
                    "radio" => "select",
                    other => {
                        issues.push(PendingIssue {
                            kind: IssueKind::UnsupportedBlock,
                            severity: Severity::Warning,
                            message: format!(
                                "form field `{}` is a {other}: written as text",
                                x.name
                            ),
                            details: json!({"field": x.name, "kind": other}),
                        });
                        "text"
                    }
                };
                let options: Vec<Value> = x
                    .options
                    .iter()
                    .map(|(value, label)| json!({"label": label, "value": value}))
                    .collect();
                let mut b = json!({"blockType": kind, "name": x.name, "label": x.label,
                    "required": x.required});
                if kind == "select" {
                    b["options"] = json!(options);
                }
                if kind == "message" {
                    b["message"] = lexical_text(&x.label);
                }
                b
            })
            .collect();
        if !f.notifications.is_empty() {
            issues.push(PendingIssue {
                kind: IssueKind::UnmappedField,
                severity: Severity::Info,
                message: format!(
                    "{} email notifications not loaded: set them up in Payload",
                    f.notifications.len()
                ),
                details: json!({"count": f.notifications.len()}),
            });
        }
        let (confirmation, more) = to_lexical(&f.confirmation, &l.ids, raw.as_deref());
        issues.extend(more);
        let data = json!({"title": f.title, "fields": fields, "confirmationType": "message",
            "confirmationMessage": confirmation});
        put(l, id, ItemKind::Form, &coll, KEY, &key(id), data, issues).await?;
    }
    Ok(())
}

fn lexical_text(s: &str) -> Value {
    use porter_model::{Inline, Marks, RichText, RtBlock};
    let rt = RichText {
        blocks: vec![RtBlock::Paragraph {
            inlines: vec![Inline::Text {
                text: s.to_string(),
                marks: Marks::default(),
            }],
            align: Default::default(),
        }],
    };
    to_lexical(
        &rt,
        &crate::destinations::payload::lexical::MemIdMap::default(),
        None,
    )
    .0
}

/// Past submissions into `mapping.forms.submissions`, against their loaded forms. Forms
/// are written without email notifications, so creating these sends no mail from them.
pub(super) async fn submissions(l: &mut Loader<'_>) -> Result<()> {
    let subs = l.items::<Submission>(ItemKind::Submission)?;
    let Some(coll) = l.ctx.config.mapping.forms.submissions.clone() else {
        for (id, ..) in &subs {
            l.skip(*id, ItemKind::Submission, NOT_MAPPED)?;
        }
        return l.unmapped(&subs, "submissions", "mapping.forms.submissions");
    };
    for (id, _, s) in subs {
        if l.cancelled() {
            return Ok(());
        }
        let Some(form) = l.ids.get(s.form) else {
            l.skip(id, ItemKind::Submission, "its form is not loaded")?;
            continue;
        };
        let rows: Vec<Value> = s
            .values
            .iter()
            .map(|(field, value)| json!({"field": field, "value": value}))
            .collect();
        let data = json!({"form": form.id, "submissionData": rows,
            "createdAt": s.created.to_rfc3339()});
        put(
            l,
            id,
            ItemKind::Submission,
            &coll,
            KEY,
            &key(id),
            data,
            vec![],
        )
        .await?;
    }
    Ok(())
}

/// Redirects keyed by `from`; a target that is a loaded document's old URL becomes its
/// new path.
pub(super) async fn redirects(l: &mut Loader<'_>) -> Result<()> {
    let coll = l.ctx.config.mapping.redirects.collection.clone();
    let typed = l
        .coll(&coll)
        .is_some_and(|c| c.fields.iter().any(|f| f.name == "type"));
    for (id, _, r) in l.items::<Redirect>(ItemKind::Redirect)? {
        if l.cancelled() {
            return Ok(());
        }
        let mut issues = vec![];
        let target = match l.ctx.project.store().lookup_url(&r.to)? {
            Some(RouteTarget::Document(d)) => {
                let path = l.ids.get(d).and_then(|x| x.path);
                if path.is_none() {
                    issues.push(PendingIssue {
                        kind: IssueKind::UnresolvedLink,
                        severity: Severity::Warning,
                        message: format!(
                            "redirect {} -> {}: target document {d} is not loaded or has no new path, kept the old URL",
                            r.from, r.to
                        ),
                        details: json!({"from": r.from, "to": r.to, "target": d}),
                    });
                }
                path
            }
            _ => None,
        };
        let mut data = json!({"from": r.from,
            "to": {"type": "custom", "url": target.unwrap_or(r.to)}});
        if typed {
            data["type"] = json!(r.code.to_string());
        }
        let from = json!(r.from);
        put(
            l,
            id,
            ItemKind::Redirect,
            &coll,
            "from",
            &from,
            data,
            issues,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn settings(l: &mut Loader<'_>) -> Result<()> {
    let m = l.ctx.config.mapping.settings.clone();
    let all = l.items::<Settings>(ItemKind::Settings)?;
    let Some(slug) = m.global.clone() else {
        for (id, ..) in &all {
            l.skip(*id, ItemKind::Settings, NOT_MAPPED)?;
        }
        return l.unmapped(&all, "settings", "mapping.settings.global");
    };
    for (id, _, s) in all {
        let mut issues = vec![];
        let mut data = Map::new();
        if let Some(f) = &m.site_name {
            data.insert(f.clone(), json!(s.site_name));
        }
        if let Some(f) = &m.front_page {
            let v = match s.front_page {
                Some(p) => {
                    let field = l
                        .schema
                        .as_ref()
                        .and_then(|d| d.globals.get(&slug))
                        .and_then(|g| g.fields.iter().find(|x| &x.name == f));
                    l.relation(p, field, IssueKind::UnresolvedRelation, &mut issues)
                }
                None => Value::Null,
            };
            data.insert(f.clone(), v);
        }
        let target = format!("globals/{slug}");
        global(
            l,
            id,
            ItemKind::Settings,
            &slug,
            &target,
            json!(slug),
            Value::Object(data),
            issues,
        )
        .await?;
    }
    Ok(())
}

/// Authors have no destination yet.
pub(super) fn authors(l: &Loader) -> Result<()> {
    let all = l.items::<Author>(ItemKind::Author)?;
    for (id, ..) in &all {
        l.skip(*id, ItemKind::Author, NOT_MAPPED)?;
    }
    l.unmapped(&all, "authors", "no author mapping exists yet")
}
