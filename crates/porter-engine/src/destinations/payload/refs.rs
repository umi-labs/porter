//! The references in a loaded Payload doc (depth 0), for verify: polymorphic
//! `{relationTo, value}` anywhere, Lexical internal links and upload/relationship nodes,
//! nested-docs `parent`, and, when the Payload config was read, bare ids in
//! relationship/upload fields (top level, groups, arrays, blocks).

use super::ts::{DestField, DestSchema};
use crate::stage::{DestReference, LoadedRefs};
use indexmap::IndexMap;
use serde_json::Value;

pub fn references(loaded: &Value, collection: &str, schema: Option<&DestSchema>) -> LoadedRefs {
    let coll = schema.and_then(|s| s.collections.get(collection));
    let mut w = W {
        blocks: schema.map(|s| &s.blocks),
        out: vec![],
    };
    if let Some(o) = loaded.as_object() {
        for (k, v) in o {
            if k == "parent" {
                // nested-docs: same collection, a bare id at depth 0.
                if let Some(id) = scalar(v) {
                    w.out.push(DestReference {
                        field: k.clone(),
                        collection: collection.to_string(),
                        id: id.clone(),
                        parent: true,
                    });
                }
                continue;
            }
            let f = coll.and_then(|c| c.fields.iter().find(|f| &f.name == k));
            w.walk(v, k, f);
        }
    }
    LoadedRefs {
        refs: w.out,
        complete: coll.is_some(),
    }
}

fn scalar(v: &Value) -> Option<&Value> {
    (v.is_number() || v.is_string()).then_some(v)
}

/// `{relationTo, value}` as Payload stores a polymorphic relation.
fn polymorphic(v: &Value) -> Option<(&str, &Value)> {
    let o = v.as_object()?;
    if o.len() != 2 {
        return None;
    }
    Some((o.get("relationTo")?.as_str()?, scalar(o.get("value")?)?))
}

fn is_lexical(v: &Value) -> bool {
    v.get("root")
        .and_then(|r| r.get("type"))
        .and_then(Value::as_str)
        == Some("root")
}

struct W<'a> {
    blocks: Option<&'a IndexMap<String, Vec<DestField>>>,
    out: Vec<DestReference>,
}

impl W<'_> {
    fn push(&mut self, field: &str, collection: &str, id: &Value) {
        self.out.push(DestReference {
            field: field.to_string(),
            collection: collection.to_string(),
            id: id.clone(),
            parent: false,
        });
    }

    fn walk(&mut self, v: &Value, path: &str, f: Option<&DestField>) {
        if let Some(f) = f.filter(|f| matches!(f.kind.as_str(), "relationship" | "upload")) {
            let items = match v {
                Value::Array(a) => a.iter().collect(),
                v => vec![v],
            };
            for x in items {
                match (polymorphic(x), scalar(x), &f.relation_to[..]) {
                    (Some((c, id)), ..) => self.push(path, c, id),
                    // A bare id names its collection only when the field takes one.
                    (None, Some(id), [c]) => self.push(path, c, id),
                    _ => {}
                }
            }
            return;
        }
        if let Some((c, id)) = polymorphic(v) {
            return self.push(path, c, id);
        }
        if is_lexical(v) {
            return self.lexical(&v["root"], path);
        }
        match v {
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    self.walk(x, &format!("{path}.{i}"), f);
                }
            }
            Value::Object(o) => {
                // A block reads its slug's fields; a group or array row its field's sub-fields.
                let fields = match o.get("blockType").and_then(Value::as_str) {
                    Some(slug) => self.blocks.and_then(|b| b.get(slug)).map(|f| &f[..]),
                    None => f.map(|f| &f.fields[..]),
                };
                for (k, x) in o {
                    let sub = fields.and_then(|fs| fs.iter().find(|f| &f.name == k));
                    self.walk(x, &format!("{path}.{k}"), sub);
                }
            }
            _ => {}
        }
    }

    fn lexical(&mut self, node: &Value, path: &str) {
        match node["type"].as_str() {
            Some("link" | "autolink") => {
                if let Some((c, id)) = polymorphic(&node["fields"]["doc"]) {
                    self.push(path, c, id);
                }
            }
            Some("upload" | "relationship") => {
                if let (Some(c), Some(id)) = (node["relationTo"].as_str(), scalar(&node["value"])) {
                    self.push(path, c, id);
                }
            }
            Some("block") => self.walk(&node["fields"], path, None),
            _ => {}
        }
        if let Some(kids) = node["children"].as_array() {
            for k in kids {
                self.lexical(k, path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destinations::payload::ts::CollectionSchema;
    use serde_json::json;

    fn field(name: &str, kind: &str, to: &[&str]) -> DestField {
        DestField {
            name: name.into(),
            kind: kind.into(),
            relation_to: to.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn finds_every_reference_shape() {
        let mut schema = DestSchema::default();
        schema.collections.insert(
            "pages".into(),
            CollectionSchema {
                fields: vec![
                    field("hero", "upload", &["media"]),
                    field("layout", "blocks", &[]),
                ],
                ..Default::default()
            },
        );
        schema.blocks.insert(
            "card".into(),
            vec![field("page", "relationship", &["pages"])],
        );
        let doc = json!({
            "id": 1, "parent": 4, "hero": 7, "width": 7,
            "related": [{"relationTo": "posts", "value": 2}],
            "layout": [{"blockType": "card", "page": 9, "size": 9}],
            "content": {"root": {"type": "root", "children": [
                {"type": "paragraph", "children": [
                    {"type": "link", "fields": {"linkType": "internal",
                        "doc": {"relationTo": "pages", "value": 3}}, "children": []},
                    {"type": "link", "fields": {"linkType": "custom", "url": "/x"}}
                ]},
                {"type": "upload", "relationTo": "media", "value": 5, "fields": {}}
            ]}}
        });
        let got = references(&doc, "pages", Some(&schema));
        assert!(got.complete);
        let mut seen: Vec<(String, String, String, bool)> = got
            .refs
            .iter()
            .map(|r| {
                (
                    r.field.clone(),
                    r.collection.clone(),
                    r.id.to_string(),
                    r.parent,
                )
            })
            .collect();
        seen.sort();
        let want = [
            ("content", "media", "5", false),
            ("content", "pages", "3", false),
            ("hero", "media", "7", false),
            ("layout.0.page", "pages", "9", false),
            ("parent", "pages", "4", true),
            ("related.0", "posts", "2", false),
        ];
        let want: Vec<_> = want
            .iter()
            .map(|(a, b, c, d)| (a.to_string(), b.to_string(), c.to_string(), *d))
            .collect();
        assert_eq!(seen, want);
        // No schema: bare ids are invisible, and the result says so.
        let got = references(&doc, "pages", None);
        assert!(!got.complete);
        assert_eq!(got.refs.len(), 4);
    }
}
