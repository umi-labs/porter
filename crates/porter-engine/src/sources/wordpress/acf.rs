//! ACF values typed by walking the field-group definitions: repeaters and flexible content
//! become lists of objects, media and relations become references.

use super::classic_to_richtext;
use super::normalise::Post;
use crate::html::HtmlCtx;
use chrono::{NaiveDate, NaiveDateTime};
use indexmap::IndexMap;
use porter_model::{FieldValue, IssueKind, ItemId, Severity};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};

/// Definitions nest at most this deep (hostile or broken data can't recurse forever).
const MAX_DEPTH: usize = 32;
/// Most rows read for one repeater or flexible field from flattened meta.
const MAX_ROWS: u64 = 10_000;

/// Top-level ACF fields by name, each with `sub_fields` and `layouts` (as an array) nested.
#[derive(Default)]
pub(crate) struct Defs {
    pub top: IndexMap<String, Value>,
}

fn arr(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => o.values().collect(),
        _ => vec![],
    }
}

/// `layouts` as an array, sub-fields included.
fn tidy(mut f: Value) -> Value {
    if let Some(o) = f.as_object_mut()
        && let Some(l) = o.get("layouts")
    {
        let l: Vec<Value> = arr(l).into_iter().cloned().collect();
        o.insert("layouts".into(), Value::Array(l));
    }
    f
}

impl Defs {
    /// From `acf_field_groups` when the exporter had it, else from `acf-field` posts.
    pub fn load<'p>(groups: &[Value], posts: impl Iterator<Item = &'p Post>) -> Defs {
        let mut d = Defs::default();
        for g in groups {
            for f in arr(&g["fields"]) {
                if let Some(n) = f["name"].as_str().filter(|n| !n.is_empty()) {
                    d.top
                        .entry(n.to_string())
                        .or_insert_with(|| tidy(f.clone()));
                }
            }
        }
        if !d.top.is_empty() {
            return d;
        }
        // Field posts: settings serialized in the content, name in the excerpt, key in the slug.
        let mut groups_ids = vec![];
        let mut fields: BTreeMap<u64, (i64, u64, Value)> = BTreeMap::new();
        for p in posts {
            match p.post_type.as_str() {
                "acf-field-group" => groups_ids.push(p.id),
                "acf-field" => {
                    let mut def = match super::db::php_decode(p.content.as_deref().unwrap_or("")) {
                        super::db::Php::Value(v @ Value::Object(_)) => v,
                        _ => json!({}),
                    };
                    if let Some(o) = def.as_object_mut() {
                        o.insert("key".into(), json!(p.slug));
                        o.insert("name".into(), json!(p.excerpt.clone().unwrap_or_default()));
                        o.insert("label".into(), json!(p.title));
                    }
                    fields.insert(
                        p.id,
                        (p.menu_order.unwrap_or(0), p.parent.unwrap_or(0), def),
                    );
                }
                _ => {}
            }
        }
        let mut children: HashMap<u64, Vec<(i64, u64)>> = HashMap::new();
        for (id, (order, parent, _)) in &fields {
            children.entry(*parent).or_default().push((*order, *id));
        }
        for c in children.values_mut() {
            c.sort();
        }
        fn build(
            id: u64,
            fields: &BTreeMap<u64, (i64, u64, Value)>,
            children: &HashMap<u64, Vec<(i64, u64)>>,
            depth: usize,
        ) -> Value {
            let mut def = fields.get(&id).map(|f| f.2.clone()).unwrap_or(json!({}));
            if depth >= MAX_DEPTH {
                return def;
            }
            let kids: Vec<Value> = children
                .get(&id)
                .into_iter()
                .flatten()
                .map(|(_, k)| build(*k, fields, children, depth + 1))
                .collect();
            let Some(o) = def.as_object_mut() else {
                return def;
            };
            if o.contains_key("layouts") {
                let mut layouts: Vec<Value> = o
                    .get("layouts")
                    .map(|l| arr(l).into_iter().cloned().collect())
                    .unwrap_or_default();
                for k in kids {
                    let lk = k["parent_layout"].as_str().unwrap_or("").to_string();
                    if let Some(l) = layouts.iter_mut().find(|l| l["key"] == lk.as_str())
                        && let Some(lo) = l.as_object_mut()
                    {
                        let subs = lo.entry("sub_fields").or_insert_with(|| json!([]));
                        if let Value::Array(a) = subs {
                            a.push(k);
                        }
                    }
                }
                o.insert("layouts".into(), Value::Array(layouts));
            } else {
                o.insert("sub_fields".into(), Value::Array(kids));
            }
            def
        }
        let mut tops: Vec<(i64, u64)> = groups_ids
            .iter()
            .flat_map(|g| children.get(g).cloned().unwrap_or_default())
            .collect();
        tops.sort();
        for (_, id) in tops {
            let def = build(id, &fields, &children, 0);
            if let Some(n) = def["name"].as_str().filter(|n| !n.is_empty()) {
                d.top.entry(n.to_string()).or_insert(def);
            }
        }
        d
    }
}

/// Everything typing needs to resolve references.
pub(crate) struct AcfCx<'a, 'h> {
    pub html: &'a mut HtmlCtx<'h>,
    pub media_ids: &'a HashMap<u64, ItemId>,
    pub docs: &'a HashMap<u64, ItemId>,
    /// `(taxonomy, term id)` → term item.
    pub term: &'a dyn Fn(&str, u64) -> Option<ItemId>,
}

/// `acf.<name>` fields for one post: from its ACF values (`acf`) when the exporter had them,
/// else rebuilt from flattened meta (`name`, `name_0_sub`, …) for the defined fields.
pub(crate) fn fields(
    defs: &Defs,
    acf: Option<&Map<String, Value>>,
    meta: &Map<String, Value>,
    cx: &mut AcfCx,
) -> Vec<(String, FieldValue)> {
    let mut out = vec![];
    match acf {
        Some(values) => {
            for (name, v) in values {
                let f = match defs.top.get(name) {
                    Some(def) => typed(def, v, cx, name, 0),
                    None => generic(v),
                };
                out.push((format!("acf.{name}"), f));
            }
        }
        None => {
            for (name, def) in &defs.top {
                if meta.contains_key(name) {
                    let v = from_meta(def, name, meta, 0);
                    out.push((format!("acf.{name}"), typed(def, &v, cx, name, 0)));
                }
            }
        }
    }
    out
}

fn count(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
    .min(MAX_ROWS)
}

/// The value ACF would return for `def`, read from flattened post meta under `key`.
fn from_meta(def: &Value, key: &str, meta: &Map<String, Value>, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return meta.get(key).cloned().unwrap_or(Value::Null);
    }
    let subs = |prefix: &str, sub_fields: &Value| -> Map<String, Value> {
        arr(sub_fields)
            .into_iter()
            .filter_map(|s| {
                let n = s["name"].as_str()?;
                Some((
                    n.to_string(),
                    from_meta(s, &format!("{prefix}_{n}"), meta, depth + 1),
                ))
            })
            .collect()
    };
    match def["type"].as_str().unwrap_or("") {
        "repeater" => Value::Array(
            (0..count(meta.get(key)))
                .map(|i| Value::Object(subs(&format!("{key}_{i}"), &def["sub_fields"])))
                .collect(),
        ),
        "flexible_content" => {
            let names: Vec<String> = match meta.get(key) {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect(),
                Some(Value::Object(o)) => o
                    .values()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect(),
                _ => vec![],
            };
            Value::Array(
                names
                    .iter()
                    .take(MAX_ROWS as usize)
                    .enumerate()
                    .map(|(i, l)| {
                        let layout = arr(&def["layouts"])
                            .into_iter()
                            .find(|x| x["name"] == l.as_str());
                        let mut row = layout
                            .map(|x| subs(&format!("{key}_{i}"), &x["sub_fields"]))
                            .unwrap_or_default();
                        row.insert("acf_fc_layout".into(), json!(l));
                        Value::Object(row)
                    })
                    .collect(),
            )
        }
        "group" => Value::Object(subs(key, &def["sub_fields"])),
        _ => meta.get(key).cloned().unwrap_or(Value::Null),
    }
}

fn empty(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => matches!(s.as_str(), "1" | "true" | "yes"),
        _ => false,
    }
}

/// Untyped: JSON shape as-is.
pub(crate) fn generic(v: &Value) -> FieldValue {
    match v {
        Value::Null => FieldValue::Null,
        Value::Bool(b) => FieldValue::Bool(*b),
        Value::Number(n) => n.as_f64().map_or(FieldValue::Null, FieldValue::Number),
        Value::String(s) => FieldValue::Text(s.clone()),
        Value::Array(a) => FieldValue::List(a.iter().map(generic).collect()),
        Value::Object(o) => {
            FieldValue::Object(o.iter().map(|(k, v)| (k.clone(), generic(v))).collect())
        }
    }
}

/// Ids in a value: numbers, numeric strings, objects with `id`/`ID`, lists of those.
fn ids(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Object(o) if !o.contains_key("id") && !o.contains_key("ID") => {
            o.values().cloned().collect()
        }
        Value::String(s) if s.contains(',') => s.split(',').map(|x| json!(x.trim())).collect(),
        Value::Null => vec![],
        other => vec![other.clone()],
    }
}

fn id_of(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Object(o) => o.get("id").or_else(|| o.get("ID")).and_then(id_of),
        _ => None,
    }
}

fn date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = s.trim();
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y%m%d %H:%M:%S"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            return Some(d.and_utc());
        }
    }
    for f in ["%Y%m%d", "%Y-%m-%d", "%d/%m/%Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, f) {
            return d.and_hms_opt(0, 0, 0).map(|d| d.and_utc());
        }
    }
    None
}

fn media(v: &Value, cx: &mut AcfCx, at: &str) -> FieldValue {
    let found = match (id_of(v), v) {
        (Some(id), _) => cx.media_ids.get(&id).copied(),
        (None, Value::String(u)) => cx.html.find_media([u.as_str()]),
        (None, Value::Object(o)) => o
            .get("url")
            .and_then(Value::as_str)
            .and_then(|u| cx.html.find_media([u])),
        _ => None,
    };
    match found {
        Some(m) => FieldValue::Media(m),
        None => {
            cx.html.issue(
                IssueKind::MissingMedia,
                Severity::Warning,
                format!("ACF field {at}: attachment {v} is not in the media library"),
                json!({ "field": at, "value": v }),
            );
            FieldValue::Null
        }
    }
}

fn doc(v: &Value, cx: &mut AcfCx, at: &str) -> FieldValue {
    if let Value::String(u) = v
        && id_of(v).is_none()
    {
        return cx.html.link_value(u);
    }
    match id_of(v).and_then(|id| cx.docs.get(&id)) {
        Some(d) => FieldValue::Doc(*d),
        None => {
            cx.html.issue(
                IssueKind::UnresolvedRelation,
                Severity::Warning,
                format!("ACF field {at}: post {v} is not among the documents"),
                json!({ "field": at, "value": v }),
            );
            FieldValue::Null
        }
    }
}

fn term(def: &Value, v: &Value, cx: &mut AcfCx, at: &str) -> FieldValue {
    let tax = def["taxonomy"].as_str().unwrap_or("category");
    match id_of(v).and_then(|id| (cx.term)(tax, id)) {
        Some(t) => FieldValue::Doc(t),
        None => {
            cx.html.issue(
                IssueKind::UnresolvedRelation,
                Severity::Warning,
                format!("ACF field {at}: {tax} term {v} not found"),
                json!({ "field": at, "value": v }),
            );
            FieldValue::Null
        }
    }
}

fn object(
    sub_fields: &Value,
    row: &Value,
    cx: &mut AcfCx,
    at: &str,
    depth: usize,
) -> IndexMap<String, FieldValue> {
    let mut o = IndexMap::new();
    for s in arr(sub_fields) {
        let Some(n) = s["name"].as_str() else {
            continue;
        };
        let v = row.get(n).unwrap_or(&Value::Null);
        o.insert(
            n.to_string(),
            typed(s, v, cx, &format!("{at}.{n}"), depth + 1),
        );
    }
    o
}

fn typed(def: &Value, v: &Value, cx: &mut AcfCx, at: &str, depth: usize) -> FieldValue {
    let ty = def["type"].as_str().unwrap_or("");
    if depth >= MAX_DEPTH {
        return generic(v);
    }
    if ty == "true_false" {
        return FieldValue::Bool(truthy(v));
    }
    if empty(v) {
        return match ty {
            "repeater" | "flexible_content" | "gallery" | "relationship" | "checkbox" => {
                FieldValue::List(vec![])
            }
            _ => FieldValue::Null,
        };
    }
    let multiple = truthy(&def["multiple"]);
    let rows = |v: &Value| -> Vec<Value> { arr(v).into_iter().cloned().collect() };
    match ty {
        "repeater" => FieldValue::List(
            rows(v)
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    FieldValue::Object(object(
                        &def["sub_fields"],
                        r,
                        cx,
                        &format!("{at}[{i}]"),
                        depth,
                    ))
                })
                .collect(),
        ),
        "flexible_content" => FieldValue::List(
            rows(v)
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let name = r["acf_fc_layout"].as_str().unwrap_or("").to_string();
                    let layout = arr(&def["layouts"])
                        .into_iter()
                        .find(|l| l["name"] == name.as_str())
                        .cloned()
                        .unwrap_or(Value::Null);
                    let mut o = IndexMap::new();
                    o.insert("layout".to_string(), FieldValue::Text(name.clone()));
                    o.extend(object(
                        &layout["sub_fields"],
                        r,
                        cx,
                        &format!("{at}[{i}]"),
                        depth,
                    ));
                    FieldValue::Object(o)
                })
                .collect(),
        ),
        "group" => FieldValue::Object(object(&def["sub_fields"], v, cx, at, depth)),
        "gallery" => FieldValue::List(ids(v).iter().map(|x| media(x, cx, at)).collect()),
        "image" | "file" => media(v, cx, at),
        "relationship" => FieldValue::List(ids(v).iter().map(|x| doc(x, cx, at)).collect()),
        "post_object" | "page_link" if multiple || v.is_array() => {
            FieldValue::List(ids(v).iter().map(|x| doc(x, cx, at)).collect())
        }
        "post_object" | "page_link" => doc(v, cx, at),
        "taxonomy" => {
            let many = matches!(
                def["field_type"].as_str(),
                Some("checkbox" | "multi_select")
            ) || v.is_array();
            let list: Vec<FieldValue> = ids(v).iter().map(|x| term(def, x, cx, at)).collect();
            if many {
                FieldValue::List(list)
            } else {
                list.into_iter().next().unwrap_or(FieldValue::Null)
            }
        }
        "wysiwyg" => match v.as_str() {
            Some(html) => FieldValue::RichText(classic_to_richtext(html, cx.media_ids, cx.html)),
            None => generic(v),
        },
        "date_picker" | "date_time_picker" => match v.as_str().and_then(date) {
            Some(d) => FieldValue::Date(d),
            None => generic(v),
        },
        "number" | "range" => match v {
            Value::Number(n) => n.as_f64().map_or(FieldValue::Null, FieldValue::Number),
            Value::String(s) => s
                .trim()
                .parse()
                .map_or_else(|_| FieldValue::Text(s.clone()), FieldValue::Number),
            _ => generic(v),
        },
        // A single choice may be stored as one string; it is still a list.
        "checkbox" | "select" if ty == "checkbox" || multiple => match v {
            Value::String(s) => FieldValue::List(vec![FieldValue::Text(s.clone())]),
            _ => FieldValue::List(rows(v).iter().map(generic).collect()),
        },
        "link" => match v {
            Value::String(u) => cx.html.link_value(u),
            Value::Object(o) => {
                let mut out = IndexMap::new();
                for (k, x) in o {
                    let f = match (k.as_str(), x.as_str()) {
                        ("url", Some(u)) => cx.html.link_value(u),
                        _ => generic(x),
                    };
                    out.insert(k.clone(), f);
                }
                FieldValue::Object(out)
            }
            _ => generic(v),
        },
        _ => generic(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::tests::{ctx, fixture};

    #[test]
    fn flexible_and_meta_rows() {
        let f = fixture();
        let mut issues = vec![];
        let mut html = ctx(&f, &mut issues);
        let media_ids = HashMap::from([(7u64, f.media)]);
        let docs = HashMap::from([(1u64, f.about)]);
        let term = |_: &str, _: u64| None;
        let mut cx = AcfCx {
            html: &mut html,
            media_ids: &media_ids,
            docs: &docs,
            term: &term,
        };
        let def = json!({"name": "blocks", "type": "flexible_content", "layouts": {
            "l1": {"key": "l1", "name": "hero", "sub_fields": [
                {"name": "pic", "type": "image"}, {"name": "on", "type": "true_false"},
                {"name": "when", "type": "date_picker"}, {"name": "rel", "type": "relationship"},
            ]},
        }});
        let defs = Defs {
            top: IndexMap::from([("blocks".to_string(), tidy(def))]),
        };
        // As flattened meta (a WXR export).
        let meta: Map<String, Value> = serde_json::from_value(json!({
            "blocks": ["hero"], "blocks_0_pic": "7", "blocks_0_on": "1",
            "blocks_0_when": "20240301", "blocks_0_rel": ["1", "99"],
        }))
        .unwrap();
        let got = fields(&defs, None, &meta, &mut cx);
        let FieldValue::List(rows) = &got[0].1 else {
            panic!("{got:?}")
        };
        let FieldValue::Object(o) = &rows[0] else {
            panic!()
        };
        assert_eq!(o["layout"], FieldValue::Text("hero".into()));
        assert_eq!(o["pic"], FieldValue::Media(f.media));
        assert_eq!(o["on"], FieldValue::Bool(true));
        assert!(matches!(o["when"], FieldValue::Date(_)));
        assert_eq!(
            o["rel"],
            FieldValue::List(vec![FieldValue::Doc(f.about), FieldValue::Null])
        );
        assert!(
            issues
                .iter()
                .any(|i| i.kind == IssueKind::UnresolvedRelation)
        );
    }
}
