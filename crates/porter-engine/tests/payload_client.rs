use porter_engine::destinations::payload::client::PayloadClient;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

fn has_query(r: &Request, k: &str) -> bool {
    r.url.query_pairs().any(|(key, _)| key == k)
}

#[tokio::test]
async fn auth_header_format() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .and(header("Authorization", "editors API-Key k3y"))
        .and(query_param("limit", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"docs": [], "totalDocs": 7})))
        .expect(1)
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k3y", "editors").unwrap();
    assert_eq!(c.count("pages").await.unwrap(), 7);
}

#[tokio::test]
async fn upsert_creates_then_updates() {
    let s = MockServer::start().await;
    // First run: nothing matches, so the client POSTs.
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .and(query_param("where[legacyId][equals]", "42"))
        .and(query_param("limit", "1"))
        .and(query_param("depth", "0"))
        .and(query_param("draft", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"docs": [], "totalDocs": 0})))
        .up_to_n_times(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages"))
        .and(body_partial_json(json!({"title": "Hi", "legacyId": 42})))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"doc": {"id": 9, "title": "Hi"}})),
        )
        .expect(1)
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    let (doc, created) = c
        .upsert(
            "pages",
            "legacyId",
            &json!(42),
            &json!({"title": "Hi"}),
            false,
        )
        .await
        .unwrap();
    assert!(created);
    assert_eq!(doc["id"], 9);

    // Second run: the doc is found, so the client PATCHes its id.
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .and(query_param("where[legacyId][equals]", "42"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"docs": [{"id": 9, "legacyId": 42}], "totalDocs": 1})),
        )
        .mount(&s)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/pages/9"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"doc": {"id": 9, "title": "Hi 2"}})),
        )
        .expect(1)
        .mount(&s)
        .await;
    let (doc, created) = c
        .upsert(
            "pages",
            "legacyId",
            &json!(42),
            &json!({"title": "Hi 2"}),
            false,
        )
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(doc["title"], "Hi 2");
}

#[tokio::test]
async fn upsert_refuses_duplicate_keys() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"docs": [{"id": 1}], "totalDocs": 2})),
        )
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    let e = c
        .upsert("pages", "slug", &json!("a b&c"), &json!({}), false)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("duplicate key slug=a b&c in pages"), "{e}");
    let reqs = s.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1, "never writes on a duplicate");
    let q = reqs[0]
        .url
        .query_pairs()
        .find(|(k, _)| k == "where[slug][equals]");
    assert_eq!(
        q.unwrap().1,
        "a b&c",
        "value round-trips through percent-encoding"
    );
}

#[tokio::test]
async fn draft_flag_and_status() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/posts"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"doc": {"id": 1}})))
        .mount(&s)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/posts/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"doc": {"id": 1}})))
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    c.create("posts", &json!({"title": "P"}), false)
        .await
        .unwrap();
    c.create("posts", &json!({"title": "D"}), true)
        .await
        .unwrap();
    c.update("posts", &json!(1), &json!({"title": "U"}), true)
        .await
        .unwrap();
    let reqs = s.received_requests().await.unwrap();
    assert_eq!(body(&reqs[0])["_status"], "published");
    assert!(!has_query(&reqs[0], "draft"));
    assert_eq!(body(&reqs[1])["_status"], "draft");
    assert!(
        reqs[1]
            .url
            .query_pairs()
            .any(|(k, v)| k == "draft" && v == "true")
    );
    assert_eq!(body(&reqs[2])["_status"], "draft");
    assert!(has_query(&reqs[2], "draft"));
}

#[tokio::test]
async fn nulls_stripped() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"doc": {"id": 1}})))
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    c.create(
        "pages",
        &json!({"title": "T", "layout": null, "seo": {"image": null, "title": "S"}, "tags": [null, "a"]}),
        false,
    )
    .await
    .unwrap();
    let b = body(&s.received_requests().await.unwrap()[0]);
    assert!(b.get("layout").is_none(), "{b}");
    assert!(b["seo"].get("image").is_none(), "{b}");
    assert_eq!(b["seo"]["title"], "S");
    assert_eq!(
        b["tags"],
        json!([null, "a"]),
        "array elements are left alone"
    );
}

#[tokio::test]
async fn validation_errors_name_fields_not_key() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"errors": [{
            "name": "ValidationError",
            "message": "The following field is invalid: Title",
            "data": {"errors": [{"path": "title", "message": "x".repeat(500)}]}
        }]})))
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "sekrit-key", "users").unwrap();
    let e = c
        .create("pages", &json!({}), false)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("400"), "{e}");
    assert!(e.contains("The following field is invalid: Title"), "{e}");
    assert!(e.contains("title: "), "{e}");
    assert!(
        !e.contains(&"x".repeat(201)),
        "values truncated to 200 chars: {e}"
    );
    assert!(!e.contains("sekrit-key"), "{e}");
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "https://elsewhere.test/steal"),
        )
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    let e = c.count("pages").await.unwrap_err().to_string();
    assert!(e.contains("redirect"), "{e}");
    assert!(e.contains("/api/pages"), "{e}");
}

#[tokio::test]
async fn upload_sends_file_and_payload_parts() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/media"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"doc": {"id": 3}})))
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.png");
    std::fs::write(&f, b"PNGDATA").unwrap();
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    Mock::given(method("GET"))
        .and(path("/api/media"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"docs": [], "totalDocs": 0})))
        .mount(&s)
        .await;
    let (doc, created) = c
        .upload_keyed(
            "media",
            "legacyId",
            &json!("m1"),
            &f,
            "pic.png",
            "image/png",
            &json!({"alt": "A"}),
        )
        .await
        .unwrap();
    assert!(created);
    assert_eq!(doc["id"], 3);
    let reqs = s.received_requests().await.unwrap();
    let r = reqs.iter().find(|r| r.method.as_str() == "POST").unwrap();
    let raw = String::from_utf8_lossy(&r.body);
    assert!(raw.contains("name=\"file\"; filename=\"pic.png\""), "{raw}");
    assert!(raw.contains("PNGDATA"));
    assert!(raw.contains("name=\"_payload\""));
    assert!(raw.contains(r#""alt":"A""#), "{raw}");
    assert!(raw.contains(r#""legacyId":"m1""#), "{raw}");
}

#[tokio::test]
async fn update_global_posts_to_globals() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/globals/header"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"result": {"nav": []}, "message": "ok"})),
        )
        .expect(1)
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    let r = c
        .update_global("header", &json!({"nav": []}))
        .await
        .unwrap();
    assert_eq!(r, json!({"nav": []}));
}

fn empty() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"docs": [], "totalDocs": 0}))
}

#[tokio::test]
async fn create_5xx_after_commit_does_not_duplicate() {
    let s = MockServer::start().await;
    // Lookup before the POST finds nothing; lookup after the 503 finds the committed doc.
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .respond_with(empty())
        .up_to_n_times(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"docs": [{"id": 5, "legacyId": "k"}], "totalDocs": 1})),
        )
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    let (doc, created) = c
        .upsert(
            "pages",
            "legacyId",
            &json!("k"),
            &json!({"title": "T"}),
            false,
        )
        .await
        .unwrap();
    assert!(created);
    assert_eq!(doc["id"], 5);
}

#[tokio::test]
async fn create_5xx_before_commit_retries() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .respond_with(empty())
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"doc": {"id": 6}})))
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    let (doc, created) = c
        .upsert(
            "pages",
            "legacyId",
            &json!("k"),
            &json!({"title": "T"}),
            false,
        )
        .await
        .unwrap();
    assert!(created);
    assert_eq!(doc["id"], 6);
    let posts = s
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .count();
    assert_eq!(posts, 2);
}

#[tokio::test]
async fn create_4xx_is_not_retried() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages"))
        .respond_with(empty())
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"errors": [{"message": "bad"}]})),
        )
        .expect(1)
        .mount(&s)
        .await;
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    let e = c
        .upsert("pages", "legacyId", &json!("k"), &json!({}), false)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("bad"), "{e}");
}

#[tokio::test]
async fn upload_5xx_after_commit_does_not_duplicate() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/media"))
        .respond_with(empty())
        .up_to_n_times(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/media"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"docs": [{"id": 8, "legacyId": "m"}], "totalDocs": 1})),
        )
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/media"))
        .respond_with(ResponseTemplate::new(502))
        .expect(1)
        .mount(&s)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.png");
    std::fs::write(&f, b"PNGDATA").unwrap();
    let c = PayloadClient::new(&s.uri(), "k", "users").unwrap();
    let (doc, created) = c
        .upload_keyed(
            "media",
            "legacyId",
            &json!("m"),
            &f,
            "a.png",
            "image/png",
            &json!({}),
        )
        .await
        .unwrap();
    assert!(created);
    assert_eq!(doc["id"], 8);
}

#[test]
fn invalid_api_key_errors() {
    let e = PayloadClient::new("http://x.test", "bad\nkey", "users")
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("API key"), "{e}");
    assert!(!e.contains("bad"), "never echoes the key: {e}");
}
