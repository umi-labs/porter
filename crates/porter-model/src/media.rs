use crate::ItemId;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Usage {
    pub item: ItemId,
    pub field: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Media {
    pub id: ItemId,
    pub hash: String,
    pub variants: Vec<String>,
    pub filename: String,
    pub mime: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt: Option<String>,
    pub caption: Option<String>,
    pub title: Option<String>,
    pub credit: Option<String>,
    pub folder: Option<String>,
    /// Focal point, percent 0-100 from the top-left.
    #[serde(default)]
    pub focal_x: Option<f64>,
    #[serde(default)]
    pub focal_y: Option<f64>,
    pub usages: Vec<Usage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_roundtrip_without_focal() {
        use crate::{ItemKind, SourceRef};
        let src = SourceRef {
            connector: "media".into(),
            source_id: "h".into(),
            url: None,
        };
        let m = Media {
            id: ItemId::new(ItemKind::Media, &src),
            hash: "h".into(),
            variants: vec![],
            filename: "a.png".into(),
            mime: "image/png".into(),
            width: None,
            height: None,
            alt: None,
            caption: None,
            title: None,
            credit: None,
            folder: None,
            focal_x: Some(1.0),
            focal_y: Some(2.0),
            usages: vec![],
        };
        let mut old = serde_json::to_value(&m).unwrap();
        old.as_object_mut()
            .unwrap()
            .retain(|k, _| !k.starts_with("focal"));
        let back: Media = serde_json::from_value(old).unwrap();
        assert_eq!((back.focal_x, back.focal_y), (None, None));
        assert_eq!(
            serde_json::from_value::<Media>(serde_json::to_value(&m).unwrap()).unwrap(),
            m
        );
    }
}
