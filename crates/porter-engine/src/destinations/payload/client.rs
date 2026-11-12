//! Payload REST client. Porter is Rust, so it writes through REST, not the Local API.
//! The API key lives only in the `Authorization` header (marked sensitive); errors carry
//! method, URL and Payload's messages, never headers.
use crate::http::send_with_retry;
use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method, RequestBuilder, redirect};
use serde_json::Value;
use std::path::Path;

const RETRIES: u32 = 5;
const MAX_VALUE: usize = 200;

pub struct PayloadClient {
    http: Client,
    base: String,
    auth: HeaderValue,
}

impl PayloadClient {
    pub fn new(base: &str, api_key: &str, auth_collection: &str) -> Self {
        let mut auth = HeaderValue::from_str(&format!("{auth_collection} API-Key {api_key}"))
            .unwrap_or_else(|_| HeaderValue::from_static(""));
        auth.set_sensitive(true);
        let http = Client::builder()
            .user_agent(concat!("porter/", env!("CARGO_PKG_VERSION")))
            // Never replay the API key to wherever a redirect points.
            .redirect(redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            auth,
        }
    }

    fn req(&self, m: Method, path: &str) -> RequestBuilder {
        self.http
            .request(m, format!("{}/api/{path}", self.base))
            .header(AUTHORIZATION, self.auth.clone())
    }

    async fn send(&self, req: RequestBuilder) -> Result<Value> {
        let (method, url) = match req.try_clone().and_then(|r| r.build().ok()) {
            Some(r) => (r.method().to_string(), r.url().to_string()),
            None => ("REQUEST".into(), self.base.clone()),
        };
        let res = send_with_retry(req, RETRIES)
            .await
            .with_context(|| format!("{method} {url}"))?;
        let status = res.status();
        if status.is_redirection() {
            bail!("{method} {url}: unexpected redirect (HTTP {status}); check destination.url");
        }
        let text = res.text().await.unwrap_or_default();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            bail!(
                "{method} {url}: HTTP {status}: {}",
                describe_errors(&body, &text)
            );
        }
        if body.is_null() {
            bail!("{method} {url}: response is not JSON: {}", truncate(&text));
        }
        Ok(body)
    }

    /// The doc whose `field` equals `value`, drafts included. More than one match is an
    /// error: an upsert key must be unique, and picking one would silently fork content.
    pub async fn find_by(
        &self,
        collection: &str,
        field: &str,
        value: &Value,
    ) -> Result<Option<Value>> {
        let v = key_string(value)?;
        let req = self.req(Method::GET, collection).query(&[
            (format!("where[{field}][equals]"), v.as_str()),
            ("limit".into(), "1"),
            ("depth".into(), "0"),
            ("draft".into(), "true"),
        ]);
        let body = self.send(req).await?;
        if body["totalDocs"].as_u64().unwrap_or(0) > 1 {
            bail!("duplicate key {field}={v} in {collection}");
        }
        Ok(body["docs"].as_array().and_then(|d| d.first()).cloned())
    }

    pub async fn create(&self, collection: &str, data: &Value, draft: bool) -> Result<Value> {
        let req = draft_query(self.req(Method::POST, collection), draft);
        let body = self.send(req.json(&prepare(data, draft))).await?;
        Ok(doc(body))
    }

    pub async fn update(
        &self,
        collection: &str,
        id: &Value,
        data: &Value,
        draft: bool,
    ) -> Result<Value> {
        let path = format!("{collection}/{}", key_string(id)?);
        let req = draft_query(self.req(Method::PATCH, &path), draft);
        let body = self.send(req.json(&prepare(data, draft))).await?;
        Ok(doc(body))
    }

    /// Updates the doc keyed by `key_field = key`, or creates it (with the key set).
    /// Returns the doc and whether it was created.
    pub async fn upsert(
        &self,
        collection: &str,
        key_field: &str,
        key: &Value,
        data: &Value,
        draft: bool,
    ) -> Result<(Value, bool)> {
        match self.find_by(collection, key_field, key).await? {
            Some(found) => {
                let id = found
                    .get("id")
                    .ok_or_else(|| anyhow!("{collection} doc without id"))?;
                Ok((self.update(collection, id, data, draft).await?, false))
            }
            None => {
                let mut data = data.clone();
                if let Some(o) = data.as_object_mut() {
                    o.insert(key_field.into(), key.clone());
                }
                Ok((self.create(collection, &data, draft).await?, true))
            }
        }
    }

    /// Multipart upload: a `file` part and a `_payload` JSON part.
    /// Streaming bodies can't be cloned, so this gets one attempt (no retry).
    pub async fn upload(
        &self,
        collection: &str,
        file: &Path,
        filename: &str,
        mime: &str,
        data: &Value,
    ) -> Result<Value> {
        let bytes = tokio::fs::read(file)
            .await
            .with_context(|| format!("reading {}", file.display()))?;
        let part = Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(mime)?;
        let form = Form::new().part("file", part).text(
            "_payload",
            serde_json::to_string(&strip_nulls(data.clone()))?,
        );
        let body = self
            .send(self.req(Method::POST, collection).multipart(form))
            .await?;
        Ok(doc(body))
    }

    pub async fn update_global(&self, slug: &str, data: &Value) -> Result<Value> {
        let req = self.req(Method::POST, &format!("globals/{slug}"));
        let body = self.send(req.json(&strip_nulls(data.clone()))).await?;
        Ok(match body.get("result") {
            Some(r) => r.clone(),
            None => body,
        })
    }

    pub async fn count(&self, collection: &str) -> Result<u64> {
        let body = self
            .send(self.req(Method::GET, collection).query(&[("limit", "0")]))
            .await?;
        body["totalDocs"]
            .as_u64()
            .ok_or_else(|| anyhow!("GET /api/{collection}: no totalDocs in response"))
    }
}

/// `draft=true` writes a draft version; without it Payload writes the published row.
fn draft_query(req: RequestBuilder, draft: bool) -> RequestBuilder {
    if draft {
        req.query(&[("draft", "true")])
    } else {
        req
    }
}

/// Sets `_status` to match the draft flag (so the main row is never left stale) and strips nulls.
fn prepare(data: &Value, draft: bool) -> Value {
    let mut v = strip_nulls(data.clone());
    if let Some(o) = v.as_object_mut() {
        let status = if draft { "draft" } else { "published" };
        o.insert("_status".into(), Value::String(status.into()));
    }
    v
}

/// Drops `null` object members recursively: Payload rejects `null` for array/blocks fields.
// ponytail: also drops nulls meant to clear scalar fields; send per-field empties once the
// writer knows the schema (Task 20).
fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(strip_nulls).collect()),
        v => v,
    }
}

fn doc(mut body: Value) -> Value {
    match body.get_mut("doc") {
        Some(d) => d.take(),
        None => body,
    }
}

fn key_string(v: &Value) -> Result<String> {
    Ok(match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => bail!("unsupported key value {other}"),
    })
}

fn truncate(s: &str) -> String {
    match s.char_indices().nth(MAX_VALUE) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Payload's `{errors:[{message, data:{errors:[{path,message}]}}]}` as one line.
fn describe_errors(body: &Value, raw: &str) -> String {
    let Some(errors) = body["errors"].as_array() else {
        return truncate(raw);
    };
    errors
        .iter()
        .map(|e| {
            let msg = truncate(e["message"].as_str().unwrap_or("error"));
            let fields: Vec<String> = e["data"]["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    let path = f["path"].as_str().or(f["field"].as_str()).unwrap_or("?");
                    format!("{path}: {}", truncate(f["message"].as_str().unwrap_or("")))
                })
                .collect();
            if fields.is_empty() {
                msg
            } else {
                format!("{msg} ({})", fields.join("; "))
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}
