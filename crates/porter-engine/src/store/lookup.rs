use super::Store;
use crate::url::normalise_url;
use anyhow::Result;
use porter_model::{ItemId, Route, RouteTarget};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Route and document URL keys, built on first lookup and dropped when either kind is written.
#[derive(Default)]
pub(super) struct UrlIndex {
    routes: HashMap<String, RouteTarget>,
    docs: HashMap<String, ItemId>,
    /// Keyed by the path part of the key (from the first `/`), for documents known by path.
    doc_paths: HashMap<String, ItemId>,
    hosts: HashSet<String>,
}

fn split_key(key: &str) -> (&str, &str) {
    key.split_at(key.find('/').unwrap_or(key.len()))
}

impl Store {
    fn url_index(&self) -> Result<Arc<UrlIndex>> {
        let mut cache = self.url_index.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(idx) = cache.as_ref() {
            return Ok(idx.clone());
        }
        let mut idx = UrlIndex::default();
        {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT data FROM items WHERE kind = 'route' AND deleted_at IS NULL ORDER BY rowid",
            )?;
            for data in stmt.query_map([], |r| r.get::<_, String>(0))? {
                let Ok(route) = serde_json::from_str::<Route>(&data?) else {
                    continue;
                };
                if let Some(k) = normalise_url(&route.url, None) {
                    idx.hosts.insert(split_key(&k.0).0.to_string());
                    // The first route with a target wins over earlier ones without.
                    let t = idx.routes.entry(k.0).or_insert(RouteTarget::None);
                    if *t == RouteTarget::None {
                        *t = route.target;
                    }
                }
            }
            let mut stmt = conn.prepare(
                "SELECT id, url, CASE WHEN json_type(data, '$.path') = 'text'
                   THEN json_extract(data, '$.path') END
                 FROM items WHERE kind = 'document' AND deleted_at IS NULL
                 ORDER BY json_type(data, '$.path') IS NOT 'text', rowid",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?;
            for row in rows {
                let (id, url, path) = row?;
                let id: ItemId = id.parse()?;
                if let Some(k) = url.and_then(|u| normalise_url(&u, None)) {
                    idx.hosts.insert(split_key(&k.0).0.to_string());
                    idx.docs.entry(k.0).or_insert(id);
                }
                if let Some(k) = path
                    .filter(|p| p.starts_with('/'))
                    .and_then(|p| normalise_url(&format!("http://h{p}"), None))
                {
                    idx.doc_paths
                        .entry(split_key(&k.0).1.to_string())
                        .or_insert(id);
                }
            }
        }
        let idx = Arc::new(idx);
        *cache = Some(idx.clone());
        Ok(idx)
    }

    /// Called by writes that can change what `lookup_url` returns.
    pub(super) fn drop_url_index(&self) {
        *self.url_index.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// What a URL points at: a route's target, else a document stored under that URL or
    /// path (rows with a `path`, i.e. normalised documents, before raw rows), else a media
    /// item with that URL.
    pub fn lookup_url(&self, url: &str) -> Result<Option<RouteTarget>> {
        let Some(key) = normalise_url(url, None) else {
            return Ok(None);
        };
        let idx = self.url_index()?;
        if let Some(t) = idx.routes.get(&key.0).filter(|t| **t != RouteTarget::None) {
            return Ok(Some(t.clone()));
        }
        if let Some(id) = idx.docs.get(&key.0) {
            return Ok(Some(RouteTarget::Document(*id)));
        }
        // Paths only match on this site's hosts (any host when no URLs are known).
        let (host, path) = split_key(&key.0);
        if (idx.hosts.is_empty() || idx.hosts.contains(host))
            && let Some(id) = idx.doc_paths.get(path)
        {
            return Ok(Some(RouteTarget::Document(*id)));
        }
        Ok(self.media_by_url(url)?.map(RouteTarget::Media))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_model::{ItemId, ItemKind, Route, SourceRef};

    #[test]
    fn lookup_url_routes_docs_media() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("p.db"), dir.path().join("m")).unwrap();
        let src = |id: &str, url: Option<&str>| SourceRef {
            connector: "t".into(),
            source_id: id.into(),
            url: url.map(Into::into),
        };
        let target = ItemId::new(ItemKind::Document, &src("x", None));
        let route = |id: &str, url: &str, target: RouteTarget| {
            let s = src(id, Some(url));
            let r = Route {
                url: url.into(),
                target,
                http_status: Some(200),
                title: None,
            };
            store
                .put_item(ItemKind::Route, ItemId::new(ItemKind::Route, &s), &s, &r)
                .unwrap();
        };
        route(
            "r1",
            "https://example.test/a/",
            RouteTarget::Document(target),
        );
        route("r2", "https://example.test/b/", RouteTarget::None);
        let doc = |id: &str, url: Option<&str>, path: &str| {
            let s = src(id, url);
            let d = serde_json::json!({ "path": path });
            store
                .put_item(
                    ItemKind::Document,
                    ItemId::new(ItemKind::Document, &s),
                    &s,
                    &d,
                )
                .unwrap();
            ItemId::new(ItemKind::Document, &s)
        };
        let b = doc("b", Some("https://example.test/b/"), "/b/");
        let c = doc("c", None, "/C-Page/");
        let m = store
            .put_media_bytes(b"x", "https://example.test/f.pdf", "f.pdf")
            .unwrap()
            .id;

        assert_eq!(
            store.lookup_url("http://www.example.test/A").unwrap(),
            Some(RouteTarget::Document(target))
        );
        // A route with no target falls through to the document.
        assert_eq!(
            store.lookup_url("https://example.test/b").unwrap(),
            Some(RouteTarget::Document(b))
        );
        assert_eq!(
            store.lookup_url("https://example.test/c-page/").unwrap(),
            Some(RouteTarget::Document(c))
        );
        assert_eq!(
            store.lookup_url("https://other.test/c-page/").unwrap(),
            None
        );
        assert_eq!(
            store.lookup_url("https://example.test/f.pdf").unwrap(),
            Some(RouteTarget::Media(m))
        );
        assert_eq!(
            store.lookup_url("https://example.test/nope/").unwrap(),
            None
        );
        // Writes after the first lookup are seen.
        let d = doc("d", Some("https://example.test/d/"), "/d/");
        assert_eq!(
            store.lookup_url("https://example.test/d/").unwrap(),
            Some(RouteTarget::Document(d))
        );
    }
}
