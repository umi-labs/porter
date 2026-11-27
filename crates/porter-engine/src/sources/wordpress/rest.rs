//! WordPress REST connector: structured content, menus, users, settings, Yoast heads and
//! Redirection rules. Says what it could not see (access-gap issues) rather than guessing.
use crate::config::ConnectorConfig;
use crate::connectors::RawItem;
use crate::connectors::crawl::site_of;
use crate::http;
use crate::stage::{Capabilities, Connector, Feature, StageCtx};
use anyhow::Result;
use async_trait::async_trait;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, Severity, SourceRef};
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, RequestBuilder, redirect};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

const NAME: &str = "wp-rest";
const USER_AGENT: &str = "porter/2.0 (+https://github.com/umi-labs/porter)";
const PER_PAGE: usize = 100;
/// Types whose ids are strings (`theme//slug`) or that other routes already cover.
const SKIP_ROUTES: [&str; 5] = [
    "media",
    "menu-items",
    "templates",
    "template-parts",
    "global-styles",
];

/// Credentials; never logged, never written to events, issues or raw files.
#[derive(Deserialize, Default, Clone)]
struct Creds {
    auth_type: Option<String>,
    username: Option<String>,
    password: Option<String>,
    bearer_token: Option<String>,
    api_key: Option<String>,
    api_key_header: Option<String>,
}

enum Auth {
    None,
    Basic(String, String),
    Bearer(String),
    ApiKey(String, String),
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

impl Creds {
    fn from_env(get: impl Fn(&str) -> Option<String>) -> Creds {
        Creds {
            auth_type: nonempty(get("PORTER_WP_AUTH_TYPE")),
            username: nonempty(get("PORTER_WP_USERNAME")),
            password: nonempty(get("PORTER_WP_PASSWORD")),
            bearer_token: nonempty(get("PORTER_WP_BEARER_TOKEN")),
            api_key: nonempty(get("PORTER_WP_API_KEY")),
            api_key_header: nonempty(get("PORTER_WP_API_KEY_HEADER")),
        }
    }

    /// Fields set in `top` win; the rest come from `self`.
    fn overlaid_by(self, top: Creds) -> Creds {
        Creds {
            auth_type: top.auth_type.or(self.auth_type),
            username: top.username.or(self.username),
            password: top.password.or(self.password),
            bearer_token: top.bearer_token.or(self.bearer_token),
            api_key: top.api_key.or(self.api_key),
            api_key_header: top.api_key_header.or(self.api_key_header),
        }
    }

    /// `configured` is the `auth` key from porter.toml; with no type named anywhere the
    /// type is inferred from which secrets are present.
    fn into_auth(self, configured: Option<&str>) -> Auth {
        let kind = self
            .auth_type
            .as_deref()
            .or(configured)
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| {
                if self.bearer_token.is_some() {
                    "bearer"
                } else if self.api_key.is_some() {
                    "apikey"
                } else {
                    "basic"
                }
                .into()
            });
        match (kind.as_str(), self) {
            ("bearer", c) => c.bearer_token.map(Auth::Bearer).unwrap_or(Auth::None),
            ("apikey", c) => match c.api_key {
                Some(k) => Auth::ApiKey(c.api_key_header.unwrap_or_else(|| "X-API-Key".into()), k),
                None => Auth::None,
            },
            ("basic", c) => match (c.username, c.password) {
                (Some(u), Some(p)) => Auth::Basic(u, p),
                _ => Auth::None,
            },
            _ => Auth::None,
        }
    }
}

impl Auth {
    fn is_some(&self) -> bool {
        !matches!(self, Auth::None)
    }
    fn apply(&self, r: RequestBuilder) -> RequestBuilder {
        match self {
            Auth::None => r,
            Auth::Basic(u, p) => r.basic_auth(u, Some(p)),
            Auth::Bearer(t) => r.bearer_auth(t),
            Auth::ApiKey(h, k) => {
                match (HeaderName::try_from(h.as_str()), HeaderValue::from_str(k)) {
                    (Ok(n), Ok(mut v)) => {
                        v.set_sensitive(true);
                        r.header(n, v)
                    }
                    // Unusable header: send nothing; the 401s become access-gaps.
                    _ => r,
                }
            }
        }
    }
}

enum Reply {
    Ok {
        body: Value,
        total: Option<u64>,
        pages: Option<u64>,
    },
    /// 401 or 403.
    Denied,
    /// 404.
    Missing,
    Status(u16),
    Fail(String),
    /// Redirected off the site; not followed, so credentials never leave it.
    CrossHost,
    Cancelled,
}

/// A path component made from remote data: `[A-Za-z0-9_-]` only, `/` becomes `_`,
/// anything else (or empty) becomes `x-` plus 12 hex of the original's sha256.
fn safe_component(raw: &str) -> String {
    let mapped = raw.replace('/', "_");
    let ok = !mapped.is_empty()
        && mapped
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        return mapped;
    }
    let hex: String = Sha256::digest(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("x-{}", &hex[..12])
}

const MAX_REDIRECTS: usize = 5;
const MAX_PAGES: u64 = 10_000;

struct Api<'a> {
    ctx: &'a StageCtx,
    client: Client,
    base: String,
    auth: Auth,
    cancel: CancellationToken,
}

pub struct WpRestConnector {
    url: String,
    auth: Option<String>,
    keyring: Option<String>,
}

impl WpRestConnector {
    pub fn new(cfg: &ConnectorConfig) -> Self {
        let (url, auth, keyring) = match cfg {
            ConnectorConfig::WpRest { url, auth, keyring } => {
                (url.clone(), auth.clone(), keyring.clone())
            }
            _ => Default::default(),
        };
        Self {
            url: site_root(&url),
            auth,
            keyring,
        }
    }

    /// Env vars override the keyring entry, field by field. A missing keyring is an info
    /// access-gap and the run carries on with env only. A login WordPress doesn't accept is
    /// dropped (see `check_login`), so it never costs the public content.
    async fn api<'a>(&self, ctx: &'a StageCtx) -> Result<Api<'a>> {
        let mut creds = Creds::default();
        if let Some(name) = &self.keyring {
            let found = keyring::Entry::new("porter", name)
                .and_then(|e| e.get_password())
                .ok()
                .and_then(|s| serde_json::from_str::<Creds>(&s).ok());
            match found {
                Some(c) => creds = c,
                None => ctx.issue(
                    IssueKind::AccessGap,
                    Severity::Info,
                    None,
                    format!("keyring entry '{name}' is unavailable or unreadable; using PORTER_WP_* environment variables only"),
                    json!({ "connector": NAME, "keyring": name }),
                )?,
            }
        }
        let creds = creds.overlaid_by(Creds::from_env(|k| std::env::var(k).ok()));
        let mut api = Api {
            ctx,
            // Redirects are followed by hand, same-site only (see `Api::get`).
            client: Client::builder()
                .user_agent(USER_AGENT)
                .redirect(redirect::Policy::none())
                .build()?,
            base: self.url.clone(),
            auth: creds.into_auth(self.auth.as_deref()),
            cancel: ctx.cancel.clone(),
        };
        api.check_login().await?;
        Ok(api)
    }
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(String::from)
}

fn title_of(v: &Value) -> Option<String> {
    let t = v.get("title").or_else(|| v.get("name"))?;
    match t {
        Value::String(s) => Some(s.clone()),
        _ => s(t, "rendered").or_else(|| s(t, "raw")),
    }
}

fn header_u64(r: &reqwest::Response, name: &str) -> Option<u64> {
    r.headers().get(name)?.to_str().ok()?.parse().ok()
}

impl Api<'_> {
    async fn get(&self, path: &str, query: &[(&str, String)]) -> Reply {
        if self.cancel.is_cancelled() {
            return Reply::Cancelled;
        }
        let mut url = format!("{}/wp-json{path}", self.base);
        let mut query = Some(query);
        let mut hops = 0;
        let r = loop {
            let mut req = self.client.get(&url);
            if let Some(q) = query.take() {
                req = req.query(q);
            }
            let sent = tokio::select! {
                _ = self.cancel.cancelled() => return Reply::Cancelled,
                r = http::send_with_retry(self.auth.apply(req), 3) => r,
            };
            let r = match sent {
                Ok(r) => r,
                Err(e) => return Reply::Fail(format!("{e:#}")),
            };
            if !r.status().is_redirection() {
                break r;
            }
            let here = r.url().clone();
            let target = r
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|l| here.join(l).ok());
            let Some(t) = target else {
                return Reply::Status(r.status().as_u16());
            };
            hops += 1;
            let same = site_of(&t) == site_of(&here) && t.scheme() == here.scheme();
            if !same {
                return Reply::CrossHost;
            }
            if hops > MAX_REDIRECTS {
                return Reply::Fail("too many redirects".into());
            }
            url = t.to_string();
        };
        match r.status().as_u16() {
            401 | 403 => return Reply::Denied,
            404 => return Reply::Missing,
            c if !r.status().is_success() => return Reply::Status(c),
            _ => {}
        }
        let (total, pages) = (
            header_u64(&r, "x-wp-total"),
            header_u64(&r, "x-wp-totalpages"),
        );
        match r.json::<Value>().await {
            Ok(body) => Reply::Ok { body, total, pages },
            Err(e) => Reply::Fail(format!("invalid JSON: {e}")),
        }
    }

    fn gap(&self, endpoint: &str, why: &str) -> Result<()> {
        self.ctx.issue(
            IssueKind::AccessGap,
            Severity::Warning,
            None,
            format!("{endpoint}: {why}"),
            json!({ "connector": NAME, "endpoint": endpoint }),
        )
    }

    /// Turns a non-OK reply into the right issue (or silence for 404); `true` means
    /// the caller should stop.
    fn problem(&self, endpoint: &str, r: &Reply) -> Result<bool> {
        match r {
            Reply::Denied => self.gap(endpoint, "access denied (HTTP 401/403); not extracted")?,
            Reply::Status(c) => self.gap(endpoint, &format!("HTTP {c}; not extracted"))?,
            Reply::Fail(e) => self.gap(endpoint, &format!("request failed: {e}; not extracted"))?,
            Reply::CrossHost => self.gap(
                endpoint,
                "redirected to another host; not followed, not extracted",
            )?,
            Reply::Missing | Reply::Cancelled | Reply::Ok { .. } => {}
        }
        Ok(!matches!(r, Reply::Ok { .. }))
    }

    /// Walks `X-WP-TotalPages` (and stops on an empty or short page), handing each page to `each`.
    /// Bounded: 10 000 pages, and a page that repeats the previous page's first id means the
    /// server ignores `page`. A count that disagrees with `X-WP-Total` is reported.
    async fn pages(
        &self,
        path: &str,
        query: &[(&str, String)],
        mut each: impl FnMut(Vec<Value>) -> Result<()>,
    ) -> Result<()> {
        let (mut count, mut expected, mut prev_first) = (0u64, None, None::<Value>);
        for page in 1u64..=MAX_PAGES {
            let mut q = query.to_vec();
            q.push(("per_page", PER_PAGE.to_string()));
            q.push(("page", page.to_string()));
            match self.get(path, &q).await {
                Reply::Ok { body, pages, total } => {
                    let Value::Array(items) = body else {
                        return self.gap(path, "unexpected response shape; not extracted");
                    };
                    expected = expected.or(total);
                    let first = items.first().map(|i| i["id"].clone());
                    if first.is_some() && first == prev_first {
                        return self.gap(
                            path,
                            &format!(
                                "server ignores the page parameter; stopped after {count} records"
                            ),
                        );
                    }
                    prev_first = first;
                    let (n, last) = (items.len(), pages.is_some_and(|p| page >= p));
                    // ponytail: without the header a short page is taken as the last.
                    let short = pages.is_none() && n < PER_PAGE;
                    count += n as u64;
                    each(items)?;
                    if n == 0 || last || short {
                        break;
                    }
                }
                // Past the end: WordPress answers 400.
                Reply::Status(400) if page > 1 => break,
                other => {
                    self.problem(path, &other)?;
                    return Ok(());
                }
            }
            if page == MAX_PAGES {
                return self.gap(
                    path,
                    &format!("stopped at {MAX_PAGES} pages ({count} records)"),
                );
            }
        }
        match expected {
            Some(t) if t != count => self.gap(
                path,
                &format!("fetched {count} records but X-WP-Total says {t}"),
            ),
            _ => Ok(()),
        }
    }

    /// With a login, asks who we are. 401/403 means WordPress treats us as anonymous (wrong or
    /// revoked application password, application passwords switched off, e.g. by Wordfence, or
    /// the server stripping the Authorization header). Edit-context requests would then all
    /// fail, so carry on as anonymous and say why. Anything else is not evidence; keep the login.
    async fn check_login(&mut self) -> Result<()> {
        if !self.auth.is_some() {
            return Ok(());
        }
        if let Reply::Denied = self
            .get("/wp/v2/users/me", &[("context", "edit".into())])
            .await
        {
            self.gap(
                "/wp/v2/users/me",
                "WordPress did not accept the login (HTTP 401/403), so porter read public content only. \
                 Check the username and application password, that application passwords are allowed \
                 (security plugins such as Wordfence can turn them off) and that the server passes the \
                 Authorization header to WordPress",
            )?;
            self.auth = Auth::None;
        }
        Ok(())
    }

    fn unauthenticated_gaps(&self) -> Result<()> {
        if self.auth.is_some() {
            return Ok(());
        }
        for (endpoint, why) in [
            (
                "/wp/v2/posts",
                "drafts and private posts not visible without credentials",
            ),
            ("/wp/v2/menus", "menus need credentials"),
            ("/wp/v2/menu-items", "menu items need credentials"),
            ("/wp/v2/settings", "site settings need credentials"),
            (
                "/wp/v2/users",
                "only authors of published content are listed, without emails or roles, until credentials are given",
            ),
        ] {
            self.gap(endpoint, why)?;
        }
        Ok(())
    }

    /// Query for a content route: full editing view, every status, when authenticated.
    fn content_query(&self) -> Vec<(&'static str, String)> {
        if self.auth.is_some() {
            vec![("context", "edit".into()), ("status", "any".into())]
        } else {
            vec![("context", "view".into())]
        }
    }

    fn edit_query(&self) -> Vec<(&'static str, String)> {
        let ctx = if self.auth.is_some() { "edit" } else { "view" };
        vec![("context", ctx.into())]
    }

    /// `(slug, rest_base)` per entry of a `/types` or `/taxonomies` object. When the listing
    /// cannot be read the fallback is used and the access-gap says so.
    async fn listing(
        &self,
        path: &str,
        what: &str,
        fallback: &[(&str, &str)],
    ) -> Result<Vec<(String, String)>> {
        let r = self.get(path, &self.edit_query()).await;
        if let Reply::Ok {
            body: Value::Object(m),
            ..
        } = r
        {
            return Ok(m
                .into_iter()
                .map(|(k, v)| {
                    let slug = s(&v, "slug").unwrap_or(k);
                    let base = s(&v, "rest_base").unwrap_or_else(|| slug.clone());
                    (slug, base)
                })
                .collect());
        }
        let status = match &r {
            Reply::Denied => "HTTP 401/403".to_string(),
            Reply::Missing => "HTTP 404".to_string(),
            Reply::Status(c) => format!("HTTP {c}"),
            Reply::Fail(e) => e.clone(),
            Reply::CrossHost => "redirected to another host".into(),
            Reply::Cancelled => return Ok(vec![]),
            Reply::Ok { .. } => "unexpected response shape".into(),
        };
        let names: Vec<&str> = fallback.iter().map(|f| f.0).collect();
        self.gap(
            path,
            &format!(
                "{what} unknown ({status}); only {} fetched",
                names.join("/")
            ),
        )?;
        Ok(fallback
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect())
    }

    async fn namespaces(&self) -> Result<(Vec<String>, usize)> {
        let r = self.get("", &[]).await;
        if let Reply::Ok { body, .. } = &r {
            let ns = body["namespaces"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let routes = body["routes"].as_object().map_or(0, |m| m.len());
            return Ok((ns, routes));
        }
        self.problem("/wp-json", &r)?;
        Ok((vec![], 0))
    }
}

/// Everything `record` needs besides the JSON.
struct Rec<'a> {
    kind: ItemKind,
    /// Folder under `raw/wp-rest/`.
    dir: String,
    /// `<route>:<id>` or similar.
    source_id: String,
    label: &'a str,
    file: String,
}

/// `root/dir` where every component of `dir` must already be a `safe_component`; anything
/// else is a bug and refuses to write.
fn checked_dir(root: &std::path::Path, dir: &str) -> Result<std::path::PathBuf> {
    let mut out = root.to_path_buf();
    for c in dir.split('/') {
        anyhow::ensure!(safe_component(c) == c, "unsafe raw path component {c:?}");
        out.push(c);
    }
    anyhow::ensure!(out.starts_with(root), "raw path escapes the raw folder");
    Ok(out)
}

/// Gives an item with no usable id a row and a Failed status, so it is never dropped.
fn fail_unnumbered(ctx: &StageCtx, kind: ItemKind, label: &str, source_id: String) -> Result<()> {
    let src = SourceRef {
        connector: NAME.into(),
        source_id,
        url: None,
    };
    let id = ItemId::new(kind, &src);
    let row = RawItem {
        label: label.into(),
        title: None,
        status: None,
        url: None,
        raw_path: None,
    };
    ctx.project.store().put_item(kind, id, &src, &row)?;
    let error = "record has no numeric id".to_string();
    ctx.item_status(id, kind, ItemStatus::Failed { error })
}

fn record(ctx: &StageCtx, r: Rec<'_>, v: &Value) -> Result<()> {
    let src = SourceRef {
        connector: NAME.into(),
        source_id: r.source_id,
        url: s(v, "link").or_else(|| s(v, "url")),
    };
    let id = ItemId::new(r.kind, &src);
    let rel = format!("{}/{}.json", r.dir, r.file);
    let root = ctx.project.raw_dir(NAME);
    let dir = checked_dir(&root, &r.dir)?;
    let written = std::fs::create_dir_all(&dir).and_then(|_| {
        std::fs::write(
            dir.join(format!("{}.json", safe_component(&r.file))),
            serde_json::to_vec_pretty(v)?.as_slice(),
        )
    });
    let row = RawItem {
        label: r.label.to_string(),
        title: title_of(v),
        status: s(v, "status"),
        url: src.url.clone(),
        raw_path: written.is_ok().then_some(rel),
    };
    ctx.project.store().put_item(r.kind, id, &src, &row)?;
    let status = match written {
        Ok(()) => ItemStatus::Done,
        Err(e) => ItemStatus::Failed {
            error: format!("writing raw file: {e}"),
        },
    };
    ctx.item_status(id, r.kind, status)
}

/// Writes each record of a page; ids that are not numbers fail the item, never drop it.
fn record_all(
    ctx: &StageCtx,
    kind: ItemKind,
    label: &str,
    id_prefix: &str,
    dir: &str,
    items: &[Value],
    seen: &mut Vec<(String, Value)>,
) -> Result<()> {
    for v in items {
        let n = seen.len() + 1;
        let Some(id) = v.get("id").and_then(Value::as_u64) else {
            fail_unnumbered(ctx, kind, label, format!("{id_prefix}unknown-{n}"))?;
            seen.push((String::new(), Value::Null));
            continue;
        };
        let rec = Rec {
            kind,
            dir: dir.into(),
            source_id: format!("{id_prefix}{id}"),
            label,
            file: id.to_string(),
        };
        record(ctx, rec, v)?;
        seen.push((id.to_string(), v.clone()));
    }
    Ok(())
}

#[async_trait]
impl Connector for WpRestConnector {
    fn name(&self) -> &str {
        NAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            reads: [
                Feature::Drafts,
                Feature::Media,
                Feature::Menus,
                Feature::Redirects,
                Feature::Taxonomies,
                Feature::Seo,
            ]
            .into_iter()
            .collect(),
            writes: Default::default(),
        }
    }

    async fn analyse(&self, ctx: &StageCtx) -> Result<()> {
        let api = self.api(ctx).await?;
        api.unauthenticated_gaps()?;
        let (namespaces, routes) = api.namespaces().await?;
        let types = api
            .listing(
                "/wp/v2/types",
                "custom post types",
                &[("post", "posts"), ("page", "pages")],
            )
            .await?;
        let taxes = api
            .listing(
                "/wp/v2/taxonomies",
                "custom taxonomies",
                &[("category", "categories")],
            )
            .await?;
        let mut out = json!({
            "authenticated": api.auth.is_some(),
            "namespaces": namespaces,
            "routes": routes,
            "types": {}, "taxonomies": {},
        });
        let mut entries: Vec<(&str, String, String)> = types
            .into_iter()
            .map(|(slug, base)| ("types", slug, base))
            .chain(
                taxes
                    .into_iter()
                    .map(|(slug, base)| ("taxonomies", slug, base)),
            )
            .collect();
        if !entries.iter().any(|e| e.2 == "media") {
            entries.push(("types", "attachment".into(), "media".into()));
        }
        for (group, slug, base) in entries {
            if ctx.cancel.is_cancelled() {
                return Ok(());
            }
            if group == "taxonomies" && slug == "nav_menu"
                || SKIP_ROUTES[1..].contains(&base.as_str())
            {
                continue;
            }
            let mut q = if group == "types" && base != "media" {
                api.content_query()
            } else {
                api.edit_query()
            };
            q.push(("per_page", "1".into()));
            let reply = api.get(&format!("/wp/v2/{base}"), &q).await;
            match reply {
                Reply::Ok { total, .. } => {
                    out[group][&slug] = json!({ "rest_base": base, "total": total });
                }
                other => {
                    api.problem(&format!("/wp/v2/{base}"), &other)?;
                }
            }
        }
        let dir = ctx.project.raw_dir(NAME);
        std::fs::create_dir_all(&dir)?;
        Ok(std::fs::write(
            dir.join("analysis.json"),
            serde_json::to_vec_pretty(&out)?,
        )?)
    }

    async fn extract(&self, ctx: &StageCtx) -> Result<()> {
        let api = self.api(ctx).await?;
        api.unauthenticated_gaps()?;
        let authed = api.auth.is_some();
        let (namespaces, _) = api.namespaces().await?;
        let has = |n: &str| namespaces.iter().any(|x| x == n);
        let cancelled = || ctx.cancel.is_cancelled();

        // Content types.
        let types = api
            .listing(
                "/wp/v2/types",
                "custom post types",
                &[("post", "posts"), ("page", "pages")],
            )
            .await?;
        let mut links: Vec<(String, String, String)> = vec![];
        for (slug, base) in types {
            if SKIP_ROUTES.contains(&base.as_str()) || slug == "nav_menu_item" {
                continue;
            }
            let mut seen = vec![];
            let dir = safe_component(&base);
            api.pages(&format!("/wp/v2/{base}"), &api.content_query(), |items| {
                record_all(
                    ctx,
                    ItemKind::Document,
                    &slug,
                    &format!("{base}:"),
                    &dir,
                    &items,
                    &mut seen,
                )
            })
            .await?;
            links.extend(seen.iter().filter_map(|(id, v)| {
                (s(v, "status").as_deref() == Some("publish"))
                    .then(|| s(v, "link").map(|l| (dir.clone(), id.clone(), l)))
                    .flatten()
            }));
            if cancelled() {
                return Ok(());
            }
        }

        // Media records only; files come from the crawl and WXR connectors.
        let mut seen = vec![];
        api.pages("/wp/v2/media", &api.edit_query(), |items| {
            record_all(
                ctx,
                ItemKind::Document,
                "attachment",
                "media:",
                "media",
                &items,
                &mut seen,
            )
        })
        .await?;

        // Menus (WordPress 5.9+, credentials needed).
        if authed && !cancelled() {
            let mut items = vec![];
            api.pages("/wp/v2/menu-items", &api.edit_query(), |p| {
                items.extend(p);
                Ok(())
            })
            .await?;
            let mut menus = vec![];
            api.pages("/wp/v2/menus", &api.edit_query(), |p| {
                menus.extend(p);
                Ok(())
            })
            .await?;
            let ids: Vec<u64> = menus.iter().filter_map(|m| m["id"].as_u64()).collect();
            let in_menu = |i: &Value, id: u64| match &i["menus"] {
                Value::Array(a) => a.iter().any(|x| x.as_u64() == Some(id)),
                x => x.as_u64() == Some(id),
            };
            for (n, m) in menus.iter().enumerate() {
                let Some(id) = m.get("id").and_then(Value::as_u64) else {
                    fail_unnumbered(
                        ctx,
                        ItemKind::Menu,
                        "nav_menu",
                        format!("menus:unknown-{}", n + 1),
                    )?;
                    continue;
                };
                let mine: Vec<&Value> = items.iter().filter(|i| in_menu(i, id)).collect();
                let rec = Rec {
                    kind: ItemKind::Menu,
                    dir: "menus".into(),
                    source_id: format!("menus:{id}"),
                    label: "nav_menu",
                    file: id.to_string(),
                };
                record(
                    ctx,
                    rec,
                    &json!({ "menu": m, "items": mine, "name": m["name"], "url": null }),
                )?;
            }
            let orphans: Vec<&Value> = items
                .iter()
                .filter(|i| !ids.iter().any(|id| in_menu(i, *id)))
                .collect();
            if !orphans.is_empty() {
                let dir = checked_dir(&ctx.project.raw_dir(NAME), "menus")?;
                std::fs::create_dir_all(&dir)?;
                std::fs::write(
                    dir.join("orphans.json"),
                    serde_json::to_vec_pretty(&orphans)?,
                )?;
                ctx.issue(
                    IssueKind::ParentMissing,
                    Severity::Warning,
                    None,
                    format!(
                        "{} menu items belong to no listed menu; saved in menus/orphans.json",
                        orphans.len()
                    ),
                    json!({ "count": orphans.len() }),
                )?;
            }
        }
        if cancelled() {
            return Ok(());
        }

        // Users.
        api.pages("/wp/v2/users", &api.edit_query(), |items| {
            record_all(
                ctx,
                ItemKind::Author,
                "user",
                "users:",
                "users",
                &items,
                &mut vec![],
            )
        })
        .await?;

        // Terms per taxonomy.
        let taxes = api
            .listing(
                "/wp/v2/taxonomies",
                "custom taxonomies",
                &[("category", "categories")],
            )
            .await?;
        for (slug, base) in taxes {
            if cancelled() {
                return Ok(());
            }
            if slug == "nav_menu" {
                continue;
            }
            api.pages(&format!("/wp/v2/{base}"), &api.edit_query(), |items| {
                record_all(
                    ctx,
                    ItemKind::Term,
                    &slug,
                    &format!("terms:{slug}:"),
                    &format!("terms/{}", safe_component(&slug)),
                    &items,
                    &mut vec![],
                )
            })
            .await?;
        }

        // Settings.
        if authed && !cancelled() {
            match api.get("/wp/v2/settings", &[]).await {
                Reply::Ok { body, .. } => {
                    let rec = Rec {
                        kind: ItemKind::Settings,
                        dir: "settings".into(),
                        source_id: "settings".into(),
                        label: "settings",
                        file: "settings".into(),
                    };
                    record(ctx, rec, &body)?;
                }
                other => {
                    api.problem("/wp/v2/settings", &other)?;
                }
            }
        }

        // Yoast heads: one request per published URL.
        if has("yoast/v1") {
            let dir = ctx.project.raw_dir(NAME).join("yoast");
            std::fs::create_dir_all(&dir)?;
            let mut misses = 0u64;
            for (route, id, link) in links {
                match api.get("/yoast/v1/get_head", &[("url", link)]).await {
                    Reply::Ok { body, .. } => std::fs::write(
                        dir.join(format!("{route}-{id}.json")),
                        serde_json::to_vec_pretty(&body)?,
                    )?,
                    Reply::Cancelled => break,
                    // Every request would be refused the same way.
                    r @ (Reply::Denied | Reply::CrossHost) => {
                        api.problem("/yoast/v1/get_head", &r)?;
                        break;
                    }
                    _ => misses += 1,
                }
            }
            if misses > 0 {
                ctx.issue(
                    IssueKind::AccessGap,
                    Severity::Info,
                    None,
                    format!("/yoast/v1/get_head: no SEO head for {misses} published URLs"),
                    json!({ "connector": NAME, "count": misses }),
                )?;
            }
        }

        // Redirection rules (0-based pages, `{items, total}` envelope).
        if has("redirection/v1") && !cancelled() {
            let (mut got, mut prev_first) = (0u64, None::<Value>);
            for page in 0..MAX_PAGES {
                let q = [
                    ("page", page.to_string()),
                    ("per_page", PER_PAGE.to_string()),
                ];
                let r = api.get("/redirection/v1/redirect", &q).await;
                let Reply::Ok { body, .. } = &r else {
                    api.problem("/redirection/v1/redirect", &r)?;
                    break;
                };
                let items = body["items"].as_array().cloned().unwrap_or_default();
                let total = body["total"].as_u64();
                let first = items.first().map(|i| i["id"].clone());
                if first.is_some() && first == prev_first {
                    api.gap(
                        "/redirection/v1/redirect",
                        "server ignores the page parameter; stopped early",
                    )?;
                    break;
                }
                prev_first = first;
                for (n, it) in items.iter().enumerate() {
                    let Some(id) = it.get("id").and_then(Value::as_u64) else {
                        let sid = format!("redirection:unknown-{}", got + n as u64 + 1);
                        fail_unnumbered(ctx, ItemKind::Redirect, "redirect", sid)?;
                        continue;
                    };
                    let mut v = it.clone();
                    if let Some(o) = v.as_object_mut() {
                        o.entry("title").or_insert_with(|| it["url"].clone());
                    }
                    let rec = Rec {
                        kind: ItemKind::Redirect,
                        dir: "redirection".into(),
                        source_id: format!("redirection:{id}"),
                        label: "redirect",
                        file: id.to_string(),
                    };
                    record(ctx, rec, &v)?;
                }
                got += items.len() as u64;
                // Without `total` the only end is an empty page.
                if items.is_empty() || total.is_some_and(|t| got >= t) {
                    break;
                }
                if page + 1 == MAX_PAGES {
                    api.gap(
                        "/redirection/v1/redirect",
                        &format!("stopped at {MAX_PAGES} pages"),
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// The site root from a site URL or any REST URL people paste
/// (`https://example.com/wp-json/wp/v2/pages/` -> `https://example.com`).
fn site_root(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    match url.find("/wp-json") {
        Some(i) => url[..i].to_string(),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn site_root_from_pasted_rest_urls() {
        for (given, want) in [
            ("https://example.com", "https://example.com"),
            ("https://example.com/", "https://example.com"),
            ("https://example.com/wp-json", "https://example.com"),
            (
                "https://example.com/wp-json/wp/v2/pages/",
                "https://example.com",
            ),
            (
                "https://example.com/blog/wp-json/wp/v2/posts?per_page=5",
                "https://example.com/blog",
            ),
        ] {
            assert_eq!(super::site_root(given), want, "{given}");
        }
    }

    use super::*;

    #[test]
    fn env_beats_keyring_field_by_field() {
        let kr = Creds {
            bearer_token: Some("from-keyring".into()),
            username: Some("kr-user".into()),
            ..Default::default()
        };
        let env = Creds::from_env(|k| (k == "PORTER_WP_BEARER_TOKEN").then(|| "from-env".into()));
        let merged = kr.overlaid_by(env);
        assert_eq!(merged.bearer_token.as_deref(), Some("from-env"));
        assert_eq!(merged.username.as_deref(), Some("kr-user"));
        assert!(matches!(merged.into_auth(None), Auth::Bearer(t) if t == "from-env"));
    }
}
