//! WordPress normalise. Everything the connectors captured is merged per WordPress id, field by
//! field (wp-db > wp-rest > wxr; the crawl adds routes, statuses and heads), and written as
//! neutral-model items at canonical ids (connector `wordpress`), so ids are the same whichever
//! connectors ran. Pass A writes media, terms, document skeletons, redirects and routes so
//! links and media resolve; pass B converts content, ACF and SEO and writes the final
//! documents. Every item written gets a Normalise status; connector rows that fed nothing get
//! their own `Skipped`, and `raw/normalise/provenance.json` maps each row to what it became.

use super::load::{Raw, configured, load};
use super::redirects;
use super::{acf, classic_to_richtext, divi, forms, gutenberg, media, seo};
use crate::html::{HtmlCtx, PendingIssue, decode_entities};
use crate::stage::StageCtx;
use crate::store::Store;
use crate::url::normalise_url;
use ::url::Url;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use porter_model::{
    DocStatus, Document, FieldValue, IssueKind, ItemId, ItemKind, ItemStatus, Menu, MenuItem,
    MenuTarget, Route, RouteTarget, Seo, Settings, Severity, SourceRef, Taxonomy, Term,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

pub(crate) const CONNECTOR: &str = "wordpress";
/// Converters recurse up to depth 256, which needs 1–2 MB in debug builds.
const STACK: usize = 16 * 1024 * 1024;
/// Parent chains longer than this are cut (paths only).
const MAX_PARENTS: usize = 64;

/// Canonical id and source for a normalised item.
pub(crate) fn canon(kind: ItemKind, sid: &str, url: Option<String>) -> (ItemId, SourceRef) {
    let src = SourceRef {
        connector: CONNECTOR.into(),
        source_id: sid.into(),
        url,
    };
    (ItemId::new(kind, &src), src)
}

fn post_id(id: u64) -> ItemId {
    canon(ItemKind::Document, &format!("post:{id}"), None).0
}

/// A term reference as a connector gives it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TermKey {
    Id(u64),
    Slug(String),
}

/// One WordPress post (any type) merged from every connector. `parent`/`featured` are
/// `Some(0)` when a connector said "none".
#[derive(Default, Clone, Debug)]
pub(crate) struct Post {
    pub id: u64,
    pub post_type: String,
    pub status: String,
    pub slug: String,
    pub title: String,
    pub link: Option<String>,
    pub parent: Option<u64>,
    pub menu_order: Option<i64>,
    /// Author key (login, or REST slug).
    pub author: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub content: Option<String>,
    /// REST's rendered HTML, used only when no connector had the raw content.
    pub content_rendered: Option<String>,
    pub excerpt: Option<String>,
    pub meta: Map<String, Value>,
    pub acf: Option<Map<String, Value>>,
    pub terms: Vec<(String, TermKey)>,
    pub featured: Option<u64>,
    pub mime: Option<String>,
    /// `_wp_attached_file`, relative to uploads.
    pub file: Option<String>,
    /// `_wp_attachment_metadata` (sizes, original_image, width, height).
    pub file_meta: Option<Value>,
    pub alt: Option<String>,
    pub caption: Option<String>,
    pub lang: Option<String>,
    /// Translation post ids a connector listed (Polylang over REST).
    pub translations: Vec<u64>,
    /// Yoast's rendered head (REST `yoast_head_json`).
    pub head: Option<Value>,
    /// WXR: slug of the menu a `nav_menu_item` belongs to.
    pub nav_menu: Option<String>,
    pub template: Option<String>,
    /// Connector rows that described this post.
    pub raw: Vec<ItemId>,
}

impl Post {
    /// Fills what this (higher-priority) record lacks from `lo`.
    pub fn fill(&mut self, lo: Post) {
        macro_rules! text {
            ($($f:ident),*) => { $( if self.$f.is_empty() { self.$f = lo.$f; } )* };
        }
        macro_rules! opt {
            ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = lo.$f; } )* };
        }
        text!(post_type, status, slug, title);
        opt!(
            link,
            parent,
            menu_order,
            author,
            created,
            updated,
            content,
            content_rendered,
            excerpt,
            acf,
            featured,
            mime,
            file,
            file_meta,
            alt,
            caption,
            lang,
            head,
            nav_menu,
            template
        );
        for (k, v) in lo.meta {
            self.meta.entry(k).or_insert(v);
        }
        if self.terms.is_empty() {
            self.terms = lo.terms;
        }
        if self.translations.is_empty() {
            self.translations = lo.translations;
        }
        self.raw.extend(lo.raw);
    }

    fn parent_id(&self) -> Option<u64> {
        self.parent.filter(|p| *p > 0 && *p != self.id)
    }
}

/// Site-wide facts every part needs.
pub(crate) struct Site {
    pub name: String,
    pub tagline: String,
    /// Base for relative URLs.
    pub home: Url,
    /// Every host the site was seen on (first is the main one).
    pub hosts: Vec<String>,
    pub front_page: Option<u64>,
    pub permalink: Option<String>,
    /// Yoast `wpseo_titles`.
    pub titles: Value,
    /// The uploads folder (`wp_root/wp-content/uploads` unless `uploads_dir` is set).
    pub uploads: Option<PathBuf>,
}

/// Runs normalise on its own thread with a big stack; errors and panics come back here.
pub fn run(ctx: &StageCtx) -> Result<()> {
    std::thread::scope(|s| {
        let h = std::thread::Builder::new()
            .name("wp-normalise".into())
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

fn doc_status(status: &str) -> Result<DocStatus, String> {
    Ok(match status {
        "publish" => DocStatus::Published,
        "draft" | "pending" => DocStatus::Draft,
        "private" => DocStatus::Private,
        "future" => DocStatus::Scheduled,
        "auto-draft" => return Err("auto-draft".into()),
        "trash" => return Err("in the trash".into()),
        "inherit" => return Err("inherits its parent's status (not content)".into()),
        other => return Err(format!("unknown post status \"{other}\"")),
    })
}

/// Why a post is not a document, if it isn't.
fn not_a_document(p: &Post) -> Option<String> {
    Some(match p.post_type.as_str() {
        "revision" => "revision".into(),
        "nav_menu_item" => "part of a menu".into(),
        "acf-field" | "acf-field-group" => "ACF field definition (used to type ACF values)".into(),
        "wp_block"
        | "et_pb_layout"
        | "wp_template"
        | "wp_template_part"
        | "wp_navigation"
        | "wp_global_styles"
        | "custom_css"
        | "oembed_cache"
        | "user_request"
        | "wp_font_family"
        | "wp_font_face"
        | "customize_changeset"
        | "nf_sub" => {
            format!("internal WordPress type {}", p.post_type)
        }
        _ => return doc_status(&p.status).err(),
    })
}

pub(crate) fn url_key(u: &str) -> Option<String> {
    normalise_url(u, None).map(|k| k.0)
}

/// Path part of a key (from the first `/`).
pub(crate) fn key_path(k: &str) -> &str {
    &k[k.find('/').unwrap_or(k.len())..]
}

/// Posts whose parent chain comes back round to them.
fn parent_cycles(parent: &HashMap<u64, u64>) -> HashSet<u64> {
    let mut state: HashMap<u64, u8> = HashMap::new();
    let mut on_cycle = HashSet::new();
    for &start in parent.keys() {
        let mut path = vec![];
        let mut cur = start;
        loop {
            match state.get(&cur) {
                Some(2) => break,
                Some(_) => {
                    if let Some(pos) = path.iter().position(|x| *x == cur) {
                        on_cycle.extend(path[pos..].iter().copied());
                    }
                    break;
                }
                None => {}
            }
            state.insert(cur, 1);
            path.push(cur);
            match parent.get(&cur) {
                Some(&p) => cur = p,
                None => break,
            }
        }
        for x in path {
            state.insert(x, 2);
        }
    }
    on_cycle
}

/// Everything pass B reads.
struct Shared<'a> {
    raw: &'a Raw,
    site: &'a Site,
    media_ids: &'a HashMap<u64, ItemId>,
    docs: &'a HashMap<u64, ItemId>,
    term_ids: &'a HashMap<(String, u64), ItemId>,
    globals: HashMap<u64, String>,
    reusable: HashMap<u64, String>,
    defs: acf::Defs,
    /// post id → (locale, translation group).
    groups: HashMap<u64, (Option<String>, Vec<u64>)>,
}

impl Shared<'_> {
    fn term(&self, tax: &str, key: &TermKey) -> Option<ItemId> {
        let sid = self.raw.term_sid(tax, key)?;
        self.raw.terms.get(&sid)?;
        Some(canon(ItemKind::Term, &sid, None).0)
    }
}

fn excerpt_text(s: &str) -> String {
    static TAGS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"<[^>]*>").expect("valid regex"));
    decode_entities(&TAGS.replace_all(s, " "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pass B for one document: content, excerpt, featured image, terms, ACF, SEO, translations.
fn convert(p: &Post, d: &mut Document, sh: &Shared, store: &Store, issues: &mut Vec<PendingIssue>) {
    let mut html = HtmlCtx {
        store,
        site_hosts: sh.site.hosts.clone(),
        issues,
        media_alternates: media::alternates,
    };
    let content = p.content.as_deref().unwrap_or("");
    let body = if content.contains("[et_pb_") {
        FieldValue::Layout(divi::divi_to_layout(
            content,
            &sh.globals,
            sh.media_ids,
            &mut html,
        ))
    } else if content.contains("<!-- wp:") {
        FieldValue::Layout(gutenberg::gutenberg_to_layout(
            content,
            &sh.reusable,
            &mut html,
        ))
    } else {
        FieldValue::RichText(classic_to_richtext(content, sh.media_ids, &mut html))
    };
    d.fields.insert("content".into(), body);
    let excerpt = excerpt_text(p.excerpt.as_deref().unwrap_or(""));
    if !excerpt.is_empty() {
        d.fields
            .insert("excerpt".into(), FieldValue::Text(excerpt.clone()));
    }
    if let Some(f) = p.featured.filter(|f| *f > 0) {
        match sh.media_ids.get(&f) {
            Some(m) => {
                d.fields
                    .insert("featured_image".into(), FieldValue::Media(*m));
            }
            None => html.issue(
                IssueKind::MissingMedia,
                Severity::Warning,
                format!("featured image (attachment {f}) is not in the media library"),
                json!({ "attachment": f }),
            ),
        }
    }
    let mut by_tax: BTreeMap<&str, Vec<FieldValue>> = BTreeMap::new();
    for (tax, key) in &p.terms {
        match sh.term(tax, key) {
            Some(t) => by_tax
                .entry(tax.as_str())
                .or_default()
                .push(FieldValue::Doc(t)),
            None => html.issue(
                IssueKind::UnresolvedRelation,
                Severity::Warning,
                format!("{tax} term {key:?} not found"),
                json!({ "taxonomy": tax }),
            ),
        }
    }
    for (tax, list) in by_tax {
        d.fields
            .insert(format!("terms.{tax}"), FieldValue::List(list));
    }
    let term = |tax: &str, id: u64| sh.term_ids.get(&(tax.to_string(), id)).copied();
    let mut cx = acf::AcfCx {
        html: &mut html,
        media_ids: sh.media_ids,
        docs: sh.docs,
        term: &term,
    };
    for (k, v) in acf::fields(&sh.defs, p.acf.as_ref(), &p.meta, &mut cx) {
        d.fields.insert(k, v);
    }
    let seo_site = seo::SeoSite {
        site_name: &sh.site.name,
        titles: &sh.site.titles,
    };
    let crawl = p
        .link
        .as_deref()
        .and_then(url_key)
        .and_then(|k| sh.raw.crawl_heads.get(&k));
    let input = seo::SeoIn {
        post_type: &p.post_type,
        meta: &p.meta,
        vars: seo::Vars {
            title: &d.title,
            excerpt: &excerpt,
        },
        rest_head: p.head.as_ref(),
        crawl,
    };
    let mut unknown = BTreeSet::new();
    let media_url = |u: &str| store.media_by_url(u).ok().flatten();
    d.seo = seo::seo(&input, &seo_site, sh.media_ids, &media_url, &mut unknown);
    for var in unknown {
        html.issue(
            IssueKind::UnmappedField,
            Severity::Info,
            format!("SEO variable {var} has no equivalent; removed"),
            json!({ "variable": var }),
        );
    }
    if let Some((locale, group)) = sh.groups.get(&p.id) {
        d.locale = d.locale.clone().or_else(|| locale.clone());
        d.translations = group
            .iter()
            .filter(|g| **g != p.id)
            .filter_map(|g| sh.docs.get(g).copied())
            .collect();
    }
}

/// Translation groups from WPML rows, Polylang groups, or REST `translations`.
fn translation_groups(raw: &Raw) -> HashMap<u64, (Option<String>, Vec<u64>)> {
    let mut out: HashMap<u64, (Option<String>, Vec<u64>)> = HashMap::new();
    let t = &raw.translations;
    let num = |v: &Value| v.as_u64().or_else(|| v.as_str()?.parse().ok());
    match t["plugin"].as_str() {
        Some("wpml") => {
            let mut by_trid: BTreeMap<u64, Vec<(u64, Option<String>)>> = BTreeMap::new();
            for r in t["rows"].as_array().into_iter().flatten() {
                if !r["element_type"]
                    .as_str()
                    .is_some_and(|e| e.starts_with("post_"))
                {
                    continue;
                }
                let (Some(id), Some(trid)) = (num(&r["element_id"]), num(&r["trid"])) else {
                    continue;
                };
                let lang = r["language_code"].as_str().map(str::to_string);
                by_trid.entry(trid).or_default().push((id, lang));
            }
            for members in by_trid.values() {
                let ids: Vec<u64> = members.iter().map(|m| m.0).collect();
                for (id, lang) in members {
                    out.insert(*id, (lang.clone(), ids.clone()));
                }
            }
        }
        Some("polylang") => {
            for g in t["groups"].as_array().into_iter().flatten() {
                let Some(o) = g.as_object() else { continue };
                let ids: Vec<u64> = o.values().filter_map(num).collect();
                for (lang, id) in o {
                    if let Some(id) = num(id) {
                        out.insert(id, (Some(lang.clone()), ids.clone()));
                    }
                }
            }
        }
        _ => {}
    }
    for p in raw.posts.values() {
        if !p.translations.is_empty() && !out.contains_key(&p.id) {
            let mut ids = p.translations.clone();
            ids.push(p.id);
            out.insert(p.id, (None, ids));
        }
    }
    out
}

/// Writes items and statuses, and remembers which connector rows each item came from.
pub(crate) struct Out<'a> {
    pub ctx: &'a StageCtx,
    pub store: &'a Store,
    /// Connector row → the canonical item it became part of.
    pub prov: BTreeMap<ItemId, ItemId>,
    /// Items given a status this run.
    pub statused: HashSet<ItemId>,
    /// Items written this run.
    pub written: HashSet<ItemId>,
}

impl Out<'_> {
    pub fn put<T: Serialize>(
        &mut self,
        kind: ItemKind,
        (id, src): &(ItemId, SourceRef),
        data: &T,
    ) -> Result<()> {
        self.written.insert(*id);
        self.store.put_item(kind, *id, src, data)
    }

    /// Soft-deletes canonical items of `kinds` an earlier run made and this one didn't, so
    /// they stop answering lookups, with `Skipped { "no longer in the source" }`.
    fn drop_stale(&mut self, kinds: &[ItemKind]) -> Result<()> {
        for (id, kind, connector) in self.store.item_rows()? {
            let kind: ItemKind = serde_json::from_value(json!(kind))?;
            if connector != CONNECTOR || !kinds.contains(&kind) || self.written.contains(&id) {
                continue;
            }
            self.store.soft_delete(id)?;
            let reason = "no longer in the source".to_string();
            self.status(kind, id, ItemStatus::Skipped { reason }, [])?;
        }
        Ok(())
    }

    /// Sets `id`'s status and maps `raw` rows to it.
    pub fn status<'r>(
        &mut self,
        kind: ItemKind,
        id: ItemId,
        st: ItemStatus,
        raw: impl IntoIterator<Item = &'r ItemId>,
    ) -> Result<()> {
        self.ctx.item_status(id, kind, st)?;
        self.statused.insert(id);
        for r in raw {
            self.prov.insert(*r, id);
        }
        Ok(())
    }

    pub fn issue(
        &self,
        kind: IssueKind,
        sev: Severity,
        id: ItemId,
        msg: String,
        details: Value,
    ) -> Result<()> {
        self.ctx.issue(kind, sev, Some(id), msg, details)
    }

    pub fn pending(&self, id: ItemId, issues: Vec<PendingIssue>) -> Result<()> {
        for i in issues {
            self.issue(i.kind, i.severity, id, i.message, i.details)?;
        }
        Ok(())
    }
}

fn normalise(ctx: &StageCtx) -> Result<()> {
    let store = ctx.project.store();
    let raw = load(ctx)?;
    let mut out = Out {
        ctx,
        store,
        prov: BTreeMap::new(),
        statused: HashSet::new(),
        written: HashSet::new(),
    };
    let cancelled = || ctx.cancel.is_cancelled();

    // Pass A: what links, media and relations resolve against.
    let media_ids =
        media::attachments(ctx, &raw.posts, &raw.site, &mut out.prov, &mut out.statused)?;
    let authors = authors(&mut out, &raw)?;
    let term_ids = terms(&mut out, &raw)?;
    let mut sk = skeletons(&mut out, &raw, &authors)?;
    if cancelled() {
        return Ok(());
    }
    let redirect_by_key = redirects::write(&mut out, &raw, &mut sk)?;
    redirects::routes(&mut out, &raw, &mut sk, &redirect_by_key, &media_ids)?;
    // Before content is converted, so links can't resolve to what is gone.
    out.drop_stale(&[ItemKind::Document, ItemKind::Route, ItemKind::Redirect])?;
    if cancelled() {
        return Ok(());
    }

    // Pass B: content.
    if !documents(&mut out, &raw, &mut sk, &media_ids, &term_ids)? {
        return Ok(());
    }
    let menu_of_item = raw.menu_of_items();
    for (wp, why) in &sk.skipped {
        let p = &raw.posts[wp];
        let reason = match (p.post_type.as_str(), menu_of_item.get(wp)) {
            ("nav_menu_item", Some(m)) => format!("part of menu {m}"),
            ("nav_menu_item", None) => "menu item of no known menu".into(),
            _ => why.clone(),
        };
        out.status(
            ItemKind::Document,
            post_id(*wp),
            ItemStatus::Skipped { reason },
            &p.raw,
        )?;
    }
    menus(&mut out, &raw, &sk.docs)?;
    if !forms::write(&mut out, &raw)? {
        return Ok(());
    }
    settings(&mut out, &raw, &sk.docs)?;
    account(&mut out, &raw)?;
    let total = sk.docs.len() as u64;
    ctx.progress(total, Some(total), "documents");
    Ok(())
}

fn authors(out: &mut Out, raw: &Raw) -> Result<HashMap<String, ItemId>> {
    let mut ids = HashMap::new();
    for (login, a) in &raw.authors {
        let c = canon(ItemKind::Author, &format!("author:{login}"), None);
        out.put(ItemKind::Author, &c, &a.author)?;
        out.status(ItemKind::Author, c.0, ItemStatus::Done, &a.raw)?;
        ids.insert(login.clone(), c.0);
    }
    Ok(ids)
}

/// Taxonomies and terms; returns `(taxonomy, term id)` → term item.
fn terms(out: &mut Out, raw: &Raw) -> Result<HashMap<(String, u64), ItemId>> {
    for (name, hier) in &raw.taxonomies {
        let c = canon(ItemKind::Taxonomy, &format!("taxonomy:{name}"), None);
        let t = Taxonomy {
            name: name.clone(),
            hierarchical: *hier,
        };
        out.put(ItemKind::Taxonomy, &c, &t)?;
        out.status(ItemKind::Taxonomy, c.0, ItemStatus::Done, [])?;
    }
    let mut ids = HashMap::new();
    for (sid, t) in &raw.terms {
        let c = canon(ItemKind::Term, sid, None);
        if let Some(n) = t.id {
            ids.insert((t.taxonomy.clone(), n), c.0);
        }
        let parent = match &t.parent {
            None => None,
            Some(k) => match raw
                .term_sid(&t.taxonomy, k)
                .filter(|p| raw.terms.contains_key(p))
            {
                Some(p) if p != *sid => Some(canon(ItemKind::Term, &p, None).0),
                _ => {
                    out.issue(
                        IssueKind::ParentMissing,
                        Severity::Warning,
                        c.0,
                        format!(
                            "{} term {} has a parent ({k:?}) that was not found",
                            t.taxonomy, t.slug
                        ),
                        json!({ "taxonomy": t.taxonomy, "parent": format!("{k:?}") }),
                    )?;
                    None
                }
            },
        };
        let term = Term {
            taxonomy: t.taxonomy.clone(),
            name: t.name.clone(),
            slug: t.slug.clone(),
            parent,
        };
        out.put(ItemKind::Term, &c, &term)?;
        out.status(ItemKind::Term, c.0, ItemStatus::Done, &t.raw)?;
    }
    Ok(ids)
}

/// Pass A's documents: who is one, their parents, paths and the URLs they live at.
pub(crate) struct Skeletons {
    /// WordPress id → document.
    pub docs: HashMap<u64, ItemId>,
    /// Posts that are not documents, and why.
    pub skipped: Vec<(u64, String)>,
    /// Documents whose parent was cleared.
    pub parent_issues: HashMap<u64, PendingIssue>,
    pub skeletons: BTreeMap<u64, Document>,
    /// URL key → document; path part of the key → document.
    pub doc_keys: HashMap<String, ItemId>,
    pub doc_paths: HashMap<String, ItemId>,
    pub routes: BTreeMap<String, Route>,
}

fn skeletons(out: &mut Out, raw: &Raw, authors: &HashMap<String, ItemId>) -> Result<Skeletons> {
    let mut docs: HashMap<u64, ItemId> = HashMap::new();
    let mut skipped = vec![];
    // A page and a post at one URL: WordPress resolves the path to the page first, so the
    // post never showed there (only a page that is live wins; a draft page doesn't).
    let live_page =
        |p: &Post| p.post_type == "page" && matches!(p.status.as_str(), "publish" | "private");
    let page_at: HashMap<String, u64> = raw
        .posts
        .values()
        .filter(|p| live_page(p))
        .filter_map(|p| Some((url_key(p.link.as_deref()?)?, p.id)))
        .collect();
    for p in raw.posts.values().filter(|p| p.post_type != "attachment") {
        let shadow = (p.post_type == "post")
            .then(|| page_at.get(&url_key(p.link.as_deref()?)?))
            .flatten();
        match (not_a_document(p), shadow) {
            (Some(why), _) => skipped.push((p.id, why)),
            (None, Some(page)) => {
                skipped.push((p.id, format!("page {page} is served at the same URL")))
            }
            (None, None) => {
                docs.insert(p.id, post_id(p.id));
            }
        }
    }
    // Parents: missing ones and cycles are cleared, with an issue each.
    let parent_of: HashMap<u64, u64> = docs
        .keys()
        .filter_map(|id| {
            Some((
                *id,
                raw.posts[id].parent_id().filter(|q| docs.contains_key(q))?,
            ))
        })
        .collect();
    let cycles = parent_cycles(&parent_of);
    let mut parent_issues: HashMap<u64, PendingIssue> = HashMap::new();
    for id in docs.keys() {
        let Some(q) = raw.posts[id].parent_id() else {
            continue;
        };
        let why = if !docs.contains_key(&q) {
            format!("parent post {q} is not among the documents")
        } else if cycles.contains(id) {
            format!("parent chain through post {q} loops back to this post")
        } else {
            continue;
        };
        parent_issues.insert(
            *id,
            PendingIssue {
                kind: IssueKind::ParentMissing,
                severity: Severity::Warning,
                message: why,
                details: json!({ "parent": q }),
            },
        );
    }
    let parent_ok = |id: u64| -> Option<u64> {
        let q = raw.posts.get(&id)?.parent_id()?;
        (!parent_issues.contains_key(&id)).then_some(q)
    };
    // The permalink's path; without a pretty permalink, the parents' slugs.
    let path_of = |id: u64| -> String {
        let p = &raw.posts[&id];
        let from_link = p
            .link
            .as_deref()
            .and_then(|l| Url::parse(l).ok())
            .filter(|u| u.query().is_none())
            .map(|u| u.path().to_string());
        from_link.unwrap_or_else(|| {
            let mut parts = vec![];
            let mut cur = Some(id);
            while let Some(c) = cur.filter(|_| parts.len() < MAX_PARENTS) {
                let Some(pp) = raw.posts.get(&c) else { break };
                parts.push(if pp.slug.is_empty() {
                    pp.id.to_string()
                } else {
                    pp.slug.clone()
                });
                cur = parent_ok(c);
            }
            parts.reverse();
            format!("/{}/", parts.join("/"))
        })
    };

    let mut sk = Skeletons {
        docs: HashMap::new(),
        skipped,
        parent_issues: HashMap::new(),
        skeletons: BTreeMap::new(),
        doc_keys: HashMap::new(),
        doc_paths: HashMap::new(),
        routes: BTreeMap::new(),
    };
    for (&wp, &id) in &docs {
        let p = &raw.posts[&wp];
        let c = canon(ItemKind::Document, &format!("post:{wp}"), p.link.clone());
        let status = doc_status(&p.status).unwrap_or(DocStatus::Draft);
        let d = Document {
            id,
            doc_type: p.post_type.clone(),
            status,
            locale: p.lang.clone(),
            translations: vec![],
            slug: p.slug.clone(),
            path: path_of(wp),
            parent: parent_ok(wp).map(post_id),
            title: decode_entities(&p.title),
            fields: IndexMap::new(),
            seo: Seo::default(),
            created: p.created,
            updated: p.updated,
            author: p.author.as_ref().and_then(|a| authors.get(a)).copied(),
            original: json!({
                "meta": p.meta, "menu_order": p.menu_order, "template": p.template,
                "wp_status": p.status,
            }),
            source: c.1.clone(),
        };
        out.put(ItemKind::Document, &c, &d)?;
        if let Some(k) = p
            .link
            .as_deref()
            .filter(|l| !l.contains('?'))
            .and_then(url_key)
        {
            sk.doc_paths.entry(key_path(&k).to_string()).or_insert(id);
            sk.doc_keys.insert(k.clone(), id);
            if status == DocStatus::Published {
                let r = Route {
                    url: p.link.clone().unwrap_or_default(),
                    target: RouteTarget::Document(id),
                    http_status: None,
                    title: Some(d.title.clone()),
                };
                sk.routes.insert(k, r);
            }
        }
        sk.skeletons.insert(wp, d);
    }
    sk.docs = docs;
    sk.parent_issues = parent_issues;
    Ok(sk)
}

/// Pass B: converts every document, then writes them all. False when cancelled.
fn documents(
    out: &mut Out,
    raw: &Raw,
    sk: &mut Skeletons,
    media_ids: &HashMap<u64, ItemId>,
    term_ids: &HashMap<(String, u64), ItemId>,
) -> Result<bool> {
    let pick = |ty: &str| -> HashMap<u64, String> {
        raw.posts
            .values()
            .filter(|p| p.post_type == ty)
            .map(|p| (p.id, p.content.clone().unwrap_or_default()))
            .collect()
    };
    let sh = Shared {
        raw,
        site: &raw.site,
        media_ids,
        docs: &sk.docs,
        term_ids,
        globals: pick("et_pb_layout"),
        reusable: pick("wp_block"),
        defs: acf::Defs::load(&raw.acf_groups, raw.posts.values()),
        groups: translation_groups(raw),
    };
    // Converted first, written after: writing a document resets the URL index lookups use.
    let total = sk.skeletons.len() as u64;
    let mut finished: Vec<(u64, Document, Vec<PendingIssue>, ItemStatus)> = vec![];
    for (n, (wp, mut d)) in std::mem::take(&mut sk.skeletons).into_iter().enumerate() {
        if out.ctx.cancel.is_cancelled() {
            return Ok(false);
        }
        if n % 200 == 0 {
            out.ctx.progress(n as u64, Some(total), "documents");
        }
        let mut issues: Vec<PendingIssue> = sk.parent_issues.remove(&wp).into_iter().collect();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut local = vec![];
            convert(&raw.posts[&wp], &mut d, &sh, out.store, &mut local);
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
                    details: json!({ "post": wp }),
                });
                ItemStatus::Failed { error }
            }
        };
        finished.push((wp, d, issues, st));
    }
    for (wp, d, issues, st) in finished {
        out.store
            .put_item(ItemKind::Document, d.id, &d.source, &d)?;
        out.pending(d.id, issues)?;
        out.status(ItemKind::Document, d.id, st, &raw.posts[&wp].raw)?;
    }
    Ok(true)
}

fn menus(out: &mut Out, raw: &Raw, docs: &HashMap<u64, ItemId>) -> Result<()> {
    for (mid, m) in &raw.menus {
        let c = canon(ItemKind::Menu, &format!("menu:{mid}"), None);
        let mut issues = vec![];
        // Items whose parents loop round sit at the top instead.
        let parents: HashMap<u64, u64> = m
            .items
            .iter()
            .filter_map(|i| Some((i.id, i.parent?)))
            .collect();
        let looped = parent_cycles(&parents);
        let mut items = m.items.clone();
        for i in items.iter_mut().filter(|i| looped.contains(&i.id)) {
            issues.push((
                format!(
                    "menu item \"{}\" is its own ancestor; moved to the top",
                    i.title
                ),
                json!({ "menu_item": i.id }),
            ));
            i.parent = None;
        }
        let menu = Menu {
            name: m.name.clone(),
            items: menu_tree(&items, None, docs, raw, &mut issues),
        };
        out.put(ItemKind::Menu, &c, &menu)?;
        for (msg, details) in issues {
            out.issue(
                IssueKind::UnresolvedRelation,
                Severity::Warning,
                c.0,
                msg,
                details,
            )?;
        }
        out.status(ItemKind::Menu, c.0, ItemStatus::Done, &m.raw)?;
    }
    Ok(())
}

fn settings(out: &mut Out, raw: &Raw, docs: &HashMap<u64, ItemId>) -> Result<()> {
    let site = &raw.site;
    let c = canon(ItemKind::Settings, "settings", None);
    let seo_site = seo::SeoSite {
        site_name: &site.name,
        titles: &site.titles,
    };
    let mut unknown = BTreeSet::new();
    let settings = Settings {
        site_name: site.name.clone(),
        front_page: site.front_page.and_then(|f| docs.get(&f)).copied(),
        permalink_structure: site.permalink.clone(),
        seo_defaults: seo::defaults(&seo_site, &site.tagline, &mut unknown),
    };
    out.put(ItemKind::Settings, &c, &settings)?;
    if let Some(f) = site.front_page.filter(|f| !docs.contains_key(f)) {
        out.issue(
            IssueKind::UnresolvedRelation,
            Severity::Warning,
            c.0,
            format!("front page (post {f}) is not among the documents"),
            json!({ "page_on_front": f }),
        )?;
    }
    for var in unknown {
        out.issue(
            IssueKind::UnmappedField,
            Severity::Info,
            c.0,
            format!("SEO variable {var} has no equivalent; removed"),
            json!({ "variable": var }),
        )?;
    }
    out.status(ItemKind::Settings, c.0, ItemStatus::Done, &raw.settings_raw)
}

/// Media no attachment claimed are done as they are; connector rows that fed nothing get
/// their own `Skipped`; the row → item map goes to `raw/normalise/provenance.json`.
fn account(out: &mut Out, raw: &Raw) -> Result<()> {
    out.drop_stale(&[
        ItemKind::Author,
        ItemKind::Taxonomy,
        ItemKind::Term,
        ItemKind::Menu,
        ItemKind::Form,
        ItemKind::Submission,
        ItemKind::Settings,
    ])?;
    for (id, src, _) in out.store.items::<Value>(ItemKind::Media)? {
        if src.connector == "media" && !out.statused.contains(&id) {
            out.status(ItemKind::Media, id, ItemStatus::Done, [])?;
        }
    }
    for (id, kind, connector) in out.store.item_rows()? {
        let kind_: ItemKind = serde_json::from_value(json!(kind))?;
        if matches!(connector.as_str(), CONNECTOR | "media") || out.prov.contains_key(&id) {
            continue;
        }
        let reason = match raw.notes.get(&id) {
            Some(n) => n.clone(),
            None if !configured(&out.ctx.config, &connector) => {
                format!("connector {connector} is no longer configured")
            }
            None => format!(
                "{kind} row from {connector} has nothing to normalise (failed or unreadable in an earlier stage)"
            ),
        };
        out.ctx
            .item_status(id, kind_, ItemStatus::Skipped { reason })?;
    }
    let dir = out.ctx.project.raw_dir("normalise");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("provenance.json"),
        serde_json::to_vec_pretty(&out.prov)?,
    )?;
    Ok(())
}

/// Menu items under `parent`, ordered; targets resolved to documents where possible.
fn menu_tree(
    items: &[super::load::MenuItemRec],
    parent: Option<u64>,
    docs: &HashMap<u64, ItemId>,
    raw: &Raw,
    issues: &mut Vec<(String, Value)>,
) -> Vec<MenuItem> {
    let ids: HashSet<u64> = items.iter().map(|i| i.id).collect();
    let mut mine: Vec<&super::load::MenuItemRec> = items
        .iter()
        .filter(|i| {
            // An item whose parent is not in the menu sits at the top.
            let p = i.parent.filter(|p| ids.contains(p) && *p != i.id);
            p == parent
        })
        .collect();
    mine.sort_by_key(|i| (i.order, i.id));
    mine.into_iter()
        .map(|i| {
            let target = match (i.kind.as_str(), i.object_id.and_then(|o| docs.get(&o))) {
                ("post_type", Some(d)) => MenuTarget::Doc(*d),
                ("post_type", None) => {
                    issues.push((
                        format!(
                            "menu item \"{}\" points at {} {} which is not among the documents",
                            i.title,
                            i.object,
                            i.object_id.unwrap_or(0)
                        ),
                        json!({ "menu_item": i.id, "object_id": i.object_id }),
                    ));
                    MenuTarget::Url(i.url.clone())
                }
                _ => MenuTarget::Url(i.url.clone()),
            };
            let label = if i.title.is_empty() {
                i.object_id
                    .and_then(|o| raw.posts.get(&o))
                    .map(|p| decode_entities(&p.title))
                    .unwrap_or_default()
            } else {
                decode_entities(&i.title)
            };
            let children = menu_tree(items, Some(i.id), docs, raw, issues);
            MenuItem {
                label,
                target,
                children,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_found() {
        let p = HashMap::from([(1, 2), (2, 1), (3, 1), (4, 4), (5, 6)]);
        let c = parent_cycles(&p);
        assert_eq!(c, HashSet::from([1, 2, 4]));
    }
}
