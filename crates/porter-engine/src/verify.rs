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

use crate::mapping::{MappingConfig, TypeMap, apply, lookup, paths};
use crate::project::Project;
use crate::stage::{DestReference, Destination, StageCtx};
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
    pub actual: u64,
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
    let docs: Vec<Document> = canonical(ctx, ItemKind::Document)?;
    let mut checks = Checks::default();
    counts(ctx, dest, &docs, &loaded, &mut checks).await?;
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
fn canonical<T: DeserializeOwned>(ctx: &StageCtx, kind: ItemKind) -> Result<Vec<T>> {
    let adapter = &ctx.config.source.adapter;
    Ok(ctx
        .project
        .store()
        .items_decodable::<T>(kind)?
        .into_iter()
        .filter(|(_, s, _)| &s.connector == adapter)
        .map(|(.., t)| t)
        .collect())
}

fn canonical_ids<T: DeserializeOwned>(ctx: &StageCtx, kind: ItemKind) -> Result<Vec<(ItemId, T)>> {
    let adapter = &ctx.config.source.adapter;
    Ok(ctx
        .project
        .store()
        .items_decodable::<T>(kind)?
        .into_iter()
        .filter(|(_, s, _)| &s.connector == adapter)
        .map(|(id, _, t)| (id, t))
        .collect())
}

// ---- counts ----

/// Per mapped collection (document types, plus media): distinct loaded dest ids against
/// the destination's count. Distinct, since several items can land on one doc.
async fn counts(
    ctx: &StageCtx,
    dest: &dyn Destination,
    docs: &[Document],
    loaded: &Loaded,
    checks: &mut Checks,
) -> Result<()> {
    let mut want: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for tm in ctx.config.mapping.types.values() {
        if let Some(to) = &tm.to {
            want.entry(to.clone()).or_default();
        }
    }
    let media: Vec<Media> = ctx
        .project
        .store()
        .items_decodable::<Media>(ItemKind::Media)?
        .into_iter()
        .map(|(.., m)| m)
        .collect();
    let ids = docs.iter().map(|d| d.id).chain(media.iter().map(|m| m.id));
    let media_coll = &ctx.config.mapping.media.collection;
    for id in ids {
        if let Some((c, v)) = loaded.get(id)
            && (want.contains_key(c) || c == media_coll)
        {
            want.entry(c.clone()).or_default().insert(key(c, v));
        }
    }
    for (collection, ids) in want {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let expected = ids.len() as u64;
        let actual = dest.count(ctx, &collection).await?;
        if actual != expected {
            ctx.issue(
                IssueKind::SchemaMismatch,
                Severity::Error,
                None,
                format!("{collection}: {expected} loaded but the destination holds {actual}"),
                json!({"collection": collection, "expected": expected, "actual": actual}),
            )?;
        }
        checks.counts.push(CountRow {
            collection,
            expected,
            actual,
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
    let todo: Vec<(&Document, &TypeMap)> = docs
        .iter()
        .filter(|d| loaded.get(d.id).is_some())
        .filter_map(|d| Some((d, mapping.types.get(&d.doc_type)?)))
        .collect();
    let mut media_exists: HashMap<ItemId, bool> = HashMap::new();
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

/// The path of `u` (absolute or relative), without a trailing slash (root stays `/`).
fn norm_path(u: &str) -> Option<String> {
    let base = Url::parse("http://h/").ok()?;
    let p = base.join(u.trim()).ok()?.path().to_string();
    let t = p.trim_end_matches('/');
    Some(if t.is_empty() { "/".into() } else { t.into() })
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

/// Routes that answered 2xx on the old site.
fn live_routes(ctx: &StageCtx) -> Result<Vec<(ItemId, Route)>> {
    Ok(canonical_ids::<Route>(ctx, ItemKind::Route)?
        .into_iter()
        .filter(|(_, r)| r.http_status.is_some_and(|s| (200..300).contains(&s)))
        .collect())
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
        .filter_map(|(_, r)| norm_path(&r.from))
        .collect();
    for (id, r) in live_routes(ctx)? {
        let Some(old) = norm_path(&r.url) else {
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
                Some(np) if norm_path(np).as_deref() == Some(old.as_str()) => None,
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
    /// Redirect sources: a 301/308 straight to this path passes.
    to: Option<String>,
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
    for (id, r) in live_routes(ctx)? {
        if let Some(url) = at(&r.url) {
            jobs.push(Job {
                id,
                kind: ItemKind::Route,
                old: r.url,
                url,
                to: None,
            });
        }
    }
    for (id, r) in canonical_ids::<Redirect>(ctx, ItemKind::Redirect)? {
        let to = match ctx.project.store().lookup_url(&r.to)? {
            Some(RouteTarget::Document(d)) => new_paths.get(&d).cloned(),
            _ => None,
        }
        .or_else(|| path_and_query(&r.to));
        if let Some(url) = at(&r.from) {
            jobs.push(Job {
                id,
                kind: ItemKind::Redirect,
                old: r.from,
                url,
                to,
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
                let (chain, error) = check_url(&client, &site, &job.url, job.to.as_deref()).await;
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

fn same_site(a: &Url, b: &Url) -> bool {
    a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default()
}

/// Requests `url`, following at most one same-site redirect by hand. Passes on 200, on
/// one 301/302/307/308 hop to a 200, or (redirect sources) on a 301/308 straight to `to`.
async fn check_url(
    client: &Client,
    site: &Url,
    url: &Url,
    to: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut chain = vec![];
    let get = |u: Url| async move {
        crate::http::send_with_retry(client.get(u), RETRIES)
            .await
            .map_err(|e| format!("{e:#}"))
    };
    let path = url.path().to_string();
    let first = match get(url.clone()).await {
        Ok(r) => r,
        Err(e) => return (chain, Some(format!("{path}: request failed: {e}"))),
    };
    let s = first.status();
    chain.push(format!("{url} {}", s.as_u16()));
    if s == StatusCode::OK {
        return (chain, None);
    }
    let Some(next) = hop(&first, url) else {
        return (chain, Some(format!("{path} answered {}", s.as_u16())));
    };
    if let Some(to) = to
        && matches!(s.as_u16(), 301 | 308)
        && same_site(&next, site)
        && norm_path(next.path()) == norm_path(to)
    {
        return (chain, None);
    }
    if !same_site(&next, site) {
        let host = next.host_str().unwrap_or("").to_string();
        chain.push(format!("{next} not followed"));
        return (chain, Some(format!("{path} redirects off-site to {host}")));
    }
    let second = match get(next.clone()).await {
        Ok(r) => r,
        Err(e) => {
            return (
                chain,
                Some(format!("{path} -> {}: request failed: {e}", next.path())),
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
                "{path} -> {} -> {}: more than 1 redirect",
                next.path(),
                third.path()
            )),
        );
    }
    (
        chain,
        Some(format!(
            "{path} -> {} answered {}",
            next.path(),
            s2.as_u16()
        )),
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
        assert_eq!(norm_path("https://old.test/a/b/").as_deref(), Some("/a/b"));
        assert_eq!(norm_path("/").as_deref(), Some("/"));
        assert_eq!(norm_path("https://old.test").as_deref(), Some("/"));
        assert_eq!(
            path_and_query("https://old.test/a/?p=1").as_deref(),
            Some("/a/?p=1")
        );
        assert_eq!(key("pages", &json!(3)), "pages/3");
        assert_eq!(key("pages", &json!("ab")), "pages/ab");
    }
}
