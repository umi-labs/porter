use crate::FieldValue;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum BlockKind {
    Section,
    Row,
    Column { width: Option<f32> },
    RichText,
    Image,
    Gallery,
    Button,
    Video,
    Embed,
    Map,
    FormRef,
    Tabs,
    Accordion,
    Slider,
    Spacer,
    Divider,
    Raw,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Original {
    pub builder: String,
    pub module: String,
    pub classes: Vec<String>,
    pub settings: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub props: IndexMap<String, FieldValue>,
    pub children: Vec<Block>,
    pub original: Option<Original>,
}

impl Block {
    /// Depth-first, parent before children.
    pub fn walk(&self, f: &mut impl FnMut(&Block)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(kind: BlockKind, children: Vec<Block>) -> Block {
        Block { kind, props: IndexMap::new(), children, original: None }
    }

    #[test]
    fn walk_visits_depth_first() {
        let tree = b(BlockKind::Section, vec![b(
            BlockKind::Row,
            vec![
                b(BlockKind::Column { width: Some(0.5) }, vec![b(BlockKind::Image, vec![])]),
                b(BlockKind::Column { width: None }, vec![b(BlockKind::RichText, vec![])]),
            ],
        )]);
        let mut seen = vec![];
        tree.walk(&mut |x| seen.push(x.kind.clone()));
        assert_eq!(
            seen,
            vec![
                BlockKind::Section,
                BlockKind::Row,
                BlockKind::Column { width: Some(0.5) },
                BlockKind::Image,
                BlockKind::Column { width: None },
                BlockKind::RichText
            ]
        );
    }

    #[test]
    fn layout_roundtrip() {
        let mut blk = b(BlockKind::Button, vec![b(BlockKind::Spacer, vec![])]);
        blk.props.insert("label".into(), FieldValue::Text("Go".into()));
        blk.original = Some(Original {
            builder: "divi".into(),
            module: "et_pb_button".into(),
            classes: vec!["a".into()],
            settings: serde_json::json!({"k": [1, 2]}),
        });
        let j = serde_json::to_string(&blk).unwrap();
        assert_eq!(serde_json::from_str::<Block>(&j).unwrap(), blk);
    }
}
