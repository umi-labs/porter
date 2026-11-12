//! Attachments → `Media` items. The file is whichever a connector already stored under one of
//! the attachment's URLs, else the copy in `wp_root/wp-content/uploads` (local, no network).
//! Every size WordPress generated, the `-scaled` copy and the original are registered as
//! variants on every known host, so any of those URLs resolves to the one item.

use super::normalise::{Post, Site, canon};
use crate::stage::StageCtx;
use crate::url::normalise_url;
use anyhow::Result;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, Media, Severity, SourceRef};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path};

/// Splits `dir/stem.ext` into `(stem, ".ext")`.
fn stem_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// URLs WordPress may serve the same upload under, for a URL that isn't stored: without a
/// `-WxH` size suffix (any size, recorded or not), then also without `-scaled`; each also as
/// the `-scaled` copy.
pub(crate) fn alternates(url: &str) -> Vec<String> {
    static SIZE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"-\d+x\d+$").expect("valid regex"));
    let bare = url.split(['?', '#']).next().unwrap_or(url);
    let Some((dir, name)) = bare.rsplit_once('/') else {
        return vec![];
    };
    let (stem, ext) = stem_ext(name);
    if ext.is_empty() {
        return vec![];
    }
    let unsized_ = SIZE.replace(stem, "").to_string();
    let unscaled = unsized_
        .strip_suffix("-scaled")
        .unwrap_or(&unsized_)
        .to_string();
    let mut out = vec![];
    for s in [&unsized_, &unscaled] {
        let scaled = (!s.ends_with("-scaled")).then(|| format!("{dir}/{s}-scaled{ext}"));
        for cand in std::iter::once(format!("{dir}/{s}{ext}")).chain(scaled) {
            if cand != bare && !out.contains(&cand) {
                out.push(cand);
            }
        }
    }
    out
}

/// File names of every variant WordPress may serve for this attachment.
pub(crate) fn variant_names(file: &str, metadata: &Value) -> Vec<String> {
    let base = file.rsplit('/').next().unwrap_or(file);
    let mut names = vec![base.to_string()];
    if let Some(o) = metadata["original_image"].as_str() {
        names.push(o.to_string());
    }
    if let Value::Object(sizes) = &metadata["sizes"] {
        names.extend(
            sizes
                .values()
                .filter_map(|s| s["file"].as_str().map(str::to_string)),
        );
    }
    // The big-image copy and the original it was scaled from.
    let (stem, ext) = stem_ext(base);
    match stem.strip_suffix("-scaled") {
        Some(orig) => names.push(format!("{orig}{ext}")),
        None => names.push(format!("{stem}-scaled{ext}")),
    }
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| !n.is_empty() && !n.contains('/') && seen.insert(n.clone()));
    names
}

/// `<uploads path>` (e.g. `/wp-content/uploads`) from the attachment URL and its file.
fn uploads_path(url: Option<&str>, file: &str) -> String {
    url.and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| {
            u.path()
                .strip_suffix(file)
                .map(|p| p.trim_end_matches('/').to_string())
        })
        .unwrap_or_else(|| "/wp-content/uploads".to_string())
}

/// A relative uploads path with no `..`, root or prefix components.
fn safe_rel(file: &str) -> bool {
    !file.is_empty()
        && Path::new(file)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Every stored file's variant URLs, by URL key, read once: a miss in the store's own lookup
/// scans the table, which thousands of attachments can't afford.
struct Known {
    exact: HashMap<String, ItemId>,
    /// Lower-cased key → item, or `None` when two items share it (ambiguous).
    folded: HashMap<String, Option<ItemId>>,
}

impl Known {
    fn load(store: &crate::store::Store) -> Result<Known> {
        let mut k = Known {
            exact: HashMap::new(),
            folded: HashMap::new(),
        };
        for (id, src, m) in store.items_decodable::<Media>(ItemKind::Media)? {
            if src.connector == "media" {
                for v in &m.variants {
                    k.add(v, id);
                }
            }
        }
        Ok(k)
    }

    fn add(&mut self, url: &str, id: ItemId) {
        let Some(key) = normalise_url(url, None) else {
            return;
        };
        let e = self.folded.entry(key.0.to_lowercase()).or_insert(Some(id));
        if *e != Some(id) {
            *e = None;
        }
        self.exact.insert(key.0, id);
    }

    /// Like `Store::media_by_url`: exact key, else an unambiguous case-insensitive match.
    fn get(&self, url: &str) -> Option<ItemId> {
        let key = normalise_url(url, None)?.0;
        self.exact
            .get(&key)
            .copied()
            .or_else(|| self.folded.get(&key.to_lowercase()).copied().flatten())
    }
}

/// Media for every attachment, keyed by attachment post id. `prov` gains attachment rows →
/// media item; attachments with no file get a missing-media warning and `Skipped`.
pub(crate) fn attachments(
    ctx: &StageCtx,
    posts: &BTreeMap<u64, Post>,
    site: &Site,
    prov: &mut BTreeMap<ItemId, ItemId>,
    done: &mut std::collections::HashSet<ItemId>,
) -> Result<HashMap<u64, ItemId>> {
    let store = ctx.project.store();
    let mut out = HashMap::new();
    let mut known = Known::load(store)?;
    for p in posts.values().filter(|p| p.post_type == "attachment") {
        if ctx.cancel.is_cancelled() {
            break;
        }
        let file = p.file.clone().unwrap_or_default();
        let metadata = p.file_meta.clone().unwrap_or(Value::Null);
        let up = uploads_path(p.link.as_deref(), &file);
        let dir = file.rsplit_once('/').map(|(d, _)| d.to_string());
        let names = if file.is_empty() {
            vec![]
        } else {
            variant_names(&file, &metadata)
        };
        let urls: Vec<String> = site
            .hosts
            .iter()
            .flat_map(|h| {
                let (up, dir) = (&up, &dir);
                names.iter().map(move |n| match dir {
                    Some(d) => format!("https://{h}{up}/{d}/{n}"),
                    None => format!("https://{h}{up}/{n}"),
                })
            })
            .collect();
        let mut found = p.link.iter().chain(&urls).find_map(|u| known.get(u));
        if found.is_none()
            && safe_rel(&file)
            && let Some(root) = &site.wp_root
        {
            let path = root.join("wp-content/uploads").join(&file);
            if let Ok(bytes) = std::fs::read(&path) {
                let url = p.link.clone().or_else(|| urls.first().cloned());
                let name = file.rsplit('/').next().unwrap_or(&file);
                found = Some(
                    store
                        .put_media_bytes(&bytes, url.as_deref().unwrap_or(&file), name)?
                        .id,
                );
            }
        }
        let (canon_id, _) = canon(ItemKind::Document, &format!("post:{}", p.id), None);
        let Some(id) = found else {
            ctx.issue(
                IssueKind::MissingMedia,
                Severity::Warning,
                Some(canon_id),
                format!(
                    "attachment {} ({}) has no file: not downloaded and not in wp_root",
                    p.id,
                    p.link.as_deref().unwrap_or(&file)
                ),
                json!({ "attachment": p.id, "file": file, "url": p.link }),
            )?;
            ctx.item_status(
                canon_id,
                ItemKind::Document,
                ItemStatus::Skipped {
                    reason: "no file".into(),
                },
            )?;
            done.insert(canon_id);
            for r in &p.raw {
                prov.insert(*r, canon_id);
            }
            continue;
        };
        let Some(mut m) = store.get_item::<Media>(id)? else {
            continue;
        };
        let nz = |s: &Option<String>| s.clone().filter(|x| !x.trim().is_empty());
        m.alt = nz(&p.alt).or(m.alt);
        m.caption = nz(&p.caption).or(m.caption);
        m.title = Some(p.title.clone()).filter(|t| !t.is_empty()).or(m.title);
        m.folder = dir.clone().or(m.folder);
        if let Some(n) = file.rsplit('/').next().filter(|n| !n.is_empty()) {
            m.filename = n.to_string();
        }
        if let Some(mime) = nz(&p.mime) {
            m.mime = mime;
        }
        let dim = |k: &str| metadata[k].as_u64().and_then(|n| u32::try_from(n).ok());
        m.width = dim("width").or(m.width);
        m.height = dim("height").or(m.height);
        let src = SourceRef {
            connector: "media".into(),
            source_id: m.hash.clone(),
            url: m.variants.first().cloned(),
        };
        store.put_item(ItemKind::Media, id, &src, &m)?;
        for u in p.link.iter().chain(&urls) {
            known.add(u, id);
            if let Some(prev) = store.add_media_variant(id, u)? {
                ctx.issue(
                    IssueKind::MissingMedia,
                    Severity::Info,
                    Some(id),
                    format!("{u} pointed at another file; now attachment {}", p.id),
                    json!({ "url": u, "previous": prev }),
                )?;
            }
        }
        if done.insert(id) {
            ctx.item_status(id, ItemKind::Media, ItemStatus::Done)?;
        }
        for r in &p.raw {
            prov.insert(*r, id);
        }
        out.insert(p.id, id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        let meta =
            json!({"original_image": "a.jpg", "sizes": {"m": {"file": "a-scaled-300x200.jpg"}}});
        assert_eq!(
            variant_names("2024/01/a-scaled.jpg", &meta),
            ["a-scaled.jpg", "a.jpg", "a-scaled-300x200.jpg"]
        );
        assert!(variant_names("b.png", &Value::Null).contains(&"b-scaled.png".to_string()));
        assert_eq!(
            uploads_path(Some("https://x.test/files/2024/01/a.jpg"), "2024/01/a.jpg"),
            "/files"
        );
        assert_eq!(
            alternates("https://x.test/u/a-scaled-640x480.jpg?v=1"),
            ["https://x.test/u/a-scaled.jpg", "https://x.test/u/a.jpg",]
        );
        assert!(alternates("https://x.test/page/").is_empty());
        assert!(!safe_rel("../etc/passwd"));
        assert!(!safe_rel("/etc/passwd"));
        assert!(safe_rel("2024/01/a.jpg"));
    }
}
