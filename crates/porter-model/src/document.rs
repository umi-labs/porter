use crate::{Block, ItemId, RichText, SourceRef};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DocStatus {
    Published,
    Draft,
    Private,
    Scheduled,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "t", content = "v", rename_all = "kebab-case")]
pub enum FieldValue {
    Null,
    Text(String),
    Number(f64),
    Bool(bool),
    Date(DateTime<Utc>),
    RichText(RichText),
    Layout(Vec<Block>),
    Media(ItemId),
    Doc(ItemId),
    List(Vec<FieldValue>),
    Object(IndexMap<String, FieldValue>),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Seo {
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical: Option<String>,
    pub noindex: bool,
    pub og_image: Option<ItemId>,
    pub og_title: Option<String>,
    pub og_description: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Document {
    pub id: ItemId,
    #[serde(rename = "type")]
    pub doc_type: String,
    pub status: DocStatus,
    pub locale: Option<String>,
    pub translations: Vec<ItemId>,
    pub slug: String,
    pub path: String,
    pub parent: Option<ItemId>,
    pub title: String,
    pub fields: IndexMap<String, FieldValue>,
    pub seo: Seo,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub author: Option<ItemId>,
    pub original: serde_json::Value,
    pub source: SourceRef,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ItemKind;

    #[test]
    fn document_roundtrip() {
        let src = SourceRef { connector: "x".into(), source_id: "1".into(), url: Some("https://example.test/a".into()) };
        let other = ItemId::new(ItemKind::Media, &SourceRef { connector: "x".into(), source_id: "9".into(), url: None });
        let date = DateTime::parse_from_rfc3339("2024-01-02T03:04:05Z").unwrap().with_timezone(&Utc);
        let mut obj = IndexMap::new();
        obj.insert("k".to_string(), FieldValue::Bool(true));
        let mut fields = IndexMap::new();
        for (k, v) in [
            ("null", FieldValue::Null),
            ("text", FieldValue::Text("hi".into())),
            ("number", FieldValue::Number(1.5)),
            ("bool", FieldValue::Bool(false)),
            ("date", FieldValue::Date(date)),
            ("rich", FieldValue::RichText(RichText { blocks: vec![] })),
            ("layout", FieldValue::Layout(vec![])),
            ("media", FieldValue::Media(other)),
            ("doc", FieldValue::Doc(other)),
            ("list", FieldValue::List(vec![FieldValue::Null, FieldValue::Number(2.0)])),
            ("object", FieldValue::Object(obj)),
        ] {
            fields.insert(k.to_string(), v);
        }
        let doc = Document {
            id: ItemId::new(ItemKind::Document, &src),
            doc_type: "post".into(),
            status: DocStatus::Published,
            locale: Some("en".into()),
            translations: vec![other],
            slug: "a".into(),
            path: "/a".into(),
            parent: None,
            title: "A".into(),
            fields,
            seo: Seo { noindex: true, ..Seo::default() },
            created: Some(date),
            updated: None,
            author: None,
            original: serde_json::json!({"x": 1}),
            source: src,
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"type\":\"post\""));
        assert_eq!(serde_json::from_str::<Document>(&json).unwrap(), doc);
    }
}
