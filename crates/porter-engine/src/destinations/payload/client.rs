//! Payload REST client. Porter is Rust, so it writes through REST, not the Local API.
//! The API key lives only in the `Authorization` header (marked sensitive); errors carry
//! method, URL and Payload's messages, never headers.
use crate::http::{retry_wait, retryable, send_with_retry};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method, RequestBuilder, Response, redirect};
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
    /// Errors if the key (or collection) can't go in an HTTP header; the error never echoes it.
    pub fn new(base: &str, api_key: &str, auth_collection: &str) -> Result<Self> {
        let mut auth = HeaderValue::from_str(&format!("{auth_collection} API-Key {api_key}"))
            .map_err(|_| {
                anyhow!("the Payload API key or auth collection holds characters not allowed in an HTTP header")
            })?;
        auth.set_sensitive(true);
        let http = Client::builder()
            .user_agent(concat!("porter/", env!("CARGO_PKG_VERSION")))
            // Never replay the API key to wherever a redirect points.
            .redirect(redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            auth,
        })
    }

    fn req(&self, m: Method, path: &str) -> RequestBuilder {
        self.http
            .request(m, format!("{}/api/{path}", self.base))
            .header(AUTHORIZATION, self.auth.clone())
    }

    fn label(&self, req: &RequestBuilder) -> String {
        match req.try_clone().and_then(|r| r.build().ok()) {
            Some(r) => format!("{} {}", r.method(), r.url()),
            None => format!("POST {}", self.base),
        }
    }

    /// Idempotent requests only: retried by `send_with_retry`.
    async fn send(&self, req: RequestBuilder) -> Result<Value> {
        let label = self.label(&req);
        let res = send_with_retry(req, RETRIES)
            .await
            .with_context(|| label.clone())?;
        finish(&label, res).await
    }

    /// Creates a doc keyed by `key_field = key` without ever duplicating it. Each POST is
    /// sent once; after a retriable failure (429/5xx/transport) the key is looked up, since
    /// Payload may have committed before failing. Found: that doc is the result. Not found:
    /// back off (http.rs policy) and POST again, up to RETRIES attempts.
    async fn create_once(
        &self,
        collection: &str,
        key_field: &str,
        key: &Value,
        make: impl Fn() -> Result<RequestBuilder>,
    ) -> Result<Value> {
        for attempt in 1..=RETRIES {
            let req = make()?;
            let label = self.label(&req);
            let last = attempt == RETRIES;
            let wait = match req.send().await {
                Ok(res) if !retryable(res.status()) || last => {
                    return finish(&label, res).await.map(doc);
                }
                Ok(res) => retry_wait(attempt, Some(&res)),
                Err(e) if last => return Err(anyhow!(e).context(label)),
                Err(_) => retry_wait(attempt, None),
            };
            if let Some(found) = self.find_by(collection, key_field, key).await? {
                return Ok(found);
            }
            tokio::time::sleep(wait).await;
        }
        bail!("create in {collection}: no attempts made")
    }
}

/// Maps a response to its JSON body, or to an error naming the request and Payload's messages.
async fn finish(label: &str, res: Response) -> Result<Value> {
    let status = res.status();
    if status.is_redirection() {
        bail!("{label}: unexpected redirect (HTTP {status}); check destination.url");
    }
    let text = res.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("{label}: HTTP {status}: {}", describe_errors(&body, &text));
    }
    if body.is_null() {
        bail!("{label}: response is not JSON: {}", truncate(&text));
    }
    Ok(body)
}

impl PayloadClient {
    /// The doc whose `field` equals `value`, drafts included. More than one match is an
    /// error: an upsert key must be unique, and picking one would silently fork content.
    pub async fn find_by(
        &self,
        collection: &str,
        field: &str,
        value: &Value,
    ) -> Result<Option<Value>> {
        let v = key_string(value)?;
        self.find_one(collection, &[(format!("where[{field}][equals]"), v)])
            .await
    }

    /// The one doc matching `filter` (raw `where[...]` query pairs), drafts included.
    /// More than one match is an error.
    pub async fn find_one(
        &self,
        collection: &str,
        filter: &[(String, String)],
    ) -> Result<Option<Value>> {
        let req = self.req(Method::GET, collection).query(filter).query(&[
            ("limit", "1"),
            ("depth", "0"),
            ("draft", "true"),
        ]);
        let body = self.send(req).await?;
        if body["totalDocs"].as_u64().unwrap_or(0) > 1 {
            let what: Vec<String> = filter
                .iter()
                .map(|(k, v)| {
                    let field = k.strip_prefix("where[").and_then(|k| k.split(']').next());
                    format!("{}={v}", field.unwrap_or(k))
                })
                .collect();
            bail!("duplicate key {} in {collection}", what.join("&"));
        }
        Ok(body["docs"].as_array().and_then(|d| d.first()).cloned())
    }

    /// One doc by id (depth 0, latest draft), or None if it doesn't exist.
    pub async fn get(&self, collection: &str, id: &Value) -> Result<Option<Value>> {
        let path = format!("{collection}/{}", key_string(id)?);
        let req = self
            .req(Method::GET, &path)
            .query(&[("depth", "0"), ("draft", "true")]);
        let label = self.label(&req);
        let res = send_with_retry(req, RETRIES)
            .await
            .with_context(|| label.clone())?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        finish(&label, res).await.map(Some)
    }

    /// Unkeyed create: sent once, never retried, since a retry after a commit would
    /// duplicate the doc. Prefer `upsert`, which can retry safely.
    pub async fn create(&self, collection: &str, data: &Value, draft: bool) -> Result<Value> {
        let req =
            draft_query(self.req(Method::POST, collection), draft).json(&prepare(data, draft));
        let label = self.label(&req);
        let res = req.send().await.with_context(|| label.clone())?;
        finish(&label, res).await.map(doc)
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
                let body = prepare(&with_key(data, key_field, key), draft);
                let doc = self
                    .create_once(collection, key_field, key, || {
                        Ok(draft_query(self.req(Method::POST, collection), draft).json(&body))
                    })
                    .await?;
                Ok((doc, true))
            }
        }
    }

    /// Multipart upload (`file` part + `_payload` JSON part) of the media keyed by
    /// `key_field = key`. If that key already exists the existing doc is returned
    /// (created = false) and nothing is uploaded; replacing files is not this call's job.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_keyed(
        &self,
        collection: &str,
        key_field: &str,
        key: &Value,
        file: &Path,
        filename: &str,
        mime: &str,
        data: &Value,
    ) -> Result<(Value, bool)> {
        if let Some(found) = self.find_by(collection, key_field, key).await? {
            return Ok((found, false));
        }
        let bytes = tokio::fs::read(file)
            .await
            .with_context(|| format!("reading {}", file.display()))?;
        let payload = serde_json::to_string(&strip_nulls(with_key(data, key_field, key)))?;
        let doc = self
            .create_once(collection, key_field, key, || {
                let part = Part::bytes(bytes.clone())
                    .file_name(filename.to_string())
                    .mime_str(mime)?;
                let form = Form::new()
                    .part("file", part)
                    .text("_payload", payload.clone());
                Ok(self.req(Method::POST, collection).multipart(form))
            })
            .await?;
        Ok((doc, true))
    }

    pub async fn update_global(&self, slug: &str, data: &Value) -> Result<Value> {
        let req = self.req(Method::POST, &format!("globals/{slug}"));
        let body = self.send(req.json(data)).await?;
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

/// The body a create/update sends: `data` plus `_status` matching the draft flag (so the
/// main row is never left stale). Nulls go as given: they clear fields, so the caller
/// decides, per the schema, where Payload needs `[]` instead.
pub fn prepare(data: &Value, draft: bool) -> Value {
    let mut v = data.clone();
    if let Some(o) = v.as_object_mut() {
        let status = if draft { "draft" } else { "published" };
        o.insert("_status".into(), Value::String(status.into()));
    }
    v
}

/// Drops `null` object members recursively; for upload creates, where a null clears nothing.
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

fn with_key(data: &Value, key_field: &str, key: &Value) -> Value {
    let mut data = data.clone();
    if let Some(o) = data.as_object_mut() {
        o.insert(key_field.into(), key.clone());
    }
    data
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
