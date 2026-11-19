//! `porter verify`: proves a load against the source model, never against what load
//! reported. Three checks:
//! - counts: per destination collection, distinct loaded items == the destination's count;
//! - structure: every loaded document's parent, relations, links and media are where the
//!   model says, mapped through `dest_ids` (the destination only says what it holds, via
//!   `Destination::references`);
//! - URLs: every old URL that was 2xx still answers on the new site (`verify.site_url`),
//!   or without a site, an in-model check that each one maps to a loaded doc or redirect.
//!
//! Each checked item gets one Verify status: Done, or Failed with its first error. The
//! check rows go to `raw/verify/checks.json` for the verify report.

use crate::connectors::crawl::site_of;
use crate::mapping::{MappingConfig, TypeMap, apply, lookup, paths};
use crate::project::Project;
use crate::stage::{DestReference, Destination, StageCtx};
use crate::url::normalise_url;
use anyhow::{Context, Result};
use porter_model::{
    Block, Document, FieldValue, Inline, IssueKind, ItemId, ItemKind, ItemStatus, LinkTarget,
    Media, Redirect, Route, RouteTarget, RtBlock, Severity, Stage,
};
use reqwest::{Client, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;
use tokio::task::JoinSet;

/// Load's Skipped reason for an item gone from the source but still in the destination.
pub const REMOVED_FROM_SOURCE: &str = "removed from source";
/// Normalise's (and then load's) Skipped reason for an item that is still in the source
/// but wasn't read this run, so normalise didn't produce it.
pub const NOT_PRODUCED: &str = "not produced this run (see extract issues)";

const USER_AGENT: &str = "porter/2.0 (+https://github.com/umi-labs/porter)";
const CONCURRENCY: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(30);
const RETRIES: u32 = 2;

/// What verify checked, for the report.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Checks {
    pub counts: Vec<CountRow>,
    /// References checked in all, and each one that failed.
    pub refs_checked: u64,
    pub refs: Vec<RefRow>,
    pub urls: Vec<UrlRow>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CountRow {
    pub collection: String,
    pub expected: u64,
    /// Destination docs carrying the key field (migrated ones).
    pub actual: u64,
    /// Every doc in the collection, pre-existing content included.
    #[serde(default)]
    pub total: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RefRow {
    pub item: String,
    pub kind: IssueKind,
    pub severity: Severity,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UrlRow {
    pub url: String,
    pub ok: bool,
    /// `<url> <status>` per request made; empty for the in-model check.
    pub chain: Vec<String>,
    pub error: Option<String>,
}

pub fn checks_path(project: &Project) -> PathBuf {
    project.raw_dir("verify").join("checks.json")
}

pub fn read_checks(project: &Project) -> Result<Option<Checks>> {
    let p = checks_path(project);
    if !p.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    Ok(Some(serde_json::from_str(&text)?))
}

/// Loaded items and where they landed: `dest_ids` rows whose Load status is Done/Partial.
struct Loaded(HashMap<ItemId, (String, Value)>);

impl Loaded {
    fn get(&self, id: ItemId) -> Option<&(String, Value)> {
        self.0.get(&id)
    }
}

/// `collection/id`, the form every message names a destination doc by.
fn key(collection: &str, id: &Value) -> String {
    match id {
        Value::String(s) => format!("{collection}/{s}"),
        v => format!("{collection}/{v}"),
    }
}

pub async fn run(ctx: &StageCtx, dest: &dyn Destination) -> Result<()> {
    let store = ctx.project.store();
    let load: HashMap<ItemId, ItemStatus> = store.statuses(Stage::Load)?.into_iter().collect();
    let loaded = Loaded(
        store
            .dest_ids()?
            .into_iter()
            .filter(|(i, ..)| {
                matches!(
                    load.get(i),
                    Some(ItemStatus::Done | ItemStatus::Partial { .. })
                )
            })
            .map(|(i, c, v)| (i, (c, v)))
            .collect(),
    );
    // Gone from the source model but left in the destination (load flagged each one).
    let mut removed: HashMap<String, u64> = HashMap::new();
    for (_, _, c, _) in store.removed_with_dest(&ctx.config.source.adapter)? {
        *removed.entry(c).or_default() += 1;
    }
    let docs: Vec<Document> = canonical(ctx, ItemKind::Document)?;
    let mut checks = Checks::default();
    counts(ctx, dest, &docs, &loaded, &removed, &mut checks).await?;
    if !ctx.cancel.is_cancelled() {
        structure(ctx, dest, &docs, &loaded, &mut checks).await?;
    }
    if !ctx.cancel.is_cancelled() {
        let new_paths: HashMap<ItemId, String> = paths::render_all(&docs, &ctx.config.mapping)
            .into_iter()
            .filter_map(|(id, r)| Some((id, r.ok()?)))
            .collect();
        match ctx.config.verify.site_url.clone() {
            Some(site) => urls(ctx, &site, &new_paths, &mut checks).await?,
            None => model_urls(ctx, &new_paths, &loaded, &mut checks)?,
        }
    }
    let p = checks_path(&ctx.project);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(&checks)?)
        .with_context(|| format!("writing {}", p.display()))
}

/// Normalised items of `kind` from the source adapter, as load reads them.
/// Undecodable rows get Failed (see `StageCtx::canonical_items`).
fn canonical<T: DeserializeOwned>(ctx: &StageCtx, kind: ItemKind) -> Result<Vec<T>> {
    Ok(canonical_ids(ctx, kind)?
        .into_iter()
        .map(|(_, t)| t)
        .collect())
}

fn canonical_ids<T: DeserializeOwned>(ctx: &StageCtx, kind: ItemKind) -> Result<Vec<(ItemId, T)>> {
    Ok(ctx
        .canonical_items::<T>(kind, &ctx.config.source.adapter)?
        .into_iter()
        .map(|(id, _, t)| (id, t))
        .collect())
}

// ---- counts ----

/// Per mapped collection (document types, plus media): distinct loaded dest ids against
/// the destination docs carrying the type's key field, so content that was there before
/// the migration doesn't count. Distinct, since several items can land on one doc. The
/// collection's total goes in the report only.
async fn counts(
    ctx: &StageCtx,
    dest: &dyn Destination,
    docs: &[Document],
    loaded: &Loaded,
    removed: &HashMap<String, u64>,
    checks: &mut Checks,
) -> Result<()> {
    // collection -> (key field, None = the destination's own key; loaded dest ids)
    let mut want: BTreeMap<String, (Option<String>, HashSet<String>)> = BTreeMap::new();
    // ponytail: types mapped into one collection with different key fields count by the
    // first one; count per key field if that ever happens.
    for tm in ctx.config.mapping.types.values() {
        if let Some(to) = &tm.to {
            want.entry(to.clone())
                .or_insert_with(|| (Some(tm.key_field.clone()), HashSet::new()));
        }
    }
    let media_coll = &ctx.config.mapping.media.collection;
    let media: Vec<Media> = ctx
        .canonical_items::<Media>(ItemKind::Media, "media")?
        .into_iter()
        .map(|(.., m)| m)
        .collect();
    if media.iter().any(|m| loaded.get(m.id).is_some()) {
        want.entry(media_coll.clone()).or_default();
    }
    let ids = docs.iter().map(|d| d.id).chain(media.iter().map(|m| m.id));
    for id in ids {
        if let Some((c, v)) = loaded.get(id)
            && let Some((_, set)) = want.get_mut(c)
        {
            set.insert(key(c, v));
        }
    }
    for (collection, (key_field, ids)) in want {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let expected = ids.len() as u64;
        let actual = dest
            .count_keyed(ctx, &collection, key_field.as_deref())
            .await?;
        let total = dest.count(ctx, &collection).await?;
        if actual != expected {
            let gone = removed.get(&collection).copied().unwrap_or(0);
            let why = if gone > 0 {
                format!(
                    "; {gone} gone from the source model (removed from the source, or not read this run) and still in the destination: see load's warnings"
                )
            } else {
                String::new()
            };
            ctx.issue(
                IssueKind::SchemaMismatch,
                Severity::Error,
                None,
                format!(
                    "{collection}: {expected} loaded but the destination holds {actual} migrated docs{why}"
                ),
                json!({"collection": collection, "expected": expected, "actual": actual}),
            )?;
        }
        checks.counts.push(CountRow {
            collection,
            expected,
            actual,
            total,
        });
    }
    Ok(())
}

// ---- structure ----

/// A reference the model says a document has, mapped through `dest_ids`.
struct Expected {
    /// Top-level destination field.
    field: String,
    target: ItemId,
    media: bool,
    collection: String,
    id: Value,
}

struct Problem {
    kind: IssueKind,
    severity: Severity,
    message: String,
    details: Value,
}

async fn structure(
    ctx: &StageCtx,
    dest: &dyn Destination,
    docs: &[Document],
    loaded: &Loaded,
    checks: &mut Checks,
) -> Result<()> {
    let mapping = &ctx.config.mapping;
    let mut todo: Vec<(&Document, &TypeMap)> = vec![];
    // Every document gets a status: the ones there is nothing to check say why.
    for d in docs {
        let tm = mapping.types.get(&d.doc_type).filter(|tm| tm.to.is_some());
        let reason = match tm {
            None => "not mapped",
            Some(_) if loaded.get(d.id).is_none() => "not loaded",
            Some(tm) => {
                todo.push((d, tm));
                continue;
            }
        };
        let reason = reason.to_string();
        ctx.item_status(d.id, ItemKind::Document, ItemStatus::Skipped { reason })?;
    }
    let mut media_exists: HashMap<ItemId, bool> = HashMap::new();
    // One up-front note when the destination can't see every reference.
    let mut blind_said = false;
    let total = todo.len() as u64;
    for (n, (d, tm)) in todo.into_iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        ctx.progress(n as u64 + 1, Some(total), "check documents");
        let Some((coll, dest_id)) = loaded.get(d.id) else {
            continue;
        };
        let at = key(coll, dest_id);
        let mut problems = vec![];
        match dest.fetch_loaded(ctx, d.id).await? {
            None => problems.push(Problem {
                kind: IssueKind::WriteFailed,
                severity: Severity::Error,
                message: format!("{at} not found in the destination"),
                details: json!({"collection": coll, "id": dest_id}),
            }),
            Some(doc) => {
                let got = dest.references(ctx, coll, &doc)?;
                if !got.complete && !blind_said {
                    blind_said = true;
                    ctx.issue(
                        IssueKind::AccessGap,
                        Severity::Info,
                        None,
                        "bare-id relations can't be verified without destination.config_dir",
                        json!({}),
                    )?;
                }
                let expected = expected_refs(d, tm, mapping, loaded);
                checks.refs_checked += expected.len() as u64 + u64::from(d.parent.is_some());
                parent(d, loaded, &got.refs, &mut problems);
                relations(&expected, &got.refs, got.complete, &mut problems);
                for e in expected.iter().filter(|e| e.media) {
                    let exists = match media_exists.get(&e.target) {
                        Some(x) => *x,
                        None => {
                            let x = dest.fetch_loaded(ctx, e.target).await?.is_some();
                            media_exists.insert(e.target, x);
                            x
                        }
                    };
                    if !exists {
                        problems.push(Problem {
                            kind: IssueKind::MissingMedia,
                            severity: Severity::Error,
                            message: format!(
                                "{}: media {} ({}) not found in the destination",
                                e.field,
                                key(&e.collection, &e.id),
                                e.target
                            ),
                            details: json!({"field": e.field, "target": e.target, "collection": e.collection, "id": e.id}),
                        });
                    }
                }
            }
        }
        let label = format!("{} ({at})", d.title);
        let mut first_error = None;
        for p in problems {
            if p.severity == Severity::Error && first_error.is_none() {
                first_error = Some(p.message.clone());
            }
            checks.refs.push(RefRow {
                item: label.clone(),
                kind: p.kind,
                severity: p.severity,
                message: p.message.clone(),
            });
            ctx.issue(p.kind, p.severity, Some(d.id), p.message, p.details)?;
        }
        let status = match first_error {
            Some(error) => ItemStatus::Failed { error },
            None => ItemStatus::Done,
        };
        ctx.item_status(d.id, ItemKind::Document, status)?;
    }
    Ok(())
}

fn parent(d: &Document, loaded: &Loaded, got: &[DestReference], out: &mut Vec<Problem>) {
    let actual: Vec<String> = got
        .iter()
        .filter(|r| r.parent)
        .map(|r| key(&r.collection, &r.id))
        .collect();
    let found = if actual.is_empty() {
        "none".to_string()
    } else {
        actual.join(", ")
    };
    let message = match d.parent {
        None if actual.is_empty() => return,
        None => format!("parent is {found} but the source has none"),
        Some(p) => match loaded.get(p) {
            Some((c, v)) if actual == [key(c, v)] => return,
            Some((c, v)) => format!("parent should be {} ({p}), found {found}", key(c, v)),
            None => format!("parent {p} was not loaded, found {found}"),
        },
    };
    out.push(Problem {
        kind: IssueKind::ParentMissing,
        severity: Severity::Error,
        message,
        details: json!({"parent": d.parent, "found": actual}),
    });
}

/// Per top-level field: expected references the doc lacks are errors (warnings when the
/// destination couldn't see every reference), a wrong target pairs with a missing one
/// and names both, and references the model doesn't have are warnings.
fn relations(expected: &[Expected], got: &[DestReference], complete: bool, out: &mut Vec<Problem>) {
    let top = |f: &str| f.split('.').next().unwrap_or(f).to_string();
    let mut fields: Vec<String> = expected.iter().map(|e| e.field.clone()).collect();
    fields.extend(got.iter().filter(|r| !r.parent).map(|r| top(&r.field)));
    fields.sort();
    fields.dedup();
    for f in fields {
        let mut actual: Vec<&DestReference> = got
            .iter()
            .filter(|r| !r.parent && top(&r.field) == f)
            .collect();
        let mut missing = vec![];
        for e in expected.iter().filter(|e| e.field == f) {
            let k = key(&e.collection, &e.id);
            match actual.iter().position(|r| key(&r.collection, &r.id) == k) {
                Some(i) => {
                    actual.remove(i);
                }
                None => missing.push(e),
            }
        }
        for e in missing {
            let want = key(&e.collection, &e.id);
            let details = json!({"field": f, "target": e.target, "expected": want});
            if !actual.is_empty() {
                let r = actual.remove(0);
                let found = key(&r.collection, &r.id);
                out.push(Problem {
                    kind: IssueKind::UnresolvedRelation,
                    severity: Severity::Error,
                    message: format!("{f}: should point at {want} ({}), found {found}", e.target),
                    details: json!({"field": f, "target": e.target, "expected": want, "actual": found}),
                });
                continue;
            }
            let (kind, what) = if e.media {
                (IssueKind::MissingMedia, "media")
            } else {
                (IssueKind::UnresolvedRelation, "reference to")
            };
            out.push(Problem {
                kind,
                severity: if complete {
                    Severity::Error
                } else {
                    Severity::Warning
                },
                message: format!(
                    "{f}: {what} {want} ({}) is missing from the loaded doc",
                    e.target
                ),
                details,
            });
        }
        for r in actual {
            let found = key(&r.collection, &r.id);
            out.push(Problem {
                kind: IssueKind::UnresolvedRelation,
                severity: Severity::Warning,
                message: format!("{}: {found} is not in the source", r.field),
                details: json!({"field": r.field, "actual": found}),
            });
        }
    }
}

/// The references `d` should hold, from the model: relation and media fields, internal
/// links (without a fragment: those become plain URLs) and images in rich text, and
/// mapped block props. Only constructs a destination can hold as a reference count, the
/// same ones load converts: images inside lists or quotes, and anything in raw HTML, are
/// dropped at load (with an issue) and not expected here. Targets that were never loaded
/// are load's issue, so they are left out.
fn expected_refs(
    d: &Document,
    tm: &TypeMap,
    mapping: &MappingConfig,
    loaded: &Loaded,
) -> Vec<Expected> {
    let mut raw: Vec<(String, ItemId, bool)> = vec![];
    for (src, fm) in tm.fields.iter().filter(|(_, fm)| !fm.drop) {
        let Ok(v) = apply(&lookup(d, src), &fm.transform) else {
            continue;
        };
        let top = fm.to.split('.').next().unwrap_or(&fm.to).to_string();
        let mut w = Want {
            mapping,
            out: vec![],
        };
        w.value(&v);
        raw.extend(w.out.into_iter().map(|(id, m)| (top.clone(), id, m)));
    }
    raw.into_iter()
        .filter_map(|(field, target, media)| {
            let (collection, id) = loaded.get(target)?.clone();
            Some(Expected {
                field,
                target,
                media,
                collection,
                id,
            })
        })
        .collect()
}

struct Want<'a> {
    mapping: &'a MappingConfig,
    /// (target, is media)
    out: Vec<(ItemId, bool)>,
}

impl Want<'_> {
    fn value(&mut self, v: &FieldValue) {
        match v {
            FieldValue::Doc(id) => self.out.push((*id, false)),
            FieldValue::Media(id) => self.out.push((*id, true)),
            FieldValue::List(xs) => xs.iter().for_each(|x| self.value(x)),
            FieldValue::Object(o) => o.values().for_each(|x| self.value(x)),
            FieldValue::RichText(rt) => self.blocks(&rt.blocks),
            FieldValue::Layout(bs) => self.layout(bs),
            _ => {}
        }
    }

    fn layout(&mut self, bs: &[Block]) {
        for b in bs {
            // Unmatched blocks become raw HTML (their props are lost); children go on.
            if let Some(m) = self.mapping.blocks.iter().find(|m| m.matches(b)) {
                for (k, v) in &b.props {
                    if m.fields.contains_key(k) {
                        self.value(v);
                    }
                }
            }
            self.layout(&b.children);
        }
    }

    fn inlines(&mut self, inlines: &[Inline]) {
        for i in inlines {
            if let Inline::Link {
                target,
                inlines,
                fragment,
                ..
            } = i
            {
                if let (LinkTarget::Doc(id) | LinkTarget::Media(id), None) = (target, fragment) {
                    self.out.push((*id, false));
                }
                self.inlines(inlines);
            }
        }
    }

    fn blocks(&mut self, bs: &[RtBlock]) {
        for b in bs {
            match b {
                RtBlock::Media { media, caption, .. } => {
                    self.out.push((*media, true));
                    if let Some(c) = caption {
                        self.blocks(&c.blocks);
                    }
                }
                RtBlock::Table { rows, .. } if self.mapping.raw_block.is_none() => {
                    rows.iter().flatten().for_each(|c| self.flow(c));
                }
                other => self.flow(std::slice::from_ref(other)),
            }
        }
    }

    /// Blocks as inline content (lists, quotes, table cells): links only.
    fn flow(&mut self, bs: &[RtBlock]) {
        for b in bs {
            match b {
                RtBlock::Paragraph { inlines, .. } | RtBlock::Heading { inlines, .. } => {
                    self.inlines(inlines)
                }
                RtBlock::List { items, .. } => items.iter().for_each(|i| self.flow(i)),
                RtBlock::Quote { blocks } => self.flow(blocks),
                _ => {}
            }
        }
    }
}

// ---- URLs ----

/// The canonical path (and meaningful query) of `u`, absolute or relative: the URL key
/// from the first `/`, so `/About` == `/about/` but `/?p=1` != `/`.
fn path_key(u: &str) -> Option<String> {
    let base = Url::parse("http://h/").ok()?;
    let k = normalise_url(u, Some(&base))?.0;
    Some(k[k.find('/')?..].to_string())
}

/// The path and query of `u`, to request on the new site.
fn path_and_query(u: &str) -> Option<String> {
    let base = Url::parse("http://h/").ok()?;
    let u = base.join(u.trim()).ok()?;
    Some(match u.query() {
        Some(q) => format!("{}?{q}", u.path()),
        None => u.path().to_string(),
    })
}

/// Where a redirect should land on the new site: its target document's new path, else
/// the path of its `to`.
fn redirect_target(
    ctx: &StageCtx,
    r: &Redirect,
    new_paths: &HashMap<ItemId, String>,
) -> Result<Option<String>> {
    Ok(match ctx.project.store().lookup_url(&r.to)? {
        Some(RouteTarget::Document(d)) => new_paths.get(&d).cloned(),
        _ => None,
    }
    .or_else(|| path_and_query(&r.to)))
}

/// An old URL to check, and where it should end up when that is known.
struct Old {
    id: ItemId,
    route: Route,
    expect: Option<String>,
}

/// Routes to check: those that answered 2xx on the old site, plus those with no known
/// status that point at a document (checked against its new path). Any other route with
/// no known status is counted in one `access-gap` warning, never dropped silently.
fn old_urls(ctx: &StageCtx, new_paths: &HashMap<ItemId, String>) -> Result<Vec<Old>> {
    let store = ctx.project.store();
    let mut out = vec![];
    let mut unchecked = 0usize;
    for (id, route) in canonical_ids::<Route>(ctx, ItemKind::Route)? {
        let live = match route.http_status {
            Some(s) => (200..300).contains(&s),
            None => false,
        };
        let expect = match &route.target {
            RouteTarget::Document(d) => new_paths.get(d).cloned(),
            RouteTarget::Redirect(x) => match store.get_item::<Redirect>(*x)? {
                Some(r) => redirect_target(ctx, &r, new_paths)?,
                None => None,
            },
            _ => None,
        };
        match (route.http_status, &route.target) {
            _ if live => {}
            (None, RouteTarget::Document(_)) if expect.is_some() => {}
            (None, _) => {
                unchecked += 1;
                continue;
            }
            _ => continue,
        }
        out.push(Old { id, route, expect });
    }
    if unchecked > 0 {
        ctx.issue(
            IssueKind::AccessGap,
            Severity::Warning,
            None,
            format!("{unchecked} routes have no known old status and were not URL-checked"),
            json!({"count": unchecked}),
        )?;
    }
    Ok(out)
}

fn url_result(
    ctx: &StageCtx,
    id: ItemId,
    kind: ItemKind,
    row: UrlRow,
    checks: &mut Checks,
) -> Result<()> {
    let status = match &row.error {
        Some(error) => {
            ctx.issue(
                IssueKind::UnresolvedLink,
                Severity::Error,
                Some(id),
                error.clone(),
                json!({"url": row.url, "chain": row.chain}),
            )?;
            ItemStatus::Failed {
                error: error.clone(),
            }
        }
        None => ItemStatus::Done,
    };
    checks.urls.push(row);
    ctx.item_status(id, kind, status)
}

/// Without a site: each old URL must map to a loaded doc at the same new path, or be
/// covered by a loaded redirect.
fn model_urls(
    ctx: &StageCtx,
    new_paths: &HashMap<ItemId, String>,
    loaded: &Loaded,
    checks: &mut Checks,
) -> Result<()> {
    ctx.issue(
        IssueKind::AccessGap,
        Severity::Info,
        None,
        "live URL check skipped: set verify.site_url",
        json!({}),
    )?;
    let redirected: HashSet<String> = canonical_ids::<Redirect>(ctx, ItemKind::Redirect)?
        .into_iter()
        .filter(|(id, _)| loaded.get(*id).is_some())
        .filter_map(|(_, r)| path_key(&r.from))
        .collect();
    for Old { id, route: r, .. } in old_urls(ctx, new_paths)? {
        let Some(old) = path_key(&r.url) else {
            continue;
        };
        let covered = redirected.contains(&old);
        let shown = path_and_query(&r.url).unwrap_or_else(|| old.clone());
        let error = match r.target {
            _ if covered => None,
            RouteTarget::Document(d) if loaded.get(d).is_none() => {
                Some(format!("{shown}: target document {d} was not loaded"))
            }
            RouteTarget::Document(d) => match new_paths.get(&d) {
                None => Some(format!(
                    "{shown}: target document {d} has no new path (set a path pattern)"
                )),
                Some(np) if path_key(np).as_deref() == Some(old.as_str()) => None,
                Some(np) => Some(format!(
                    "{shown} now lives at {np} and no redirect covers it"
                )),
            },
            RouteTarget::Redirect(x) | RouteTarget::Media(x) if loaded.get(x).is_some() => None,
            RouteTarget::Redirect(x) => Some(format!("{shown}: redirect {x} was not loaded")),
            RouteTarget::Media(x) => Some(format!("{shown}: media {x} was not loaded")),
            RouteTarget::None => Some(format!("{shown} maps to nothing that was loaded")),
        };
        let row = UrlRow {
            url: r.url.clone(),
            ok: error.is_none(),
            chain: vec![],
            error,
        };
        url_result(ctx, id, ItemKind::Route, row, checks)?;
    }
    Ok(())
}

/// One URL to request: the old route or redirect source on the new site.
struct Job {
    id: ItemId,
    kind: ItemKind,
    old: String,
    url: Url,
    /// Where it must end up, when known.
    expect: Option<String>,
}

async fn urls(
    ctx: &StageCtx,
    site: &str,
    new_paths: &HashMap<ItemId, String>,
    checks: &mut Checks,
) -> Result<()> {
    let site = Url::parse(site).with_context(|| format!("verify.site_url: {site}"))?;
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()?;
    let at = |u: &str| path_and_query(u).and_then(|p| site.join(&p).ok());
    let mut jobs = vec![];
    for Old { id, route, expect } in old_urls(ctx, new_paths)? {
        if let Some(url) = at(&route.url) {
            jobs.push(Job {
                id,
                kind: ItemKind::Route,
                old: route.url,
                url,
                expect,
            });
        }
    }
    for (id, r) in canonical_ids::<Redirect>(ctx, ItemKind::Redirect)? {
        let expect = redirect_target(ctx, &r, new_paths)?;
        if let Some(url) = at(&r.from) {
            jobs.push(Job {
                id,
                kind: ItemKind::Redirect,
                old: r.from,
                url,
                expect,
            });
        }
    }
    let total = jobs.len() as u64;
    let mut jobs = jobs.into_iter();
    let mut running = JoinSet::new();
    let mut done = 0u64;
    loop {
        while running.len() < CONCURRENCY && !ctx.cancel.is_cancelled() {
            let Some(job) = jobs.next() else { break };
            let (client, site) = (client.clone(), site.clone());
            running.spawn(async move {
                let (chain, error) =
                    check_url(&client, &site, &job.url, job.expect.as_deref()).await;
                (job, chain, error)
            });
        }
        let Some(res) = running.join_next().await else {
            break;
        };
        let (job, chain, error) = res?;
        done += 1;
        ctx.progress(done, Some(total), "check old URLs");
        let row = UrlRow {
            url: job.old,
            ok: error.is_none(),
            chain,
            error,
        };
        url_result(ctx, job.id, job.kind, row, checks)?;
    }
    Ok(())
}

/// Same site as the crawl sees it: `www.` and apex, http and https are one site.
fn same_site(a: &Url, b: &Url) -> bool {
    site_of(a).is_some() && site_of(a) == site_of(b)
}

/// Requests `url`, following at most one same-site redirect by hand. With `expect` (the
/// new path, when known) it must answer 200 at that path, or redirect once to exactly
/// that path, which answers 200. Without it, 200 or one 301/302/307/308 hop to a 200.
async fn check_url(
    client: &Client,
    site: &Url,
    url: &Url,
    expect: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut chain = vec![];
    let get = |u: Url| async move {
        crate::http::send_with_retry(client.get(u), RETRIES)
            .await
            .map_err(|e| format!("{e:#}"))
    };
    let path = path_and_query(url.as_str()).unwrap_or_else(|| url.path().to_string());
    let want = expect.and_then(path_key);
    let first = match get(url.clone()).await {
        Ok(r) => r,
        Err(e) => return (chain, Some(format!("{path}: request failed: {e}"))),
    };
    let s = first.status();
    chain.push(format!("{url} {}", s.as_u16()));
    if s == StatusCode::OK {
        return match (&want, expect) {
            (Some(w), Some(e)) if path_key(url.as_str()).as_ref() != Some(w) => (
                chain,
                Some(format!(
                    "{path} answers 200 itself, expected a redirect to {e}"
                )),
            ),
            _ => (chain, None),
        };
    }
    let Some(next) = hop(&first, url) else {
        return (chain, Some(format!("{path} answered {}", s.as_u16())));
    };
    let next_path = path_and_query(next.as_str()).unwrap_or_else(|| next.path().to_string());
    if !same_site(&next, site) {
        let host = next.host_str().unwrap_or("").to_string();
        chain.push(format!("{next} not followed"));
        return (chain, Some(format!("{path} redirects off-site to {host}")));
    }
    if let (Some(w), Some(e)) = (&want, expect)
        && path_key(next.as_str()).as_ref() != Some(w)
    {
        chain.push(format!("{next} not followed"));
        return (
            chain,
            Some(format!("{path} redirects to {next_path}, expected {e}")),
        );
    }
    let second = match get(next.clone()).await {
        Ok(r) => r,
        Err(e) => {
            return (
                chain,
                Some(format!("{path} -> {next_path}: request failed: {e}")),
            );
        }
    };
    let s2 = second.status();
    chain.push(format!("{next} {}", s2.as_u16()));
    if s2 == StatusCode::OK {
        return (chain, None);
    }
    if let Some(third) = hop(&second, &next) {
        chain.push(format!("{third} not followed"));
        return (
            chain,
            Some(format!(
                "{path} -> {next_path} -> {}: more than 1 redirect",
                third.path()
            )),
        );
    }
    (
        chain,
        Some(format!("{path} -> {next_path} answered {}", s2.as_u16())),
    )
}

/// Where a 301/302/307/308 response points, resolved against `from`.
fn hop(res: &reqwest::Response, from: &Url) -> Option<Url> {
    if !matches!(res.status().as_u16(), 301 | 302 | 307 | 308) {
        return None;
    }
    let loc = res
        .headers()
        .get(reqwest::header::LOCATION)?
        .to_str()
        .ok()?;
    from.join(loc).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_normalise() {
        assert_eq!(path_key("https://old.test/A/b").as_deref(), Some("/a/b/"));
        assert_eq!(path_key("/a/b/"), path_key("https://www.old.test/a/b"));
        assert_eq!(path_key("/").as_deref(), Some("/"));
        assert_eq!(path_key("https://old.test").as_deref(), Some("/"));
        // Meaningful queries count; tracking ones don't.
        assert_ne!(path_key("/?p=123"), path_key("/"));
        assert_eq!(path_key("/a/?utm_source=x"), path_key("/a/"));
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(same_site(
            &u("http://www.old.test/a"),
            &u("https://old.test/")
        ));
        assert!(!same_site(
            &u("https://other.test/a"),
            &u("https://old.test/")
        ));
        assert_eq!(
            path_and_query("https://old.test/a/?p=1").as_deref(),
            Some("/a/?p=1")
        );
        assert_eq!(key("pages", &json!(3)), "pages/3");
        assert_eq!(key("pages", &json!("ab")), "pages/ab");
    }
}
