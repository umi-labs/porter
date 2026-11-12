//! Attachments → `Media` items. The file is whichever a connector already stored under one of
//! the attachment's URLs, else the copy in the uploads folder (`wp_root/wp-content/uploads`, or
//! `uploads_dir`; local, no network).
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

/// Whether `rel` exists under `root` with exactly this case. A case-insensitive disk (macOS)
/// opens `photo.jpg` when only `Photo.jpg` is there, which would be another file's bytes.
/// `Err(Some(name))` names the entry that differs only by case; listings are cached per folder.
fn exact_case(
    root: &Path,
    rel: &str,
    cache: &mut HashMap<std::path::PathBuf, Vec<String>>,
) -> std::result::Result<(), Option<String>> {
    let mut dir = root.to_path_buf();
    for part in rel.split('/') {
        let names = cache.entry(dir.clone()).or_insert_with(|| {
            std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default()
        });
        if !names.iter().any(|n| n == part) {
            let lower = part.to_lowercase();
            return Err(names.iter().find(|n| n.to_lowercase() == lower).cloned());
        }
        dir.push(part);
    }
    Ok(())
}

/// Every stored file's variant URLs, by URL key, read once from the store's URL table (the
/// authority on which file a URL is now; an item's own `variants` keeps URLs it no longer
/// owns): a miss in the store's own lookup scans the table, which thousands of attachments
/// can't afford.
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
        for (url, id) in store.media_variant_rows()? {
            k.add(&url, id);
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

    fn exact(&self, url: &str) -> Option<ItemId> {
        self.exact.get(&normalise_url(url, None)?.0).copied()
    }

    /// An unambiguous case-insensitive match, like `Store::media_by_url`'s fallback.
    fn folded(&self, url: &str) -> Option<ItemId> {
        let key = normalise_url(url, None)?.0;
        self.folded.get(&key.to_lowercase()).copied().flatten()
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
    let mut listings = HashMap::new();
    // Attachment -> why its uploads file wasn't used: the file found under another case,
    // or "" when it isn't a regular file inside uploads.
    let mut case_miss: HashMap<u64, String> = HashMap::new();
    // Media item -> the attachments that resolved to it, with their own alt, title and folder.
    type Own = (u64, Option<String>, String, Option<String>);
    let mut shared: BTreeMap<ItemId, Vec<Own>> = BTreeMap::new();
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
        let mut found = p.link.iter().chain(&urls).find_map(|u| known.exact(u));
        if found.is_none()
            && safe_rel(&file)
            && let Some(root) = &site.uploads
        {
            let path = root.join(&file);
            let exact = exact_case(root, &file, &mut listings);
            if let Err(Some(other)) = &exact {
                case_miss.insert(p.id, other.clone());
            }
            // Only a regular file inside the uploads folder is read: a symlink could
            // point anywhere on this machine.
            let regular = exact.is_err() || inside_regular(root, &path);
            if !regular {
                case_miss.insert(p.id, String::new());
            }
            if let (true, Ok(()), Ok(bytes)) = (regular, exact, std::fs::read(&path)) {
                let url = p.link.clone().or_else(|| urls.first().cloned());
                let name = file.rsplit('/').next().unwrap_or(&file);
                found = Some(
                    store
                        .put_media_bytes(&bytes, url.as_deref().unwrap_or(&file), name)?
                        .id,
                );
            }
        }
        // A URL that differs only by case, unless the uploads show that's another file.
        if found.is_none() && !case_miss.contains_key(&p.id) {
            found = p.link.iter().chain(&urls).find_map(|u| known.folded(u));
        }
        let (canon_id, _) = canon(ItemKind::Document, &format!("post:{}", p.id), None);
        let Some(id) = found else {
            let message = match case_miss.get(&p.id) {
                Some(other) if other.is_empty() => format!(
                    "attachment {}: {file} in uploads is not a regular file (symlink?): not read",
                    p.id
                ),
                Some(other) => format!(
                    "attachment {}: file {file} not found with exact case (found {other})",
                    p.id
                ),
                None => format!(
                    "attachment {} ({}) has no file: not downloaded and not in uploads",
                    p.id,
                    p.link.as_deref().unwrap_or(&file)
                ),
            };
            ctx.issue(
                IssueKind::MissingMedia,
                Severity::Warning,
                Some(canon_id),
                message,
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
        shared
            .entry(id)
            .or_default()
            .push((p.id, nz(&p.alt), p.title.clone(), dir.clone()));
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
    // Attachments sharing one file share one item, which keeps one alt, title and folder.
    for (id, owners) in shared {
        let first = (&owners[0].1, &owners[0].2, &owners[0].3);
        if owners.iter().all(|o| (&o.1, &o.2, &o.3) == first) {
            continue;
        }
        let Some(m) = store.get_item::<Media>(id)? else {
            continue;
        };
        let ids: Vec<u64> = owners.iter().map(|o| o.0).collect();
        ctx.issue(
            IssueKind::MissingMedia,
            Severity::Info,
            Some(id),
            format!(
                "attachments {ids:?} are the same file and share one media item; kept alt {:?}, title {:?}, folder {:?}",
                m.alt, m.title, m.folder
            ),
            json!({ "attachments": ids, "alt": m.alt, "title": m.title, "folder": m.folder }),
        )?;
    }
    Ok(out)
}

/// `path` is a regular file (not a symlink) and, with every link resolved, inside `root`.
fn inside_regular(root: &Path, path: &Path) -> bool {
    let file = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file());
    let inside = match (std::fs::canonicalize(root), std::fs::canonicalize(path)) {
        (Ok(r), Ok(p)) => p.starts_with(r),
        _ => false,
    };
    file && inside
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

    #[test]
    fn exact_case_rejects_case_siblings() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("2024/03")).unwrap();
        std::fs::write(d.path().join("2024/03/Photo.jpg"), b"x").unwrap();
        let mut cache = HashMap::new();
        assert_eq!(
            exact_case(d.path(), "2024/03/Photo.jpg", &mut cache),
            Ok(())
        );
        assert_eq!(
            exact_case(d.path(), "2024/03/photo.jpg", &mut cache),
            Err(Some("Photo.jpg".into()))
        );
        assert_eq!(
            exact_case(d.path(), "2024/03/none.jpg", &mut cache),
            Err(None)
        );
    }
}
