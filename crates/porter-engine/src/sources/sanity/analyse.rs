//! Detects which Sanity types are form submissions or singletons, from the data alone.

use serde_json::Value;
use std::collections::BTreeMap;

/// Fewest documents a type needs before it can be detected as submissions.
pub(crate) const MIN_SUBMISSION_DOCS: usize = 50;

/// Field names (lower-cased) that mark a contact form.
const CONTACT: &[&str] = &["email", "firstname", "lastname", "phone", "phonenumber"];

fn ty(d: &Value) -> Option<&str> {
    d.get("_type")?.as_str()
}

fn id(d: &Value) -> &str {
    d.get("_id").and_then(Value::as_str).unwrap_or("")
}

fn by_type<'a>(docs: impl Iterator<Item = &'a Value>) -> BTreeMap<&'a str, Vec<&'a Value>> {
    let mut m: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for d in docs {
        if let Some(t) = ty(d) {
            m.entry(t).or_default().push(d);
        }
    }
    m
}

fn has_contact_key(o: &serde_json::Map<String, Value>) -> bool {
    o.keys()
        .any(|k| CONTACT.contains(&k.to_ascii_lowercase().as_str()))
}

/// A contact field at the top level, or inside a nested object whose key ends in `Details`.
fn contact_like(d: &Value) -> bool {
    let Some(o) = d.as_object() else {
        return false;
    };
    has_contact_key(o)
        || o.iter()
            .any(|(k, v)| k.ends_with("Details") && v.as_object().is_some_and(has_contact_key))
}

/// Types with at least `min_docs` documents, none with a `slug`, and contact-like fields.
pub(crate) fn detect_submissions(docs: &[Value], min_docs: usize) -> Vec<String> {
    by_type(docs.iter())
        .into_iter()
        .filter(|(_, ds)| {
            ds.len() >= min_docs
                && ds.iter().all(|d| d.get("slug").is_none())
                && ds.iter().any(|d| contact_like(d))
        })
        .map(|(t, _)| t.to_string())
        .collect()
}

fn uuid_shaped(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Types with exactly one published document whose `_id` is its `_type` or a fixed name.
// used by analysis and seeding (Task 9)
#[allow(dead_code)]
pub(crate) fn detect_singletons(docs: &[Value]) -> Vec<String> {
    by_type(docs.iter().filter(|d| !id(d).starts_with("drafts.")))
        .into_iter()
        .filter(|(t, ds)| ds.len() == 1 && (id(ds[0]) == *t || !uuid_shaped(id(ds[0]))))
        .map(|(t, _)| t.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn submissions_need_count_no_slug_and_contact_fields() {
        let docs = vec![
            json!({"_id":"a1","_type":"enquiry","name":"A","email":"a@example.com"}),
            json!({"_id":"a2","_type":"enquiry","name":"B"}),
            json!({"_id":"b1","_type":"lead","leadPassengerDetails":{"firstName":"C"}}),
            json!({"_id":"b2","_type":"lead","leadPassengerDetails":{"firstName":"D"}}),
            json!({"_id":"c1","_type":"page","email":"x","slug":{"current":"a"}}),
            json!({"_id":"c2","_type":"page","email":"y"}),
            json!({"_id":"d1","_type":"note","body":"x"}),
            json!({"_id":"d2","_type":"note","body":"y"}),
            json!({"_id":"e1","_type":"single","email":"z"}),
        ];
        assert_eq!(detect_submissions(&docs, 2), ["enquiry", "lead"]);
        assert!(detect_submissions(&docs, MIN_SUBMISSION_DOCS).is_empty());
    }

    #[test]
    fn singletons_have_one_published_doc_and_a_fixed_id() {
        let docs = vec![
            json!({"_id":"siteSettings","_type":"siteSettings"}),
            json!({"_id":"drafts.siteSettings","_type":"siteSettings"}),
            json!({"_id":"home-1","_type":"homepage"}),
            json!({"_id":"0f8fad5b-d9cb-469f-a165-70867728950e","_type":"post"}),
            json!({"_id":"p1","_type":"page"}),
            json!({"_id":"p2","_type":"page"}),
        ];
        assert_eq!(detect_singletons(&docs), ["homepage", "siteSettings"]);
    }
}
