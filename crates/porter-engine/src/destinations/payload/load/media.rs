//! Media folders, then media: uploaded keyed by `legacyId` = the media item id, 4 at a
//! time until a filename collision (parallel uploads of one name race in Payload), then
//! one at a time. Existing media get changed alt/caption/title/folder PATCHed.

use super::Loader;
use crate::destinations::payload::client::PayloadClient;
use crate::destinations::payload::lexical::{MemIdMap, to_lexical};
use anyhow::Result;
use porter_model::{
    Inline, IssueKind, ItemId, ItemKind, Marks, Media, RichText, RtBlock, Severity,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use tokio::task::JoinSet;

const KIND: ItemKind = ItemKind::Media;
const KEY: &str = "legacyId";
const FOLDERS: &str = "payload-folders";
const CONCURRENCY: usize = 4;

struct Job {
    item: ItemId,
    file: PathBuf,
    filename: String,
    mime: String,
    data: Value,
}

pub(super) async fn load(l: &mut Loader<'_>) -> Result<()> {
    let store = l.ctx.project.store();
    // Media rows are hash-keyed (connector "media"), not the adapter's: load them all.
    let items: Vec<Media> = store
        .items_decodable::<Media>(KIND)?
        .into_iter()
        .map(|(.., m)| m)
        .collect();
    let coll = l.ctx.config.mapping.media.collection.clone();
    for m in &items {
        if let Some(u) = m.variants.first() {
            l.ids.urls.insert(m.id, u.clone());
        }
    }
    let want_folders =
        l.ctx.config.mapping.media.folders && l.coll(&coll).is_some_and(|c| c.folders);
    let folders = if want_folders {
        folders(l, &coll, &items).await?
    } else {
        HashMap::new()
    };
    let fields: Option<BTreeSet<String>> = l
        .coll(&coll)
        .map(|c| c.fields.iter().map(|f| f.name.clone()).collect());
    let rich_caption = l
        .coll(&coll)
        .and_then(|c| c.fields.iter().find(|f| f.name == "caption"))
        .is_some_and(|f| f.kind == "richText");

    let mut jobs = VecDeque::new();
    let mut partial: HashMap<ItemId, String> = HashMap::new();
    for m in &items {
        let mut data = Map::new();
        for (k, v) in [
            ("alt", &m.alt),
            ("caption", &m.caption),
            ("title", &m.title),
        ] {
            if fields.as_ref().is_some_and(|f| !f.contains(k)) {
                continue;
            }
            let v = match v {
                Some(s) if k == "caption" && rich_caption => lexical(s),
                Some(s) => json!(s),
                None => Value::Null,
            };
            data.insert(k.into(), v);
        }
        if let Some(dir) = m.folder.as_deref().filter(|_| want_folders) {
            match folders.get(dir) {
                Some(id) => {
                    data.insert("folder".into(), id.clone());
                }
                None => {
                    partial.insert(m.id, format!("folder `{dir}` not created"));
                }
            }
        }
        let data = Value::Object(data);
        let file = store.media_path(m.id)?;
        if !file.exists() {
            let error = format!("no downloaded file for {}", m.filename);
            l.ctx.issue(
                IssueKind::MissingMedia,
                Severity::Error,
                Some(m.id),
                error.clone(),
                json!({"url": m.variants.first()}),
            )?;
            l.status(m.id, KIND, porter_model::ItemStatus::Failed { error })?;
            continue;
        }
        if !l.apply() {
            let mut body = data.clone();
            body[KEY] = json!(m.id.to_string());
            l.preview(&coll, m.id, &body)?;
            l.placeholder(m.id, &coll);
            l.finish(m.id, KIND, partial.remove(&m.id))?;
            continue;
        }
        jobs.push_back(Job {
            item: m.id,
            file,
            filename: m.filename.clone(),
            mime: mime(&m.mime, &m.filename),
            data,
        });
    }
    let Some(client) = l.client.clone() else {
        return Ok(());
    };

    let total = jobs.len() as u64;
    let (mut done, mut limit) = (0u64, CONCURRENCY);
    let mut running: JoinSet<(Job, Result<Value>)> = JoinSet::new();
    loop {
        while running.len() < limit && !l.cancelled() {
            let Some(job) = jobs.pop_front() else { break };
            let (c, coll) = (client.clone(), coll.clone());
            running.spawn(async move {
                let r = upload(&c, &coll, &job).await;
                (job, r)
            });
        }
        let Some(joined) = running.join_next().await else {
            break;
        };
        let (job, r) = joined?;
        match r {
            Ok(id) => {
                done += 1;
                l.ctx
                    .progress(done, Some(total), format!("upload {}", job.filename));
                l.landed(job.item, &coll, id)?;
                l.finish(job.item, KIND, partial.remove(&job.item))?;
            }
            Err(e) if limit > 1 && collision(&e) => {
                limit = 1;
                jobs.push_back(job);
            }
            Err(e) => l.fail(job.item, KIND, &coll, format!("{e:#}"))?,
        }
    }
    Ok(())
}

async fn upload(c: &PayloadClient, coll: &str, job: &Job) -> Result<Value> {
    let key = json!(job.item.to_string());
    let (doc, created) = c
        .upload_keyed(
            coll,
            KEY,
            &key,
            &job.file,
            &job.filename,
            &job.mime,
            &job.data,
        )
        .await?;
    let id = doc["id"].clone();
    if !created {
        let changed: Map<String, Value> = job
            .data
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, v)| !same(&doc[k.as_str()], v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !changed.is_empty() {
            c.update(coll, &id, &Value::Object(changed), false).await?;
        }
    }
    Ok(id)
}

/// Equal, counting a populated relation (`{id, ...}`) as its id.
fn same(have: &Value, want: &Value) -> bool {
    have == want || (have.is_null() && want.is_null()) || have.get("id") == Some(want)
}

/// Payload's unique-filename race between parallel uploads of one name.
fn collision(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}").to_lowercase();
    s.contains("filename")
        && (s.contains("unique") || s.contains("exists") || s.contains("duplicate"))
}

/// Upserts each folder path (`a`, `a/b`, ...) into `payload-folders`; path -> folder id.
/// A folder that can't be written is an error issue; its media load without it.
async fn folders(
    l: &mut Loader<'_>,
    coll: &str,
    items: &[Media],
) -> Result<HashMap<String, Value>> {
    let mut paths = BTreeSet::new();
    for dir in items.iter().filter_map(|m| m.folder.as_deref()) {
        let segs: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
        for i in 1..=segs.len() {
            paths.insert(segs[..i].join("/"));
        }
    }
    let mut out: HashMap<String, Value> = HashMap::new();
    for p in paths {
        if l.cancelled() {
            break;
        }
        let (parent, name) = match p.rsplit_once('/') {
            Some((a, b)) => (Some(a.to_string()), b.to_string()),
            None => (None, p.clone()),
        };
        let parent_id = match &parent {
            Some(a) => match out.get(a) {
                Some(id) => Some(id.clone()),
                None => continue, // parent failed: already reported
            },
            None => None,
        };
        let Some(client) = l.client.clone() else {
            out.insert(p.clone(), json!(format!("dry-run:folder:{p}")));
            continue;
        };
        match folder(&client, coll, &name, parent_id.as_ref()).await {
            Ok(id) => {
                out.insert(p, id);
            }
            Err(e) => l.ctx.issue(
                IssueKind::WriteFailed,
                Severity::Error,
                None,
                format!("media folder `{p}` not written: {e:#}"),
                json!({"folder": p}),
            )?,
        }
    }
    Ok(out)
}

async fn folder(
    c: &PayloadClient,
    coll: &str,
    name: &str,
    parent: Option<&Value>,
) -> Result<Value> {
    let mut filter = vec![
        ("where[name][equals]".to_string(), name.to_string()),
        ("where[folderType][contains]".to_string(), coll.to_string()),
    ];
    match parent {
        Some(p) => filter.push((
            "where[folder][equals]".into(),
            p.to_string().trim_matches('"').into(),
        )),
        None => filter.push(("where[folder][exists]".into(), "false".into())),
    }
    if let Some(found) = c.find_one(FOLDERS, &filter).await? {
        return Ok(found["id"].clone());
    }
    // Unkeyed create, sent once. On an error, look again before reporting: Payload may
    // have committed before failing.
    let created = c
        .create(
            FOLDERS,
            &json!({"name": name, "folder": parent, "folderType": [coll]}),
            false,
        )
        .await;
    match created {
        Ok(doc) => Ok(doc["id"].clone()),
        Err(e) => match c.find_one(FOLDERS, &filter).await? {
            Some(found) => Ok(found["id"].clone()),
            None => Err(e),
        },
    }
}

fn lexical(s: &str) -> Value {
    let rt = RichText {
        blocks: vec![RtBlock::Paragraph {
            inlines: vec![Inline::Text {
                text: s.to_string(),
                marks: Marks::default(),
            }],
            align: Default::default(),
        }],
    };
    to_lexical(&rt, &MemIdMap::default(), None).0
}

/// The stored type, or one guessed from the extension when the store only knows bytes.
fn mime(stored: &str, filename: &str) -> String {
    if stored != "application/octet-stream" {
        return stored.to_string();
    }
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        _ => stored,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_detection() {
        let e = anyhow::anyhow!("POST /api/media: HTTP 400: Value must be unique (filename: x)");
        assert!(collision(&e));
        assert!(!collision(&anyhow::anyhow!("HTTP 500: boom")));
    }

    #[test]
    fn mime_guess() {
        assert_eq!(mime("application/octet-stream", "a.JPG"), "image/jpeg");
        assert_eq!(mime("image/png", "a.jpg"), "image/png");
        assert_eq!(
            mime("application/octet-stream", "a.bin"),
            "application/octet-stream"
        );
    }
}
