//! `porter load` into Payload. Order: media folders, media, terms, documents (pass 1:
//! create with scalars, parents first; pass 2: references, rich text, layouts, parent),
//! menus, forms, submissions, redirects, settings. Every write is an upsert on a key, and
//! where each item landed is kept in `dest_ids` as it goes, so a re-run (or a resume after
//! an interruption) updates and never duplicates. Without `apply` nothing is sent: each
//! would-be body goes to `raw/load-preview/<collection>/<item>.json`.

mod documents;
mod extras;
mod media;

use super::client::{PayloadClient, prepare};
use super::lexical::{DestRef, IdMap};
use super::ts::{CollectionSchema, DestField, DestSchema, read_payload_config};
use crate::html::PendingIssue;
use crate::stage::StageCtx;
use crate::verify::{NOT_PRODUCED, REMOVED_FROM_SOURCE};
use anyhow::Result;
use porter_model::{IssueKind, ItemId, ItemKind, ItemStatus, Severity, SourceRef};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// What a dry run records for each item it would write (a skip, since nothing was sent).
pub const DRY_RUN: &str = "dry run";
const NOT_MAPPED: &str = "no destination mapped";

/// Item -> where it is in the destination, from `dest_ids` plus this run's writes.
#[derive(Default)]
pub(super) struct LoadIds {
    refs: HashMap<ItemId, DestRef>,
    urls: HashMap<ItemId, String>,
    /// New URL paths of documents, from the mapping's path patterns.
    paths: HashMap<ItemId, String>,
}

impl IdMap for LoadIds {
    fn get(&self, id: ItemId) -> Option<DestRef> {
        self.refs.get(&id).cloned()
    }
    fn old_url(&self, id: ItemId) -> Option<String> {
        self.urls.get(&id).cloned()
    }
}

impl LoadIds {
    fn insert(&mut self, item: ItemId, collection: &str, id: Value) {
        let path = self.paths.get(&item).cloned();
        self.refs.insert(
            item,
            DestRef {
                collection: collection.to_string(),
                id,
                path,
            },
        );
    }
}

pub(super) struct Loader<'a> {
    ctx: &'a StageCtx,
    /// None on a dry run: nothing is sent.
    client: Option<Arc<PayloadClient>>,
    schema: Option<DestSchema>,
    /// `[mapping]` with the tidy rules compiled in.
    mapping: crate::mapping::MappingConfig,
    ids: LoadIds,
}

pub(super) async fn run(
    ctx: &StageCtx,
    client: impl FnOnce() -> Result<PayloadClient>,
    config_dir: Option<std::path::PathBuf>,
    apply: bool,
) -> Result<()> {
    let preview_dir = ctx.project.raw_dir("load-preview");
    if !apply {
        // The previews are exactly this dry run's: nothing left from an earlier one.
        match std::fs::remove_dir_all(&preview_dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    let client = if apply {
        Some(Arc::new(client()?))
    } else {
        None
    };
    let schema = match config_dir {
        Some(dir) => Some(read_payload_config(&dir)?),
        None => {
            ctx.issue(
                IssueKind::SchemaMismatch,
                Severity::Warning,
                None,
                "destination.config_dir is not set: load can't check required fields, write polymorphic relations, clear array fields or file media into folders",
                json!({}),
            )?;
            None
        }
    };
    // Tidy rules are accounted first: a field one would lose or mangle blocks the load.
    let mapping = crate::tidy::effective_mapping(&ctx.config)?;
    if !crate::tidy::check_before_load(ctx, schema.as_ref())? {
        return Ok(());
    }
    let mut l = Loader {
        ctx,
        client,
        schema,
        mapping,
        ids: LoadIds::default(),
    };
    for (item, collection, id) in ctx.project.store().dest_ids()? {
        l.ids.insert(item, &collection, id);
    }
    removed(&l)?;
    media::load(&mut l).await?;
    if l.cancelled() {
        return Ok(());
    }
    extras::terms(&mut l).await?;
    if l.cancelled() {
        return Ok(());
    }
    documents::load(&mut l).await?;
    if l.cancelled() {
        return Ok(());
    }
    extras::menus(&mut l).await?;
    if l.cancelled() {
        return Ok(());
    }
    extras::forms(&mut l).await?;
    if l.cancelled() {
        return Ok(());
    }
    extras::submissions(&mut l).await?;
    if l.cancelled() {
        return Ok(());
    }
    extras::redirects(&mut l).await?;
    if l.cancelled() {
        return Ok(());
    }
    extras::settings(&mut l).await?;
    extras::authors(&l)?;
    if !apply {
        mark_preview_done(ctx, &preview_dir)?;
    }
    Ok(())
}

/// Marks a dry run's previews complete and says when they were made, unless it was
/// cancelled (in settings or authors, say): a partial preview must never back an apply.
fn mark_preview_done(ctx: &StageCtx, preview_dir: &std::path::Path) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        return Ok(());
    }
    std::fs::create_dir_all(preview_dir)?;
    std::fs::write(preview_dir.join(PREVIEW_DONE), "")?;
    Ok(())
}

/// Written when a dry run ran to the end; its mtime is the preview's age.
pub const PREVIEW_DONE: &str = ".done";

/// Items the source no longer has but an earlier load wrote: never deleted here (that is
/// the editor's call), but each gets a warning saying where it still is, and a status.
fn removed(l: &Loader) -> Result<()> {
    let adapter = &l.ctx.config.source.adapter;
    for (item, kind, collection, id) in l.ctx.project.store().removed_with_dest(adapter)? {
        let at = match &id {
            Value::String(s) => format!("{collection}/{s}"),
            v => format!("{collection}/{v}"),
        };
        // Normalise says whether it left the source or just wasn't read this run.
        let not_read = matches!(
            l.ctx.project.store().status(porter_model::Stage::Normalise, item)?,
            Some(ItemStatus::Skipped { reason }) if reason == NOT_PRODUCED
        );
        let (message, reason) = if not_read {
            (
                format!("{NOT_PRODUCED}; still in destination at {at} as an earlier load wrote it"),
                NOT_PRODUCED,
            )
        } else {
            (
                format!(
                    "removed from source; still in destination at {at} — unpublish or delete by hand"
                ),
                REMOVED_FROM_SOURCE,
            )
        };
        l.ctx.issue(
            IssueKind::SchemaMismatch,
            Severity::Warning,
            Some(item),
            message,
            json!({"collection": collection, "id": id}),
        )?;
        l.skip(item, kind, reason)?;
    }
    Ok(())
}

/// The loaded doc for `id`, through the client.
pub(super) async fn fetch(
    client: &PayloadClient,
    ctx: &StageCtx,
    id: ItemId,
) -> Result<Option<Value>> {
    match ctx.project.store().dest_id(id)? {
        Some((c, _)) if c.starts_with("globals/") => Ok(None),
        Some((c, v)) => client.get(&c, &v).await,
        None => Ok(None),
    }
}

impl Loader<'_> {
    fn cancelled(&self) -> bool {
        self.ctx.cancel.is_cancelled()
    }

    fn apply(&self) -> bool {
        self.client.is_some()
    }

    /// Normalised items of `kind` from the source adapter (connector rows and other
    /// connectors' data are not loadable).
    fn items<T: DeserializeOwned>(&self, kind: ItemKind) -> Result<Vec<(ItemId, SourceRef, T)>> {
        self.ctx
            .canonical_items(kind, &self.ctx.config.source.adapter)
    }

    fn coll(&self, name: &str) -> Option<&CollectionSchema> {
        self.schema.as_ref()?.collections.get(name)
    }

    fn status(&self, id: ItemId, kind: ItemKind, s: ItemStatus) -> Result<()> {
        self.ctx.item_status(id, kind, s)
    }

    fn skip(&self, id: ItemId, kind: ItemKind, reason: &str) -> Result<()> {
        self.status(
            id,
            kind,
            ItemStatus::Skipped {
                reason: reason.into(),
            },
        )
    }

    /// Done, Partial when `partial` names what is missing, or on a dry run Skipped (the
    /// issues are raised either way).
    fn finish(&self, id: ItemId, kind: ItemKind, partial: Option<String>) -> Result<()> {
        let s = match partial {
            _ if !self.apply() => ItemStatus::Skipped {
                reason: DRY_RUN.into(),
            },
            Some(reason) => ItemStatus::Partial { reason },
            None => ItemStatus::Done,
        };
        self.status(id, kind, s)
    }

    /// A failed write: Failed plus a `write-failed` error. The load goes on.
    fn fail(&self, id: ItemId, kind: ItemKind, collection: &str, error: String) -> Result<()> {
        self.ctx.issue(
            IssueKind::WriteFailed,
            Severity::Error,
            Some(id),
            format!("write to {collection} failed: {error}"),
            json!({"collection": collection}),
        )?;
        self.status(id, kind, ItemStatus::Failed { error })
    }

    /// Raises `issues` against `item`; returns the first warning or error, the reason the
    /// item is only partly loaded.
    fn emit(&self, item: ItemId, issues: Vec<PendingIssue>) -> Result<Option<String>> {
        let mut first = None;
        for i in issues {
            if i.severity != Severity::Info && first.is_none() {
                first = Some(i.message.clone());
            }
            self.ctx
                .issue(i.kind, i.severity, Some(item), i.message, i.details)?;
        }
        Ok(first)
    }

    /// One info issue for a kind that has items but no destination in the mapping.
    fn unmapped<T>(&self, items: &[T], what: &str, set: &str) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        self.ctx.issue(
            IssueKind::UnmappedField,
            Severity::Info,
            None,
            format!(
                "{} {what} not loaded: no destination mapped (set {set})",
                items.len()
            ),
            json!({"kind": what, "count": items.len()}),
        )
    }

    fn preview(&self, collection: &str, item: ItemId, body: &Value) -> Result<()> {
        let dir = self.ctx.project.raw_dir("load-preview").join(collection);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join(format!("{item}.json")),
            serde_json::to_string_pretty(body)?,
        )?;
        Ok(())
    }

    /// Records a dry-run stand-in id (or the id from an earlier run) so references resolve.
    fn placeholder(&mut self, item: ItemId, collection: &str) -> Value {
        let id = match self.ids.refs.get(&item) {
            Some(r) if r.collection == collection => r.id.clone(),
            _ => json!(format!("dry-run:{item}")),
        };
        self.ids.insert(item, collection, id.clone());
        id
    }

    fn landed(&mut self, item: ItemId, collection: &str, id: Value) -> Result<()> {
        self.ctx
            .project
            .store()
            .put_dest_id(item, collection, &id)?;
        self.ids.insert(item, collection, id);
        Ok(())
    }

    /// Upserts `data` keyed by `key_field = key` and records where it landed. Dry run:
    /// writes the preview instead. `Err` is the write's error, for the item's status.
    async fn upsert(
        &mut self,
        item: ItemId,
        collection: &str,
        key_field: &str,
        key: &Value,
        data: Value,
        draft: bool,
    ) -> Result<Result<Value, String>> {
        let Some(client) = self.client.clone() else {
            let mut body = data;
            body[key_field] = key.clone();
            self.preview(collection, item, &prepare(&body, draft))?;
            return Ok(Ok(self.placeholder(item, collection)));
        };
        match client
            .upsert(collection, key_field, key, &data, draft)
            .await
        {
            Ok((doc, _)) => {
                let id = doc["id"].clone();
                self.landed(item, collection, id.clone())?;
                Ok(Ok(id))
            }
            Err(e) => Ok(Err(format!("{e:#}"))),
        }
    }

    /// The relation value for `id` in field `f`: `{relationTo, value}` when the field is
    /// polymorphic, else the plain id. Null plus a warning when the target isn't loaded
    /// (issue `kind`), or when the field is known and doesn't accept the target's
    /// collection: Payload ids are per collection, so a wrong one would point elsewhere.
    fn relation(
        &self,
        id: ItemId,
        f: Option<&DestField>,
        kind: IssueKind,
        out: &mut Vec<PendingIssue>,
    ) -> Value {
        let Some(r) = self.ids.get(id) else {
            let what = if kind == IssueKind::MissingMedia {
                "media"
            } else {
                "document"
            };
            out.push(PendingIssue {
                kind,
                severity: Severity::Warning,
                message: format!("{what} {id} is not loaded: reference left empty"),
                details: json!({"target": id, "old_url": self.ids.old_url(id)}),
            });
            return Value::Null;
        };
        match f {
            Some(f) if !accepts(f, &r.collection) => {
                out.push(wrong_collection(&f.name, id, &r.collection, &f.relation_to));
                Value::Null
            }
            Some(f) if f.polymorphic => json!({"relationTo": r.collection, "value": r.id}),
            _ => r.id,
        }
    }
}

/// Whether relation field `f` takes `collection` (unknown `relationTo`: can't tell, yes).
pub(super) fn accepts(f: &DestField, collection: &str) -> bool {
    f.relation_to.is_empty() || f.relation_to.iter().any(|c| c == collection)
}

pub(super) fn wrong_collection(
    field: &str,
    id: impl std::fmt::Display,
    have: &str,
    want: &[String],
) -> PendingIssue {
    PendingIssue {
        kind: IssueKind::UnresolvedRelation,
        severity: Severity::Warning,
        message: format!(
            "`{field}` takes {} but {id} was loaded into {have}: reference left empty",
            want.join("/")
        ),
        details: json!({"field": field, "target": id.to_string(), "collection": have, "accepts": want}),
    }
}

/// `Value` is "empty" for a required field: missing, null, blank text or no items.
fn empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    }
}

/// The first required field `data` sets but leaves empty (a dotted path), walking into
/// groups it sets. Fields `data` doesn't set are the schema check's to report.
fn required_gap(fields: &[DestField], data: &Map<String, Value>, prefix: &str) -> Option<String> {
    for f in fields {
        let Some(v) = data.get(&f.name) else { continue };
        let path = format!("{prefix}{}", f.name);
        if f.required && empty(Some(v)) {
            return Some(path);
        }
        if f.kind == "group"
            && let Some(o) = v.as_object()
            && let Some(p) = required_gap(&f.fields, o, &format!("{path}."))
        {
            return Some(p);
        }
    }
    None
}

/// Schema-aware nulls: a null for an array, blocks or hasMany field becomes `[]` (Payload
/// rejects null there); other nulls stay, so a re-run clears the field. Without a schema
/// for the collection nulls are dropped: clearing can't be told apart from breaking.
fn settle_nulls(fields: Option<&[DestField]>, data: &mut Map<String, Value>) {
    let Some(fields) = fields else {
        data.retain(|_, v| !v.is_null());
        for v in data.values_mut() {
            if let Some(o) = v.as_object_mut() {
                settle_nulls(None, o);
            }
        }
        return;
    };
    for (k, v) in data.iter_mut() {
        let Some(f) = fields.iter().find(|f| &f.name == k) else {
            continue;
        };
        if v.is_null() && (matches!(f.kind.as_str(), "array" | "blocks") || f.many) {
            *v = json!([]);
        } else if f.kind == "group" {
            // Payload throws reading a sub-field of a null group: clear its fields instead.
            if v.is_null() {
                *v = Value::Object(
                    f.fields
                        .iter()
                        .map(|x| (x.name.clone(), Value::Null))
                        .collect(),
                );
            }
            if let Some(o) = v.as_object_mut() {
                settle_nulls(Some(&f.fields), o);
            }
        }
    }
}

/// Sets a dotted path, creating groups on the way.
fn set_path(data: &mut Map<String, Value>, path: &str, v: Value) {
    match path.split_once('.') {
        None => {
            data.insert(path.to_string(), v);
        }
        Some((head, rest)) => {
            let e = data
                .entry(head.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !e.is_object() {
                *e = Value::Object(Map::new());
            }
            if let Some(o) = e.as_object_mut() {
                set_path(o, rest, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_dry_run_never_marks_its_preview_done() {
        let d = tempfile::tempdir().unwrap();
        crate::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let project = std::sync::Arc::new(crate::project::Project::open(d.path()).unwrap());
        let ctx = StageCtx {
            config: std::sync::Arc::new(project.config().clone()),
            events: crate::events::EventSink::new(16),
            cancel: tokio_util::sync::CancellationToken::new(),
            run: project
                .store()
                .begin_run(porter_model::Stage::Load)
                .unwrap(),
            stage: porter_model::Stage::Load,
            refresh: false,
            project,
        };
        let dir = d.path().join("raw/load-preview");
        ctx.cancel.cancel();
        mark_preview_done(&ctx, &dir).unwrap();
        assert!(!dir.join(PREVIEW_DONE).exists(), "cancelled: no marker");
        let ctx = StageCtx {
            cancel: tokio_util::sync::CancellationToken::new(),
            ..ctx
        };
        mark_preview_done(&ctx, &dir).unwrap();
        assert!(dir.join(PREVIEW_DONE).exists());
    }

    fn field(name: &str, kind: &str) -> DestField {
        DestField {
            name: name.into(),
            kind: kind.into(),
            ..Default::default()
        }
    }

    #[test]
    fn nulls_follow_the_schema() {
        let mut group = field("seo", "group");
        group.fields = vec![field("tags", "array"), field("image", "upload")];
        let fields = vec![field("layout", "blocks"), field("title", "text"), group];
        let mut d =
            json!({"layout": null, "title": null, "seo": {"tags": null, "image": null}, "x": null});
        settle_nulls(Some(&fields), d.as_object_mut().unwrap());
        assert_eq!(
            d,
            json!({"layout": [], "title": null, "seo": {"tags": [], "image": null}, "x": null})
        );
        // Payload crashes reading a sub-field of a null group: send its fields as nulls.
        let mut d = json!({"seo": null});
        settle_nulls(Some(&fields), d.as_object_mut().unwrap());
        assert_eq!(d, json!({"seo": {"tags": [], "image": null}}));
        let mut d = json!({"a": null, "g": {"b": null, "c": 1}});
        settle_nulls(None, d.as_object_mut().unwrap());
        assert_eq!(d, json!({"g": {"c": 1}}));
    }

    #[test]
    fn required_gap_walks_groups() {
        let mut t = field("title", "text");
        t.required = true;
        let mut g = field("seo", "group");
        let mut inner = field("title", "text");
        inner.required = true;
        g.fields = vec![inner];
        let fields = vec![t, g];
        let ok = json!({"title": "T", "seo": {"title": "S"}});
        assert_eq!(required_gap(&fields, ok.as_object().unwrap(), ""), None);
        let bad = json!({"title": "T", "seo": {"title": " "}});
        assert_eq!(
            required_gap(&fields, bad.as_object().unwrap(), "").as_deref(),
            Some("seo.title")
        );
        let unset = json!({"seo": {}});
        assert_eq!(required_gap(&fields, unset.as_object().unwrap(), ""), None);
    }

    #[test]
    fn set_path_nests() {
        let mut m = Map::new();
        set_path(&mut m, "seo.title", json!("a"));
        set_path(&mut m, "seo.description", json!("b"));
        set_path(&mut m, "title", json!("t"));
        assert_eq!(
            Value::Object(m),
            json!({"seo": {"title": "a", "description": "b"}, "title": "t"})
        );
    }
}
