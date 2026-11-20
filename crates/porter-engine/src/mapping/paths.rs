use super::MappingConfig;
use indexmap::IndexMap;
use porter_model::{Document, FieldValue, ItemId};
use std::collections::{HashMap, HashSet};

/// Each document's depth in the parent tree (a parent is always shallower; cycles stop).
pub fn depths(docs: &[Document]) -> HashMap<ItemId, usize> {
    let by_id: HashMap<ItemId, &Document> = docs.iter().map(|d| (d.id, d)).collect();
    docs.iter()
        .map(|d| {
            let (mut n, mut cur, mut seen) = (0usize, d.parent, HashSet::new());
            while let Some(p) = cur.filter(|p| seen.insert(*p)) {
                n += 1;
                cur = by_id.get(&p).and_then(|d| d.parent);
            }
            (d.id, n)
        })
        .collect()
}

/// What a pattern can look at besides its own document: every document (to follow
/// references) and the mapping's lookup tables.
pub struct PathCx<'a> {
    pub docs: &'a HashMap<ItemId, Document>,
    pub lookups: &'a IndexMap<String, IndexMap<String, String>>,
}

/// The new URL path of every document whose type maps somewhere with a path pattern,
/// parents first so `{parent.path}` resolves. Parents that don't render leave their
/// children an error too.
pub fn render_all(
    docs: &[Document],
    mapping: &MappingConfig,
) -> Vec<(ItemId, Result<String, String>)> {
    let depth = depths(docs);
    let by_id: HashMap<ItemId, Document> = docs.iter().map(|d| (d.id, d.clone())).collect();
    let cx = PathCx {
        docs: &by_id,
        lookups: &mapping.lookups,
    };
    let mut todo: Vec<(&Document, &str)> = docs
        .iter()
        .filter_map(|d| {
            let tm = mapping.types.get(&d.doc_type)?;
            tm.to.as_ref()?;
            Some((d, tm.path.as_deref()?))
        })
        .collect();
    todo.sort_by_key(|(d, _)| (depth[&d.id], d.path.matches('/').count()));
    let mut done: HashMap<ItemId, String> = HashMap::new();
    let mut out = vec![];
    for (d, pattern) in todo {
        let parent = match d.parent {
            Some(p) => done.get(&p).cloned(),
            None => Some(String::new()),
        };
        let r = render(pattern, d, parent.as_deref(), &cx);
        if let Ok(p) = &r {
            done.insert(d.id, p.clone());
        }
        out.push((d.id, r));
    }
    out
}

/// Follows `segs` from `doc`: every segment but the last names a `Doc` field (or `parent`)
/// and moves to its target; the last is a Document attribute (`slug`, `title`, `type`, `id`,
/// `path`) or a Text/Number field.
fn resolve(doc: &Document, segs: &[&str], cx: &PathCx) -> Result<String, String> {
    let mut cur = doc;
    let (last, hops) = segs.split_last().ok_or("empty")?;
    for seg in hops {
        let next = match (*seg, cur.fields.get(*seg)) {
            (_, Some(FieldValue::Doc(id))) => *id,
            ("parent", None) => cur.parent.ok_or("has no parent")?,
            (_, Some(_)) => return Err(format!("\"{seg}\" is not a reference")),
            _ => return Err(format!("no field \"{seg}\"")),
        };
        cur = cx
            .docs
            .get(&next)
            .ok_or_else(|| format!("\"{seg}\" points at a missing document"))?;
    }
    match *last {
        "slug" => Ok(cur.slug.clone()),
        "title" => Ok(cur.title.clone()),
        "type" => Ok(cur.doc_type.clone()),
        "id" => Ok(cur.source.source_id.clone()),
        "path" => Ok(cur.path.clone()),
        f => match cur.fields.get(f) {
            Some(FieldValue::Text(t)) => Ok(t.clone()),
            Some(FieldValue::Number(n)) => Ok(n.to_string()),
            Some(_) => Err(format!("\"{f}\" is not text or a number")),
            None => Err(format!("no field \"{f}\"")),
        },
    }
}

/// Renders a URL path pattern. Placeholders: `{slug}`, `{id}`, `{type}`, `{yyyy}`, `{mm}`,
/// `{parent.path}` (the parent's rendered path), and dotted chains `{a.b.attr}`: `a`, `b` are
/// `Doc` fields (or `parent`, the document's parent link) followed in turn, `attr` is `slug`,
/// `title`, `type`, `id`, `path` or a Text/Number field of the target. A single name that isn't
/// built in is a Text/Number field of the document itself. `{token|lookup:table}` maps the
/// value through `mapping.lookups.<table>`. Date placeholders use `created`, then `updated`.
pub fn render(
    pattern: &str,
    doc: &Document,
    parent_path: Option<&str>,
    cx: &PathCx,
) -> Result<String, String> {
    let date = doc.created.or(doc.updated);
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let Some(j) = tail.find('}') else {
            return Err(format!("path pattern \"{pattern}\" has an unclosed '{{'"));
        };
        let token = &tail[..j];
        let (name, table) = match token.split_once('|') {
            Some((n, l)) => (
                n,
                Some(l.strip_prefix("lookup:").ok_or_else(|| {
                    format!("unknown filter in {{{token}}} (expected |lookup:<table>)")
                })?),
            ),
            None => (token, None),
        };
        let value = match name {
            "slug" => doc.slug.clone(),
            "id" => doc.source.source_id.clone(),
            "type" => doc.doc_type.clone(),
            k @ ("yyyy" | "mm") => {
                let d =
                    date.ok_or_else(|| format!("{{{k}}} needs a date but the item has none"))?;
                d.format(if k == "yyyy" { "%Y" } else { "%m" }).to_string()
            }
            "parent.path" => parent_path
                .ok_or_else(|| "{parent.path} used but the item has no parent".to_string())?
                .to_string(),
            other => {
                let segs: Vec<&str> = other.split('.').collect();
                resolve(doc, &segs, cx).map_err(|why| {
                    if segs.len() == 1 && why.starts_with("no field") {
                        format!("unknown placeholder {{{other}}} in \"{pattern}\"")
                    } else {
                        format!("{{{token}}}: {why}")
                    }
                })?
            }
        };
        match table {
            Some(t) => out.push_str(
                cx.lookups
                    .get(t)
                    .ok_or_else(|| format!("{{{token}}}: no lookup table \"{t}\""))?
                    .get(&value)
                    .ok_or_else(|| format!("{{{token}}}: no \"{value}\" in lookup \"{t}\""))?,
            ),
            None => out.push_str(&value),
        }
        rest = &tail[j + 1..];
    }
    out.push_str(rest);
    // Collapse runs of slashes; keep a leading slash.
    let mut norm = String::with_capacity(out.len());
    for c in out.chars() {
        if !(c == '/' && norm.ends_with('/')) {
            norm.push(c);
        }
    }
    Ok(norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use porter_model::{DocStatus, ItemId, ItemKind, Seo, SourceRef};

    fn cx() -> PathCx<'static> {
        // Leaked: tests only, a few bytes.
        let docs = Box::leak(Box::new(HashMap::new()));
        let lookups = Box::leak(Box::new(IndexMap::new()));
        PathCx { docs, lookups }
    }

    fn doc(slug: &str) -> Document {
        let src = SourceRef {
            connector: "c".into(),
            source_id: "42".into(),
            url: None,
        };
        Document {
            id: ItemId::new(ItemKind::Document, &src),
            doc_type: "room".into(),
            status: DocStatus::Published,
            locale: None,
            translations: vec![],
            slug: slug.into(),
            path: String::new(),
            parent: None,
            title: String::new(),
            fields: Default::default(),
            seo: Seo::default(),
            created: Some(Utc.with_ymd_and_hms(2024, 3, 5, 0, 0, 0).unwrap()),
            updated: None,
            author: None,
            original: serde_json::Value::Null,
            source: src,
        }
    }

    #[test]
    fn nested_path_pattern() {
        let d = doc("suite");
        assert_eq!(
            render("{parent.path}/{slug}/", &d, Some("/rooms/"), &cx()).unwrap(),
            "/rooms/suite/"
        );
    }

    #[test]
    fn all_placeholders() {
        let d = doc("a");
        assert_eq!(
            render("/{type}/{yyyy}/{mm}/{id}-{slug}/", &d, None, &cx()).unwrap(),
            "/room/2024/03/42-a/"
        );
    }

    #[test]
    fn errors() {
        let d = doc("a");
        assert!(
            render("/{nope}/", &d, None, &cx())
                .unwrap_err()
                .contains("nope")
        );
        assert!(render("{parent.path}/x", &d, None, &cx()).is_err());
        let mut u = doc("a");
        u.created = None;
        assert!(render("/{yyyy}/", &u, None, &cx()).is_err());
    }

    fn chain() -> (HashMap<ItemId, Document>, Document) {
        let mk = |sid: &str, slug: &str| {
            let mut d = doc(slug);
            d.source.source_id = sid.into();
            d.id = ItemId::new(ItemKind::Document, &d.source);
            d
        };
        let region = mk("r", "europe");
        let mut country = mk("c", "spain");
        country
            .fields
            .insert("region".into(), FieldValue::Doc(region.id));
        let mut city = mk("ci", "marbella");
        city.fields
            .insert("country".into(), FieldValue::Doc(country.id));
        city.fields
            .insert("tier".into(), FieldValue::Text("gold".into()));
        let docs = [region, country, city.clone()]
            .into_iter()
            .map(|d| (d.id, d))
            .collect();
        (docs, city)
    }

    #[test]
    fn follows_references() {
        let (docs, city) = chain();
        let lookups = IndexMap::new();
        let cx = PathCx {
            docs: &docs,
            lookups: &lookups,
        };
        assert_eq!(
            render(
                "/destinations/{country.region.slug}/{country.slug}/{slug}",
                &city,
                None,
                &cx
            )
            .unwrap(),
            "/destinations/europe/spain/marbella"
        );
        assert_eq!(render("/{tier}/", &city, None, &cx).unwrap(), "/gold/");
        let e = render("/{country.nope.slug}/", &city, None, &cx).unwrap_err();
        assert!(e.contains("country.nope.slug"), "{e}");
    }

    #[test]
    fn lookup_maps_values() {
        let (docs, city) = chain();
        let lookups: IndexMap<String, IndexMap<String, String>> = [(
            "tiers".to_string(),
            [("gold".to_string(), "premium".to_string())]
                .into_iter()
                .collect(),
        )]
        .into_iter()
        .collect();
        let cx = PathCx {
            docs: &docs,
            lookups: &lookups,
        };
        assert_eq!(
            render("/{tier|lookup:tiers}/{slug}", &city, None, &cx).unwrap(),
            "/premium/marbella"
        );
    }

    #[test]
    fn missing_lookup_key_is_err_naming_token() {
        let (docs, mut city) = chain();
        city.fields
            .insert("tier".into(), FieldValue::Text("tin".into()));
        let lookups: IndexMap<String, IndexMap<String, String>> =
            [("tiers".to_string(), IndexMap::new())]
                .into_iter()
                .collect();
        let cx = PathCx {
            docs: &docs,
            lookups: &lookups,
        };
        let e = render("/{tier|lookup:tiers}/", &city, None, &cx).unwrap_err();
        assert!(
            e.contains("{tier|lookup:tiers}") && e.contains("tin"),
            "{e}"
        );
        let e = render("/{tier|lookup:nope}/", &city, None, &cx).unwrap_err();
        assert!(e.contains("{tier|lookup:nope}"), "{e}");
    }
}
