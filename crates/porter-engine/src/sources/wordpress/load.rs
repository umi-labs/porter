//! Reads what the WordPress connectors left under `raw/` and in their RawItem rows, and merges
//! it per WordPress id: wp-db first, then wp-rest, then wxr fill what is still missing.

use super::db::fold_meta;
use super::normalise::{Post, Site, TermKey};
use super::wxr::{WxrAuthor, WxrItem, WxrTerm};
use crate::config::ConnectorConfig;
use crate::connectors::RawItem;
use crate::connectors::crawl::PageHead;
use crate::html::decode_entities;
use crate::stage::StageCtx;
use crate::url::normalise_url;
use ::url::Url;
use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use porter_model::{Author, IssueKind, ItemId, ItemKind, Severity, SourceRef};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::Path;

pub(crate) struct AuthorRec {
    pub author: Author,
    pub raw: Vec<ItemId>,
}

pub(crate) struct TermRec {
    pub taxonomy: String,
    pub id: Option<u64>,
    pub slug: String,
    pub name: String,
    pub parent: Option<TermKey>,
    pub raw: Vec<ItemId>,
}

#[derive(Clone, Debug)]
pub(crate) struct MenuItemRec {
    pub id: u64,
    pub parent: Option<u64>,
    pub order: i64,
    pub title: String,
    pub url: String,
    /// `post_type`, `taxonomy`, `custom`…
    pub kind: String,
    pub object: String,
    pub object_id: Option<u64>,
}

#[derive(Default)]
pub(crate) struct MenuRec {
    pub name: String,
    pub slug: String,
    pub items: Vec<MenuItemRec>,
    pub raw: Vec<ItemId>,
}

pub(crate) struct Raw {
    pub posts: BTreeMap<u64, Post>,
    /// login → author.
    pub authors: BTreeMap<String, AuthorRec>,
    /// Canonical term source id (`term:<taxonomy>:<id or slug>`) → term.
    pub terms: BTreeMap<String, TermRec>,
    /// `(taxonomy, "id:<n>" | "slug:<s>")` → canonical term source id.
    term_ix: HashMap<(String, String), String>,
    /// Name → hierarchical.
    pub taxonomies: BTreeMap<String, bool>,
    pub menus: BTreeMap<u64, MenuRec>,
    pub gf: Vec<(Value, Vec<ItemId>)>,
    pub nf: Vec<(Value, Vec<ItemId>)>,
    pub submissions: Vec<(Value, Option<ItemId>)>,
    /// Redirection plugin rows.
    pub redirection: Vec<(Value, Option<ItemId>)>,
    pub acf_groups: Vec<Value>,
    pub translations: Value,
    /// Crawled `<head>`s by URL key.
    pub crawl_heads: HashMap<String, PageHead>,
    /// Crawled URL key → WordPress id from its body classes.
    pub page_ids: HashMap<String, u64>,
    pub site: Site,
    pub settings_raw: Vec<ItemId>,
    /// Why a connector row fed nothing, when known.
    pub notes: HashMap<ItemId, String>,
    /// A crawl connector is configured (its store rows count).
    pub crawl: bool,
}

impl Raw {
    /// Canonical source id of a term reference.
    pub fn term_sid(&self, tax: &str, key: &TermKey) -> Option<String> {
        let k = match key {
            TermKey::Id(n) => format!("id:{n}"),
            TermKey::Slug(s) => format!("slug:{s}"),
        };
        self.term_ix.get(&(tax.to_string(), k)).cloned()
    }

    /// `nav_menu_item` post id → menu id.
    pub fn menu_of_items(&self) -> HashMap<u64, u64> {
        self.menus
            .iter()
            .flat_map(|(m, r)| r.items.iter().map(move |i| (i.id, *m)))
            .collect()
    }

    fn add_term(&mut self, t: TermRec) {
        let sid = match t.id {
            Some(n) => format!("term:{}:{n}", t.taxonomy),
            None => format!("term:{}:{}", t.taxonomy, t.slug),
        };
        // Known by slug from another connector (WXR without ids): same term.
        let sid = self
            .term_ix
            .get(&(t.taxonomy.clone(), format!("slug:{}", t.slug)))
            .cloned()
            .unwrap_or(sid);
        if let Some(n) = t.id {
            self.term_ix
                .insert((t.taxonomy.clone(), format!("id:{n}")), sid.clone());
        }
        self.term_ix
            .entry((t.taxonomy.clone(), format!("slug:{}", t.slug)))
            .or_insert_with(|| sid.clone());
        self.taxonomies.entry(t.taxonomy.clone()).or_insert(false);
        match self.terms.get_mut(&sid) {
            Some(cur) => {
                cur.raw.extend(t.raw);
                if cur.parent.is_none() {
                    cur.parent = t.parent;
                }
                if cur.id.is_none() {
                    cur.id = t.id;
                }
            }
            None => {
                self.terms.insert(sid, t);
            }
        }
    }

    fn add_post(&mut self, p: Post) {
        match self.posts.get_mut(&p.id) {
            Some(cur) => cur.fill(p),
            None => {
                self.posts.insert(p.id, p);
            }
        }
    }

    fn add_author(&mut self, login: String, a: Author, raw: Option<ItemId>) {
        let e = self.authors.entry(login).or_insert(AuthorRec {
            author: a,
            raw: vec![],
        });
        e.raw.extend(raw);
    }
}

fn row_id(kind: ItemKind, connector: &str, sid: &str) -> ItemId {
    ItemId::new(
        kind,
        &SourceRef {
            connector: connector.into(),
            source_id: sid.into(),
            url: None,
        },
    )
}

fn read_json<T: DeserializeOwned>(p: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn str_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn s(v: &Value, k: &str) -> String {
    str_of(&v[k]).unwrap_or_default()
}

fn opt(v: &Value, k: &str) -> Option<String> {
    str_of(&v[k]).filter(|x| !x.is_empty())
}

fn num(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `0` for "none" (null, 0, ""), so a connector that knows there is none says so.
fn known(v: &Value) -> Option<u64> {
    match v {
        Value::Null => Some(0),
        Value::String(s) if s.is_empty() => Some(0),
        other => num(other),
    }
}

pub(crate) fn wp_date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() || s.starts_with("0000") {
        return None;
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            return Some(d.and_utc());
        }
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn obj(v: &Value) -> Option<Map<String, Value>> {
    v.as_object().cloned()
}

// ---------- wp-db ----------

fn db_post(v: &Value, logins: &HashMap<u64, String>) -> Option<Post> {
    let id = num(&v["id"])?;
    let ty = s(v, "type");
    let raw_kind = if ty == "attachment" {
        "attachment"
    } else {
        "post"
    };
    let meta = obj(&v["meta"]).unwrap_or_default();
    let terms = v["terms"]
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(tax, ids)| {
            ids.as_array()
                .into_iter()
                .flatten()
                .filter_map(num)
                .map(move |n| (tax.clone(), TermKey::Id(n)))
        })
        .collect();
    let att = &v["attachment"];
    Some(Post {
        id,
        status: s(v, "status"),
        slug: s(v, "slug"),
        title: s(v, "title"),
        link: opt(v, "link"),
        parent: known(&v["parent"]),
        menu_order: v["menu_order"].as_i64(),
        author: num(&v["author"]).and_then(|a| logins.get(&a).cloned()),
        created: wp_date(&s(v, "date_gmt")),
        updated: wp_date(&s(v, "modified_gmt")),
        content: str_of(&v["content"]),
        excerpt: str_of(&v["excerpt"]),
        acf: obj(&v["acf"]),
        terms,
        featured: known(&v["featured_image"]),
        mime: opt(v, "mime"),
        file: opt(att, "file").or_else(|| meta.get("_wp_attached_file").and_then(str_of)),
        file_meta: Some(att["metadata"].clone())
            .filter(|m| m.is_object())
            .or_else(|| meta.get("_wp_attachment_metadata").cloned()),
        alt: meta.get("_wp_attachment_image_alt").and_then(str_of),
        caption: (ty == "attachment").then(|| s(v, "excerpt")),
        lang: opt(v, "lang"),
        template: opt(v, "template"),
        raw: vec![row_id(
            ItemKind::Document,
            "wp-db",
            &format!("{raw_kind}:{id}"),
        )],
        meta,
        post_type: ty,
        ..Post::default()
    })
}

fn ndjson(p: &Path) -> Vec<Value> {
    let Ok(f) = std::fs::File::open(p) else {
        return vec![];
    };
    std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

/// Err when the export can't be read (the caller reports it).
fn load_db(r: &mut Raw, dir: &Path, site: &mut SiteParts) -> Result<(), String> {
    let bytes = std::fs::read(dir.join("export.json")).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    drop(bytes);
    let db = |kind, sid: String| row_id(kind, "wp-db", &sid);
    let mut logins = HashMap::new();
    for u in v["users"].as_array().into_iter().flatten() {
        let (Some(id), Some(login)) = (num(&u["id"]), opt(u, "login")) else {
            continue;
        };
        logins.insert(id, login.clone());
        let a = Author {
            name: opt(u, "display_name").unwrap_or_else(|| login.clone()),
            email: opt(u, "email"),
            slug: opt(u, "nicename").unwrap_or_else(|| login.clone()),
        };
        r.add_author(login, a, Some(db(ItemKind::Author, format!("user:{id}"))));
    }
    for p in v["posts"].as_array().into_iter().flatten() {
        if let Some(p) = db_post(p, &logins) {
            r.add_post(p);
        }
    }
    if let Some(t) = v["taxonomies"].as_object() {
        for (name, def) in t {
            if name != "nav_menu" {
                r.taxonomies
                    .insert(name.clone(), def["hierarchical"] == true);
            }
        }
    }
    for t in v["terms"].as_array().into_iter().flatten() {
        let (tax, id) = (s(t, "taxonomy"), num(&t["id"]));
        if tax == "nav_menu" || tax.is_empty() {
            continue;
        }
        r.add_term(TermRec {
            raw: id
                .map(|n| db(ItemKind::Term, format!("term:{tax}:{n}")))
                .into_iter()
                .collect(),
            taxonomy: tax,
            id,
            slug: s(t, "slug"),
            name: decode_entities(&s(t, "name")),
            parent: num(&t["parent"]).filter(|n| *n > 0).map(TermKey::Id),
        });
    }
    for m in v["menus"].as_array().into_iter().flatten() {
        let Some(id) = num(&m["id"]) else { continue };
        let items = m["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| {
                Some(MenuItemRec {
                    id: num(&i["id"])?,
                    parent: num(&i["parent"]).filter(|n| *n > 0),
                    order: i["order"].as_i64().unwrap_or(0),
                    title: s(i, "title"),
                    url: s(i, "url"),
                    kind: s(i, "type"),
                    object: s(i, "object"),
                    object_id: num(&i["object_id"]),
                })
            })
            .collect();
        r.menus.insert(
            id,
            MenuRec {
                name: s(m, "name"),
                slug: s(m, "slug"),
                items,
                raw: vec![db(ItemKind::Menu, format!("menu:{id}"))],
            },
        );
    }
    for (kind, list) in [("gf", &mut r.gf), ("nf", &mut r.nf)] {
        for f in v["forms"][kind].as_array().into_iter().flatten() {
            let raw = str_of(&f["id"])
                .map(|id| db(ItemKind::Form, format!("form:{kind}:{id}")))
                .into_iter()
                .collect();
            list.push((f.clone(), raw));
        }
    }
    r.acf_groups = v["acf_field_groups"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    r.translations = v["translations"].clone();
    let st = &v["site"];
    site.names.push(opt(st, "blogname"));
    site.taglines.push(opt(st, "blogdescription"));
    site.urls.extend([opt(st, "home"), opt(st, "siteurl")]);
    site.front
        .push(num(&st["page_on_front"]).filter(|n| *n > 0));
    site.permalink.push(str_of(&st["permalink_structure"]));
    if st["wpseo_titles"].is_object() {
        site.titles = st["wpseo_titles"].clone();
    }
    r.settings_raw
        .push(db(ItemKind::Settings, "settings".into()));
    for row in ndjson(&dir.join("redirection.ndjson")) {
        let raw = str_of(&row["id"]).map(|id| db(ItemKind::Redirect, format!("redirect:{id}")));
        r.redirection.push((row, raw));
    }
    for row in ndjson(&dir.join("submissions.ndjson")) {
        let raw = str_of(&row["id"]).map(|id| {
            db(
                ItemKind::Submission,
                format!("submission:{}:{id}", s(&row, "source")),
            )
        });
        r.submissions.push((row, raw));
    }
    Ok(())
}

// ---------- wp-rest ----------

fn rendered(v: &Value, k: &str) -> Option<String> {
    v[k]["raw"]
        .as_str()
        .map(str::to_string)
        .or_else(|| v[k]["rendered"].as_str().map(str::to_string))
        .or_else(|| v[k].as_str().map(str::to_string))
}

fn rest_post(
    v: &Value,
    raw: ItemId,
    users: &HashMap<u64, String>,
    tax_bases: &HashMap<String, String>,
    label: &str,
) -> Option<Post> {
    let id = num(&v["id"])?;
    let is_media = label == "attachment";
    let terms = tax_bases
        .iter()
        .flat_map(|(base, tax)| {
            v[base]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(num)
                .map(move |n| (tax.clone(), TermKey::Id(n)))
        })
        .collect();
    let md = &v["media_details"];
    let title = match v["title"]["raw"].as_str() {
        Some(t) => t.to_string(),
        None => decode_entities(&rendered(v, "title").unwrap_or_default()),
    };
    Some(Post {
        id,
        post_type: if is_media {
            "attachment".into()
        } else {
            opt(v, "type").unwrap_or_else(|| label.to_string())
        },
        status: s(v, "status"),
        slug: s(v, "slug"),
        title,
        link: if is_media {
            opt(v, "source_url")
        } else {
            opt(v, "link")
        },
        parent: known(&v["parent"]).or(Some(0)),
        menu_order: v["menu_order"].as_i64(),
        author: num(&v["author"]).and_then(|a| users.get(&a).cloned()),
        created: wp_date(&s(v, "date_gmt")),
        updated: wp_date(&s(v, "modified_gmt")),
        content: v["content"]["raw"].as_str().map(str::to_string),
        content_rendered: v["content"]["rendered"].as_str().map(str::to_string),
        excerpt: rendered(v, "excerpt"),
        meta: obj(&v["meta"]).unwrap_or_default(),
        acf: obj(&v["acf"]).filter(|o| !o.is_empty()),
        terms,
        featured: known(&v["featured_media"]),
        mime: opt(v, "mime_type"),
        file: opt(md, "file"),
        file_meta: md.is_object().then(|| md.clone()),
        alt: opt(v, "alt_text"),
        caption: is_media
            .then(|| rendered(v, "caption"))
            .flatten()
            .map(|c| decode_entities(&c)),
        lang: opt(v, "lang"),
        translations: v["translations"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, x)| num(x))
            .filter(|x| *x != id)
            .collect(),
        head: v["yoast_head_json"]
            .is_object()
            .then(|| v["yoast_head_json"].clone()),
        template: opt(v, "template"),
        raw: vec![raw],
        ..Post::default()
    })
}

fn rows(
    ctx: &StageCtx,
    kind: ItemKind,
    connector: &str,
) -> Result<Vec<(ItemId, SourceRef, RawItem)>> {
    Ok(ctx
        .project
        .store()
        .items::<Value>(kind)?
        .into_iter()
        .filter(|(_, src, _)| src.connector == connector)
        .filter_map(|(id, src, v)| Some((id, src, serde_json::from_value(v).ok()?)))
        .collect())
}

fn load_rest(ctx: &StageCtx, r: &mut Raw, site: &mut SiteParts) -> Result<()> {
    let dir = ctx.project.raw_dir("wp-rest");
    let file = |row: &RawItem| -> Option<Value> { read_json(&dir.join(row.raw_path.as_deref()?)) };
    let mut users = HashMap::new();
    for (id, src, row) in rows(ctx, ItemKind::Author, "wp-rest")? {
        let Some(v) = file(&row) else {
            r.notes.insert(id, "raw file missing or unreadable".into());
            continue;
        };
        let Some(login) = opt(&v, "username").or_else(|| opt(&v, "slug")) else {
            continue;
        };
        if let Some(n) = src
            .source_id
            .rsplit_once(':')
            .and_then(|(_, n)| n.parse().ok())
        {
            users.insert(n, login.clone());
        }
        let a = Author {
            name: opt(&v, "name").unwrap_or_else(|| login.clone()),
            email: opt(&v, "email"),
            slug: opt(&v, "slug").unwrap_or_else(|| login.clone()),
        };
        r.add_author(login, a, Some(id));
    }
    let mut tax_bases: HashMap<String, String> = HashMap::from([
        ("categories".to_string(), "category".to_string()),
        ("tags".to_string(), "post_tag".to_string()),
    ]);
    if let Some(a) = read_json::<Value>(&dir.join("analysis.json"))
        && let Some(t) = a["taxonomies"].as_object()
    {
        for (slug, def) in t {
            if let Some(b) = def["rest_base"].as_str() {
                tax_bases.insert(b.to_string(), slug.clone());
            }
        }
    }
    for (id, _, row) in rows(ctx, ItemKind::Document, "wp-rest")? {
        match file(&row).and_then(|v| rest_post(&v, id, &users, &tax_bases, &row.label)) {
            Some(p) => r.add_post(p),
            None => {
                r.notes.insert(id, "raw file missing or unreadable".into());
            }
        }
    }
    for (id, src, row) in rows(ctx, ItemKind::Term, "wp-rest")? {
        let Some(v) = file(&row) else {
            r.notes.insert(id, "raw file missing or unreadable".into());
            continue;
        };
        let tax = opt(&v, "taxonomy").unwrap_or_else(|| {
            src.source_id
                .split(':')
                .nth(1)
                .unwrap_or_default()
                .to_string()
        });
        r.add_term(TermRec {
            taxonomy: tax,
            id: num(&v["id"]),
            slug: s(&v, "slug"),
            name: decode_entities(&s(&v, "name")),
            parent: num(&v["parent"]).filter(|n| *n > 0).map(TermKey::Id),
            raw: vec![id],
        });
    }
    for (id, _, row) in rows(ctx, ItemKind::Menu, "wp-rest")? {
        let Some(v) = file(&row) else {
            r.notes.insert(id, "raw file missing or unreadable".into());
            continue;
        };
        let Some(mid) = num(&v["menu"]["id"]) else {
            continue;
        };
        if let Some(m) = r.menus.get_mut(&mid) {
            m.raw.push(id);
            continue;
        }
        let items = v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| {
                Some(MenuItemRec {
                    id: num(&i["id"])?,
                    parent: num(&i["parent"]).filter(|n| *n > 0),
                    order: i["menu_order"].as_i64().unwrap_or(0),
                    title: rendered(i, "title").unwrap_or_default(),
                    url: s(i, "url"),
                    kind: s(i, "type"),
                    object: s(i, "object"),
                    object_id: num(&i["object_id"]),
                })
            })
            .collect();
        r.menus.insert(
            mid,
            MenuRec {
                name: s(&v["menu"], "name"),
                slug: s(&v["menu"], "slug"),
                items,
                raw: vec![id],
            },
        );
    }
    for (id, _, row) in rows(ctx, ItemKind::Settings, "wp-rest")? {
        r.settings_raw.push(id);
        if let Some(v) = file(&row) {
            site.names.push(opt(&v, "title"));
            site.taglines.push(opt(&v, "description"));
            site.urls.push(opt(&v, "url"));
            site.front.push(num(&v["page_on_front"]).filter(|n| *n > 0));
        }
    }
    for (id, _, row) in rows(ctx, ItemKind::Redirect, "wp-rest")? {
        match file(&row) {
            Some(v) => r.redirection.push((v, Some(id))),
            None => {
                r.notes.insert(id, "raw file missing or unreadable".into());
            }
        }
    }
    // Yoast heads saved as `<rest base>-<id>.json`.
    if let Ok(d) = std::fs::read_dir(dir.join("yoast")) {
        for e in d.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(pid) = name
                .strip_suffix(".json")
                .and_then(|n| n.rsplit_once('-'))
                .and_then(|(_, n)| n.parse::<u64>().ok())
            else {
                continue;
            };
            if let (Some(v), Some(p)) = (read_json::<Value>(&e.path()), r.posts.get_mut(&pid))
                && p.head.is_none()
                && v["json"].is_object()
            {
                p.head = Some(v["json"].clone());
            }
        }
    }
    Ok(())
}

// ---------- wxr ----------

fn wxr_post(i: WxrItem, raw: ItemId) -> Post {
    let pairs = Value::Array(i.meta.iter().map(|(k, v)| json!([k, v])).collect());
    let meta = fold_meta(&pairs, &mut false);
    let mut terms = vec![];
    let (mut lang, mut nav_menu) = (None, None);
    for (domain, slug, _) in &i.categories {
        match domain.as_str() {
            "nav_menu" => nav_menu = Some(slug.clone()),
            "language" => lang = Some(slug.clone()),
            "post_translations" => {}
            _ => terms.push((domain.clone(), TermKey::Slug(slug.clone()))),
        }
    }
    let m = |k: &str| meta.get(k).and_then(str_of).filter(|x| !x.is_empty());
    let is_media = i.post_type == "attachment";
    Post {
        id: i.post_id,
        status: i.status,
        slug: i.slug,
        title: i.title,
        link: if is_media {
            i.attachment_url
        } else {
            Some(i.link).filter(|l| !l.is_empty())
        },
        parent: Some(i.parent),
        menu_order: Some(i.menu_order),
        author: Some(i.author).filter(|a| !a.is_empty()),
        created: wp_date(&i.date_gmt),
        updated: wp_date(&i.modified_gmt),
        content: Some(i.content),
        caption: is_media.then(|| i.excerpt.clone()),
        excerpt: Some(i.excerpt),
        terms,
        featured: m("_thumbnail_id").and_then(|x| x.parse().ok()).or(Some(0)),
        file: m("_wp_attached_file"),
        file_meta: meta
            .get("_wp_attachment_metadata")
            .filter(|v| v.is_object())
            .cloned(),
        alt: m("_wp_attachment_image_alt"),
        lang,
        nav_menu,
        template: m("_wp_page_template").filter(|t| t != "default"),
        raw: vec![raw],
        post_type: i.post_type,
        meta,
        ..Post::default()
    }
}

fn load_wxr(ctx: &StageCtx, r: &mut Raw, site: &mut SiteParts) -> Result<()> {
    let dir = ctx.project.raw_dir("wxr");
    for (id, _, row) in rows(ctx, ItemKind::Document, "wxr")? {
        let item = row
            .raw_path
            .as_deref()
            .and_then(|p| read_json::<WxrItem>(&dir.join(p)));
        match item {
            Some(i) => r.add_post(wxr_post(i, id)),
            None => {
                r.notes
                    .insert(id, "not extracted (no raw item file)".into());
            }
        }
    }
    for a in read_json::<Vec<WxrAuthor>>(&dir.join("authors.json")).unwrap_or_default() {
        let raw = row_id(ItemKind::Author, "wxr", &a.login);
        let author = Author {
            name: if a.display_name.is_empty() {
                a.login.clone()
            } else {
                a.display_name.clone()
            },
            email: Some(a.email.clone()).filter(|e| !e.is_empty()),
            slug: a.login.clone(),
        };
        r.add_author(a.login, author, Some(raw));
    }
    let terms = read_json::<Vec<WxrTerm>>(&dir.join("terms.json")).unwrap_or_default();
    let mut menu_by_slug: HashMap<String, u64> = r
        .menus
        .iter()
        .map(|(id, m)| (m.slug.clone(), *id))
        .collect();
    for t in terms {
        let raw = row_id(
            ItemKind::Term,
            "wxr",
            &format!("{}:{}", t.domain, t.nicename),
        );
        if t.domain == "nav_menu" {
            match t.id {
                Some(id) => {
                    let m = r.menus.entry(id).or_insert_with(|| MenuRec {
                        name: decode_entities(&t.name),
                        slug: t.nicename.clone(),
                        ..MenuRec::default()
                    });
                    m.raw.push(raw);
                    menu_by_slug.entry(t.nicename.clone()).or_insert(id);
                }
                None => {
                    r.notes
                        .insert(raw, "menu without a term id in the export".into());
                }
            }
            continue;
        }
        r.add_term(TermRec {
            taxonomy: t.domain,
            id: t.id,
            slug: t.nicename,
            name: decode_entities(&t.name),
            parent: Some(t.parent).filter(|p| !p.is_empty()).map(TermKey::Slug),
            raw: vec![raw],
        });
    }
    // Menu items exported as posts, for menus no better connector described.
    let from_db_or_rest: Vec<u64> = r
        .menus
        .iter()
        .filter(|(_, m)| !m.items.is_empty())
        .map(|(id, _)| *id)
        .collect();
    let mut add = vec![];
    for p in r.posts.values().filter(|p| p.post_type == "nav_menu_item") {
        let Some(mid) = p.nav_menu.as_ref().and_then(|s| menu_by_slug.get(s)) else {
            continue;
        };
        if from_db_or_rest.contains(mid) {
            continue;
        }
        let m = |k: &str| p.meta.get(k).and_then(str_of).unwrap_or_default();
        add.push((
            *mid,
            MenuItemRec {
                id: p.id,
                parent: m("_menu_item_menu_item_parent")
                    .parse()
                    .ok()
                    .filter(|n| *n > 0),
                order: p.menu_order.unwrap_or(0),
                title: p.title.clone(),
                url: m("_menu_item_url"),
                kind: m("_menu_item_type"),
                object: m("_menu_item_object"),
                object_id: m("_menu_item_object_id").parse().ok(),
            },
        ));
    }
    for (mid, item) in add {
        if let Some(m) = r.menus.get_mut(&mid) {
            m.items.push(item);
        }
    }
    if let Some(v) = read_json::<Value>(&dir.join("site.json")) {
        site.names.push(opt(&v, "title"));
        site.urls
            .extend([opt(&v, "base_site_url"), opt(&v, "link")]);
    }
    Ok(())
}

// ---------- crawl ----------

fn load_crawl(ctx: &StageCtx, r: &mut Raw) {
    let Ok(d) = std::fs::read_dir(ctx.project.raw_dir("crawl").join("pages")) else {
        return;
    };
    for e in d.flatten() {
        let Some(h) = read_json::<PageHead>(&e.path()) else {
            continue;
        };
        let Some(k) = normalise_url(&h.url, None).map(|k| k.0) else {
            continue;
        };
        let id = h.body_classes.iter().find_map(|c| {
            c.strip_prefix("postid-")
                .or_else(|| c.strip_prefix("page-id-"))?
                .parse::<u64>()
                .ok()
        });
        if let Some(id) = id {
            r.page_ids.insert(k.clone(), id);
        }
        r.crawl_heads.insert(k, h);
    }
}

/// Site facts as each source states them, best first.
#[derive(Default)]
struct SiteParts {
    names: Vec<Option<String>>,
    taglines: Vec<Option<String>>,
    urls: Vec<Option<String>>,
    front: Vec<Option<u64>>,
    permalink: Vec<Option<String>>,
    titles: Value,
}

fn first<T: Clone>(v: &[Option<T>]) -> Option<T> {
    v.iter().flatten().next().cloned()
}

/// A connector of this name is in the config.
pub(crate) fn configured(cfg: &crate::config::ProjectConfig, name: &str) -> bool {
    cfg.connectors.iter().any(|c| {
        matches!(
            (c, name),
            (ConnectorConfig::WpDb { .. }, "wp-db")
                | (ConnectorConfig::WpRest { .. }, "wp-rest")
                | (ConnectorConfig::Wxr { .. }, "wxr")
                | (ConnectorConfig::Crawl { .. }, "crawl")
        )
    })
}

pub(crate) fn load(ctx: &StageCtx) -> Result<Raw> {
    let mut parts = SiteParts::default();
    let mut uploads = None;
    // Configured URLs come first: a site_url override beats the dump's own home.
    for c in &ctx.config.connectors {
        match c {
            ConnectorConfig::WpDb {
                site_url,
                wp_root: root,
                uploads_dir,
                ..
            } => {
                parts.urls.insert(0, site_url.clone());
                let dir = ctx.project.dir();
                uploads = uploads_dir.as_ref().map(|p| dir.join(p)).or_else(|| {
                    root.as_ref()
                        .map(|p| dir.join(p).join("wp-content/uploads"))
                });
            }
            ConnectorConfig::WpRest { url, .. } | ConnectorConfig::Crawl { url, .. } => {
                parts.urls.push(Some(url.clone()));
            }
            _ => {}
        }
    }
    let mut r = Raw {
        posts: BTreeMap::new(),
        authors: BTreeMap::new(),
        terms: BTreeMap::new(),
        term_ix: HashMap::new(),
        taxonomies: BTreeMap::new(),
        menus: BTreeMap::new(),
        gf: vec![],
        nf: vec![],
        submissions: vec![],
        redirection: vec![],
        acf_groups: vec![],
        translations: Value::Null,
        crawl_heads: HashMap::new(),
        page_ids: HashMap::new(),
        site: Site {
            name: String::new(),
            tagline: String::new(),
            home: Url::parse("https://localhost/")?,
            hosts: vec![],
            front_page: None,
            permalink: None,
            titles: Value::Null,
            uploads: None,
        },
        settings_raw: vec![],
        notes: HashMap::new(),
        crawl: false,
    };
    // Only connectors configured now: a removed one's leftovers in raw/ are not the site.
    let on = |name: &str| configured(&ctx.config, name);
    r.crawl = on("crawl");
    let mut db_parts = SiteParts::default();
    if on("wp-db")
        && let Err(e) = load_db(&mut r, &ctx.project.raw_dir("wp-db"), &mut db_parts)
    {
        ctx.issue(
            IssueKind::AccessGap,
            Severity::Error,
            None,
            format!("wp-db export unreadable: {e}"),
            json!({ "connector": "wp-db", "file": "export.json" }),
        )?;
    }
    let mut rest_parts = SiteParts::default();
    if on("wp-rest") {
        load_rest(ctx, &mut r, &mut rest_parts)?;
    }
    let mut wxr_parts = SiteParts::default();
    if on("wxr") {
        load_wxr(ctx, &mut r, &mut wxr_parts)?;
    }
    if r.crawl {
        load_crawl(ctx, &mut r);
    }
    // Rendered HTML ranks below every connector's raw content.
    for p in r.posts.values_mut() {
        if p.content.is_none() {
            p.content = p.content_rendered.take();
        }
    }
    // Configured URL overrides, then db, rest, wxr, then the rest of config.
    let cfg_urls = std::mem::take(&mut parts.urls);
    let (cfg_first, cfg_rest) = cfg_urls.split_at(usize::from(
        ctx.config
            .connectors
            .iter()
            .any(|c| matches!(c, ConnectorConfig::WpDb { .. })),
    ));
    for p in [&db_parts, &rest_parts, &wxr_parts] {
        parts.names.extend(p.names.iter().cloned());
        parts.taglines.extend(p.taglines.iter().cloned());
        parts.front.extend(p.front.iter().cloned());
        parts.permalink.extend(p.permalink.iter().cloned());
    }
    let urls: Vec<Option<String>> = cfg_first
        .iter()
        .chain(&db_parts.urls)
        .chain(&rest_parts.urls)
        .chain(&wxr_parts.urls)
        .chain(cfg_rest)
        .cloned()
        .collect();
    let parsed: Vec<Url> = urls
        .iter()
        .flatten()
        .filter_map(|u| Url::parse(u).ok())
        .filter(|u| u.host_str().is_some())
        .collect();
    let mut hosts: Vec<String> = vec![];
    for u in &parsed {
        let h = u.host_str().unwrap_or_default().to_lowercase();
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    // No URL anywhere: the hosts the posts' own links use.
    if hosts.is_empty() {
        for p in r.posts.values() {
            if let Some(h) = p
                .link
                .as_deref()
                .and_then(|l| Url::parse(l).ok())
                .and_then(|u| u.host_str().map(str::to_lowercase))
                && !hosts.contains(&h)
            {
                hosts.push(h);
            }
        }
    }
    r.site = Site {
        name: first(&parts.names).unwrap_or_default(),
        tagline: first(&parts.taglines).unwrap_or_default(),
        home: parsed
            .first()
            .cloned()
            .or_else(|| {
                hosts
                    .first()
                    .and_then(|h| Url::parse(&format!("https://{h}/")).ok())
            })
            .unwrap_or(r.site.home),
        hosts,
        front_page: first(&parts.front),
        permalink: first(&parts.permalink).filter(|p| !p.is_empty()),
        titles: db_parts.titles,
        uploads,
    };
    if !r.terms.is_empty() || !r.taxonomies.is_empty() {
        for (t, h) in [("category", true), ("post_tag", false)] {
            if let Some(cur) = r.taxonomies.get_mut(t) {
                *cur |= h;
            }
        }
        let with_parent: Vec<String> = r
            .terms
            .values()
            .filter(|t| t.parent.is_some())
            .map(|t| t.taxonomy.clone())
            .collect();
        for t in with_parent {
            r.taxonomies.insert(t, true);
        }
    }
    Ok(r)
}
