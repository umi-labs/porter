//! Gated: runs only when PORTER_PAYLOAD_TEST_URL and PORTER_PAYLOAD_API_KEY are set
//! (see tests/payload-app). Rerunnable against the same database.
use porter_engine::destinations::payload::client::PayloadClient;
use serde_json::json;

/// A valid 1x1 transparent PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

fn client() -> Option<PayloadClient> {
    let url = std::env::var("PORTER_PAYLOAD_TEST_URL").ok()?;
    let key = std::env::var("PORTER_PAYLOAD_API_KEY").ok()?;
    Some(PayloadClient::new(&url, &key, "users").unwrap())
}

#[tokio::test]
async fn live_upload_and_upsert() {
    let Some(c) = client() else {
        eprintln!("skipped: PORTER_PAYLOAD_TEST_URL / PORTER_PAYLOAD_API_KEY not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("pixel.png");
    std::fs::write(&f, PNG).unwrap();
    let (media, _) = c
        .upload_keyed(
            "media",
            "legacyId",
            &json!("live-pixel"),
            &f,
            "pixel.png",
            "image/png",
            &json!({"alt": "Pixel"}),
        )
        .await
        .unwrap();
    assert_eq!(media["alt"], "Pixel");
    assert_eq!(media["mimeType"], "image/png");

    let data = json!({
        "title": "Live page",
        "layout": [{"blockType": "image", "image": media["id"]}],
        "seo": {"title": "SEO", "image": null},
        "parent": null,
    });
    let fresh = c
        .find_by("pages", "legacyId", &json!("live-1"))
        .await
        .unwrap()
        .is_none();
    let (first, created) = c
        .upsert("pages", "legacyId", &json!("live-1"), &data, false)
        .await
        .unwrap();
    assert_eq!(created, fresh);
    assert_eq!(first["_status"], "published");
    assert_eq!(first["legacyId"], "live-1");

    let mut data2 = data.clone();
    data2["title"] = json!("Live page 2");
    let (second, created) = c
        .upsert("pages", "legacyId", &json!("live-1"), &data2, true)
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second["title"], "Live page 2");
    assert_eq!(second["_status"], "draft");
    // find_by errors on a duplicate key, so this also proves no second doc was made.
    let found = c
        .find_by("pages", "legacyId", &json!("live-1"))
        .await
        .unwrap();
    assert_eq!(found.unwrap()["id"], first["id"]);
}

/// Generated Lexical and blocks are accepted by Payload and read back unchanged.
#[tokio::test]
async fn live_lexical_and_blocks_round_trip() {
    use indexmap::IndexMap;
    use porter_engine::destinations::payload::blocks::to_blocks;
    use porter_engine::destinations::payload::lexical::{DestRef, MemIdMap};
    use porter_engine::mapping::{BlockMap, BlockMatch};
    use porter_model::{
        Align, Block, BlockKind, FieldValue, Inline, ItemId, ItemKind, LinkTarget, Marks, RichText,
        RtBlock, SourceRef,
    };

    let Some(c) = client() else {
        eprintln!("skipped: PORTER_PAYLOAD_TEST_URL / PORTER_PAYLOAD_API_KEY not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("pixel.png");
    std::fs::write(&f, PNG).unwrap();
    let (media, _) = c
        .upload_keyed(
            "media",
            "legacyId",
            &json!("live-pixel"),
            &f,
            "pixel.png",
            "image/png",
            &json!({"alt": "Pixel"}),
        )
        .await
        .unwrap();
    let (post, _) = c
        .upsert(
            "posts",
            "legacyId",
            &json!("live-post"),
            &json!({"title": "Target"}),
            false,
        )
        .await
        .unwrap();

    let id = |k, s: &str| {
        ItemId::new(
            k,
            &SourceRef {
                connector: "live".into(),
                source_id: s.into(),
                url: None,
            },
        )
    };
    let (doc, pic) = (id(ItemKind::Document, "post"), id(ItemKind::Media, "pic"));
    let mut ids = MemIdMap::default();
    ids.ids.insert(
        doc,
        DestRef {
            collection: "posts".into(),
            id: post["id"].clone(),
            path: None,
        },
    );
    ids.ids.insert(
        pic,
        DestRef {
            collection: "media".into(),
            id: media["id"].clone(),
            path: None,
        },
    );
    let t = |s: &str, bold| Inline::Text {
        text: s.into(),
        marks: Marks {
            bold,
            ..Marks::default()
        },
    };
    let rt = RichText {
        blocks: vec![
            RtBlock::Heading {
                level: 2,
                inlines: vec![t("Head", false)],
                align: Align::None,
            },
            RtBlock::Paragraph {
                inlines: vec![
                    t("bold", true),
                    Inline::Break,
                    Inline::Link {
                        target: LinkTarget::Doc(doc),
                        new_tab: false,
                        inlines: vec![t("int", false)],
                        fragment: None,
                    },
                    Inline::Link {
                        target: LinkTarget::Url("https://x.example.test/a b".into()),
                        new_tab: true,
                        inlines: vec![t("ext", false)],
                        fragment: None,
                    },
                ],
                align: Align::Center,
            },
            RtBlock::List {
                ordered: true,
                start: 1,
                items: vec![vec![RtBlock::Paragraph {
                    inlines: vec![t("one", false)],
                    align: Align::None,
                }]],
            },
            RtBlock::Quote {
                blocks: vec![RtBlock::Paragraph {
                    inlines: vec![t("q", false)],
                    align: Align::None,
                }],
            },
            RtBlock::Rule,
            RtBlock::Media {
                media: pic,
                caption: None,
                align: Align::None,
                width: None,
                link: None,
            },
        ],
    };
    let block = |kind, props: Vec<(&str, FieldValue)>| Block {
        kind,
        props: props
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<IndexMap<_, _>>(),
        children: vec![],
        original: None,
    };
    let layout = vec![Block {
        children: vec![
            block(
                BlockKind::RichText,
                vec![("text", FieldValue::RichText(rt))],
            ),
            block(BlockKind::Image, vec![("media", FieldValue::Media(pic))]),
            block(
                BlockKind::Raw,
                vec![("html", FieldValue::Text("<i>raw</i>".into()))],
            ),
        ],
        ..block(BlockKind::Section, vec![])
    }];
    let map = |kind: &str, to: &str, from: &str, field: &str| BlockMap {
        r#match: BlockMatch {
            kind: Some(kind.into()),
            module: None,
        },
        to: to.into(),
        fields: [(from.to_string(), field.to_string())]
            .into_iter()
            .collect(),
    };
    let maps = [
        map("rich-text", "richText", "text", "content"),
        map("image", "image", "media", "image"),
        map("raw", "raw", "html", "html"),
    ];
    let (blocks, issues) = to_blocks(&layout, &maps, &ids, None);
    assert!(issues.is_empty(), "{issues:?}");

    let (page, _) = c
        .upsert(
            "pages",
            "legacyId",
            &json!("live-lexical"),
            &json!({"title": "Lexical", "layout": blocks}),
            false,
        )
        .await
        .unwrap();
    let back = c
        .find_by("pages", "legacyId", &json!("live-lexical"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back["id"], page["id"]);
    let got = back["layout"].as_array().unwrap();
    let types: Vec<_> = got
        .iter()
        .map(|b| b["blockType"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["richText", "image", "raw"]);
    assert_eq!(
        got[0]["content"], blocks[0]["content"],
        "Lexical read back unchanged"
    );
    assert_eq!(got[1]["image"], media["id"]);
    assert_eq!(got[2]["html"], "<i>raw</i>");
}
