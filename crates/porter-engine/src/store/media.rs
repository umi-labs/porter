use super::{NOW, Store};
use crate::url::normalise_url;
use anyhow::{Context, Result, anyhow};
use porter_model::{ItemId, ItemKind, Media, SourceRef};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Result of storing a file.
#[derive(Debug, Clone)]
pub struct MediaHandle {
    pub id: ItemId,
    pub hash: String,
    pub path: PathBuf,
    /// False when these exact bytes were already stored.
    pub is_new: bool,
    /// Set when this URL previously pointed at a different media item (its id).
    pub replaced: Option<ItemId>,
}

impl Store {
    /// `media/<h0h1>/<h2h3>/<sha256>`; paths are hashes so case-insensitive filesystems are safe.
    /// Errors on anything but 64 lowercase hex digits: a hash read back from the store is
    /// never trusted to be sliceable or free of path separators.
    fn hash_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() != 64 || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(anyhow!("invalid media hash {hash:?}"));
        }
        Ok(self
            .media_dir
            .join(&hash[0..2])
            .join(&hash[2..4])
            .join(hash))
    }

    pub fn put_media_bytes(
        &self,
        bytes: &[u8],
        url: &str,
        suggested_name: &str,
    ) -> Result<MediaHandle> {
        let hash: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = self.hash_path(&hash)?;
        let is_new = !path.exists();
        if is_new {
            let dir = path.parent().ok_or_else(|| anyhow!("no parent dir"))?;
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            // Synced temp file then rename: an interrupted write never leaves a partial
            // hash-named file, and a failure drops the temp file.
            let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
            tmp.write_all(bytes)?;
            tmp.as_file().sync_all()?;
            tmp.persist(&path)
                .with_context(|| format!("renaming to {}", path.display()))?;
        }
        let src = SourceRef {
            connector: "media".into(),
            source_id: hash.clone(),
            url: Some(url.to_string()),
        };
        let id = ItemId::new(ItemKind::Media, &src);
        if self.get_item::<Media>(id)?.is_none() {
            let media = Media {
                id,
                hash: hash.clone(),
                variants: vec![],
                filename: suggested_name.to_string(),
                // ponytail: no content sniffing; converters/loaders set mime when they need it.
                mime: "application/octet-stream".into(),
                width: None,
                height: None,
                alt: None,
                caption: None,
                title: None,
                credit: None,
                folder: None,
                usages: vec![],
            };
            self.put_item(ItemKind::Media, id, &src, &media)?;
        }
        let replaced = self.add_media_variant(id, url)?;
        Ok(MediaHandle {
            id,
            hash,
            path,
            is_new,
            replaced,
        })
    }

    /// Records `url` as a variant of `id`. If the URL key already maps to another media,
    /// the latest call wins for the mapping; nothing is deleted.
    pub fn add_media_variant(&self, id: ItemId, url: &str) -> Result<Option<ItemId>> {
        let mut media: Media = self
            .get_item(id)?
            .ok_or_else(|| anyhow!("no media item {id}"))?;
        if !media.variants.iter().any(|v| v == url) {
            media.variants.push(url.to_string());
            let src = SourceRef {
                connector: "media".into(),
                source_id: media.hash.clone(),
                url: media.variants.first().cloned(),
            };
            self.put_item(ItemKind::Media, id, &src, &media)?;
        }
        let mut replaced = None;
        if let Some(key) = normalise_url(url, None) {
            let conn = self.conn();
            let prev: Option<String> = conn
                .query_row(
                    "SELECT media_id FROM media_variants WHERE url_key = ?1 AND deleted_at IS NULL",
                    [&key.0],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(p) = prev.filter(|p| *p != id.to_string()) {
                replaced = Some(p.parse()?);
            }
            conn.execute(
                &format!(
                    "INSERT INTO media_variants (url_key, url, media_id) VALUES (?1, ?2, ?3)
                     ON CONFLICT(url_key) DO UPDATE SET url = excluded.url,
                       media_id = excluded.media_id, updated_at = {NOW}, deleted_at = NULL"
                ),
                params![key.0, url, id.to_string()],
            )?;
        }
        Ok(replaced)
    }

    /// Every live `(url, media item)` in the URL table: what each URL is now.
    pub fn media_variant_rows(&self) -> Result<Vec<(String, ItemId)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT url, media_id FROM media_variants WHERE deleted_at IS NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (url, id) = row?;
            out.push((url, id.parse()?));
        }
        Ok(out)
    }

    pub fn media_by_url(&self, url: &str) -> Result<Option<ItemId>> {
        let Some(key) = normalise_url(url, None) else {
            return Ok(None);
        };
        let conn = self.conn();
        let exact: Option<String> = conn
            .query_row(
                "SELECT media_id FROM media_variants WHERE url_key = ?1 AND deleted_at IS NULL",
                [&key.0],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = exact {
            return Ok(Some(id.parse()?));
        }
        // Case-insensitive fallback, only when it is unambiguous.
        let mut stmt = conn.prepare(
            "SELECT media_id FROM media_variants WHERE lower(url_key) = lower(?1) AND deleted_at IS NULL LIMIT 2",
        )?;
        let ids: Vec<String> = stmt
            .query_map([&key.0], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        match ids.as_slice() {
            [one] => Ok(Some(one.parse()?)),
            _ => Ok(None),
        }
    }

    pub fn media_path(&self, id: ItemId) -> Result<PathBuf> {
        let media: Media = self
            .get_item(id)?
            .ok_or_else(|| anyhow!("no media item {id}"))?;
        self.hash_path(&media.hash)
    }
}
