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
    let fresh = c.count("pages").await.unwrap() == 0;
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
    assert_eq!(c.count("pages").await.unwrap(), 1);
}
