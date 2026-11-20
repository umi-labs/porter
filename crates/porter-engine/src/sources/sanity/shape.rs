//! Classifies a Sanity field value by its shape, so normalise can pick a converter.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

#[derive(Debug, PartialEq)]
pub(crate) enum Shape {
    Reference { id: String, weak: bool },
    Image,
    File,
    PortableText,
    Builder,
    Slug(String),
    Geopoint { lat: f64, lng: f64 },
    Date(DateTime<Utc>),
    Object,
    List,
    Scalar,
}

fn ty(v: &Value) -> Option<&str> {
    v.get("_type")?.as_str()
}

// used by normalise (Task 6)
#[allow(dead_code)]
pub(crate) fn shape(v: &Value) -> Shape {
    match v {
        Value::String(s) => parse_date(s).map_or(Shape::Scalar, Shape::Date),
        Value::Array(a) => {
            let typed = !a.is_empty() && a.iter().all(|x| x.is_object() && ty(x).is_some());
            if !typed {
                Shape::List
            } else if a.iter().any(|x| ty(x) == Some("block")) {
                Shape::PortableText
            } else if a.iter().all(|x| ty(x) == Some("reference")) {
                Shape::List
            } else {
                Shape::Builder
            }
        }
        Value::Object(o) => match ty(v) {
            Some("reference") => match o.get("_ref").and_then(Value::as_str) {
                Some(id) => Shape::Reference {
                    id: id.to_string(),
                    weak: o.get("_weak").and_then(Value::as_bool).unwrap_or(false),
                },
                None => Shape::Object,
            },
            Some("image") => Shape::Image,
            Some("file") => Shape::File,
            Some("slug") => match o.get("current").and_then(Value::as_str) {
                Some(s) => Shape::Slug(s.to_string()),
                None => Shape::Object,
            },
            Some("geopoint") => match (
                o.get("lat").and_then(Value::as_f64),
                o.get("lng").and_then(Value::as_f64),
            ) {
                (Some(lat), Some(lng)) => Shape::Geopoint { lat, lng },
                _ => Shape::Object,
            },
            _ => Shape::Object,
        },
        _ => Shape::Scalar,
    }
}

fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc())
}

/// An image/file object's asset reference (`image-<sha1>-<w>x<h>-<ext>`).
// used by normalise (Task 6)
#[allow(dead_code)]
pub(crate) fn asset_ref(v: &Value) -> Option<&str> {
    v.get("asset")?.get("_ref")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shape_classifies_each_kind() {
        assert_eq!(
            shape(&json!({"_type":"reference","_ref":"abc","_weak":true})),
            Shape::Reference {
                id: "abc".into(),
                weak: true
            }
        );
        assert_eq!(
            shape(&json!({"_type":"reference","_ref":"abc"})),
            Shape::Reference {
                id: "abc".into(),
                weak: false
            }
        );
        assert_eq!(
            shape(&json!({"_type":"image","asset":{"_ref":"image-a-1x1-png"}})),
            Shape::Image
        );
        assert_eq!(shape(&json!({"_type":"file"})), Shape::File);
        assert_eq!(
            shape(&json!([{"_type":"block"}, {"_type":"image"}])),
            Shape::PortableText
        );
        assert_eq!(
            shape(&json!([{"_type":"hero"}, {"_type":"cta"}])),
            Shape::Builder
        );
        assert_eq!(
            shape(&json!([{"_type":"reference","_ref":"a"}, {"_type":"reference","_ref":"b"}])),
            Shape::List
        );
        assert_eq!(shape(&json!(["a", "b"])), Shape::List);
        assert_eq!(
            shape(&json!({"_type":"slug","current":"hi"})),
            Shape::Slug("hi".into())
        );
        assert_eq!(
            shape(&json!({"_type":"geopoint","lat":1.5,"lng":-2.0})),
            Shape::Geopoint {
                lat: 1.5,
                lng: -2.0
            }
        );
        assert!(matches!(shape(&json!("2024-01-22")), Shape::Date(_)));
        assert!(matches!(
            shape(&json!("2024-01-22T10:00:00Z")),
            Shape::Date(_)
        ));
        assert_eq!(shape(&json!("next week")), Shape::Scalar);
        assert_eq!(shape(&json!({"a":1})), Shape::Object);
        assert_eq!(shape(&json!(3)), Shape::Scalar);
    }

    #[test]
    fn asset_ref_reads_nested_ref() {
        assert_eq!(
            asset_ref(&json!({"_type":"image","asset":{"_ref":"image-x-1x1-png"}})),
            Some("image-x-1x1-png")
        );
        assert_eq!(asset_ref(&json!({"_type":"image"})), None);
    }
}
