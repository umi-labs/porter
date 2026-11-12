use super::{NOW, Store};
use crate::url::normalise_url;
use anyhow::{Context, Result, anyhow};
use porter_model::{ItemId, ItemKind, Media, SourceRef};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

/// Result of storing a file.
#[derive(Debug, Clone)]
pub struct MediaHandle {
    pub id: ItemId,
    pub hash: String,
    pub path: PathBuf,
    /// False when these exact bytes were already stored.
    pub is_new: bool,
}

impl Store {
    /// `media/<h0h1>/<h2h3>/<sha256>`; paths are hashes so case-insensitive filesystems are safe.
    fn hash_path(&self, hash: &str) -> PathBuf {
        self.media_dir
            .join(&hash[0..2])
            .join(&hash[2..4])
            .join(hash)
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
        let path = self.hash_path(&hash);
        let is_new = !path.exists();
        if is_new {
            let dir = path.parent().ok_or_else(|| anyhow!("no parent dir"))?;
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            // Temp file then rename: an interrupted write never leaves a partial hash-named file.
            let tmp = dir.join(format!(".{hash}.{}.tmp", std::process::id()));
            fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
            fs::rename(&tmp, &path).with_context(|| format!("renaming to {}", path.display()))?;
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
        self.add_media_variant(id, url)?;
        Ok(MediaHandle {
            id,
            hash,
            path,
            is_new,
        })
    }

    /// Records `url` as a variant of `id`. If the URL key already maps to another media,
    /// the latest call wins for the mapping; nothing is deleted.
    pub fn add_media_variant(&self, id: ItemId, url: &str) -> Result<()> {
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
        if let Some(key) = normalise_url(url, None) {
            self.conn().execute(
                &format!(
                    "INSERT INTO media_variants (url_key, url, media_id) VALUES (?1, ?2, ?3)
                     ON CONFLICT(url_key) DO UPDATE SET url = excluded.url,
                       media_id = excluded.media_id, updated_at = {NOW}, deleted_at = NULL"
                ),
                params![key.0, url, id.to_string()],
            )?;
        }
        Ok(())
    }

    pub fn media_by_url(&self, url: &str) -> Result<Option<ItemId>> {
        let Some(key) = normalise_url(url, None) else {
            return Ok(None);
        };
        let id: Option<String> = self
            .conn()
            .query_row(
                "SELECT media_id FROM media_variants WHERE url_key = ?1 AND deleted_at IS NULL",
                [key.0],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id.map(|s| s.parse()).transpose()?)
    }

    pub fn media_path(&self, id: ItemId) -> Result<PathBuf> {
        let media: Media = self
            .get_item(id)?
            .ok_or_else(|| anyhow!("no media item {id}"))?;
        Ok(self.hash_path(&media.hash))
    }
}
