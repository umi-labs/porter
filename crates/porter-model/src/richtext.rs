use crate::ItemId;
use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct RichText {
    pub blocks: Vec<RtBlock>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Marks {
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub strike: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub code: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub sub: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub sup: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum LinkTarget {
    Doc(ItemId),
    Media(ItemId),
    Url(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Inline {
    Text {
        text: String,
        #[serde(default)]
        marks: Marks,
    },
    Link {
        target: LinkTarget,
        #[serde(default)]
        new_tab: bool,
        inlines: Vec<Inline>,
    },
    Break,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RtBlock {
    Paragraph {
        inlines: Vec<Inline>,
        #[serde(default)]
        align: Align,
    },
    Heading {
        level: u8,
        inlines: Vec<Inline>,
        #[serde(default)]
        align: Align,
    },
    List {
        ordered: bool,
        start: u32,
        items: Vec<Vec<RtBlock>>,
    },
    Quote {
        blocks: Vec<RtBlock>,
    },
    Code {
        lang: Option<String>,
        text: String,
    },
    Rule,
    Table {
        rows: Vec<Vec<Vec<RtBlock>>>,
        header: bool,
    },
    Media {
        media: ItemId,
        caption: Option<RichText>,
        #[serde(default)]
        align: Align,
        width: Option<u32>,
    },
    Embed {
        provider: Option<String>,
        url: String,
    },
    Raw {
        html: String,
    },
}

fn inline_text(inlines: &[Inline], out: &mut String) {
    for i in inlines {
        match i {
            Inline::Text { text, .. } => out.push_str(text),
            Inline::Link { inlines, .. } => inline_text(inlines, out),
            Inline::Break => out.push('\n'),
        }
    }
}

fn block_text(b: &RtBlock) -> String {
    let join = |parts: Vec<String>, sep: &str| {
        parts
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(sep)
    };
    let blocks = |bs: &[RtBlock]| join(bs.iter().map(block_text).collect(), "\n");
    match b {
        RtBlock::Paragraph { inlines, .. } | RtBlock::Heading { inlines, .. } => {
            let mut s = String::new();
            inline_text(inlines, &mut s);
            s
        }
        RtBlock::List { items, .. } => join(items.iter().map(|i| blocks(i)).collect(), "\n"),
        RtBlock::Quote { blocks: bs } => blocks(bs),
        RtBlock::Code { text, .. } => text.clone(),
        RtBlock::Table { rows, .. } => join(
            rows.iter()
                .map(|r| join(r.iter().map(|c| blocks(c)).collect(), "\t"))
                .collect(),
            "\n",
        ),
        RtBlock::Media { caption, .. } => caption
            .as_ref()
            .map(RichText::plain_text)
            .unwrap_or_default(),
        RtBlock::Rule | RtBlock::Embed { .. } | RtBlock::Raw { .. } => String::new(),
    }
}

impl RichText {
    /// Text content only: blocks separated by newlines, markup and non-text nodes dropped.
    pub fn plain_text(&self) -> String {
        self.blocks
            .iter()
            .map(block_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ItemId, ItemKind, SourceRef};

    fn txt(s: &str) -> Inline {
        Inline::Text {
            text: s.into(),
            marks: Marks::default(),
        }
    }

    #[test]
    fn plain_text_flattens() {
        let rt = RichText {
            blocks: vec![
                RtBlock::Heading {
                    level: 1,
                    inlines: vec![txt("Title")],
                    align: Align::None,
                },
                RtBlock::Paragraph {
                    inlines: vec![
                        txt("Hello "),
                        Inline::Link {
                            target: LinkTarget::Url("https://example.test".into()),
                            new_tab: false,
                            inlines: vec![txt("world")],
                        },
                    ],
                    align: Align::None,
                },
            ],
        };
        assert_eq!(rt.plain_text(), "Title\nHello world");
    }

    #[test]
    fn richtext_roundtrip() {
        let id = ItemId::new(
            ItemKind::Media,
            &SourceRef {
                connector: "x".into(),
                source_id: "1".into(),
                url: None,
            },
        );
        let rt = RichText {
            blocks: vec![
                RtBlock::List {
                    ordered: true,
                    start: 1,
                    items: vec![vec![RtBlock::Rule]],
                },
                RtBlock::Media {
                    media: id,
                    caption: None,
                    align: Align::Center,
                    width: Some(10),
                },
                RtBlock::Paragraph {
                    inlines: vec![
                        Inline::Break,
                        Inline::Text {
                            text: "a".into(),
                            marks: Marks {
                                bold: true,
                                ..Marks::default()
                            },
                        },
                    ],
                    align: Align::None,
                },
            ],
        };
        let j = serde_json::to_string(&rt).unwrap();
        assert_eq!(serde_json::from_str::<RichText>(&j).unwrap(), rt);
    }
}
