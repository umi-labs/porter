use crate::config::ProjectConfig;
use crate::events::{Event, EventSink, Summary};
use crate::project::Project;
use crate::store::RunId;
use anyhow::Result;
use async_trait::async_trait;
use porter_model::{Issue, IssueKind, ItemId, ItemKind, ItemStatus, Resolution, Severity, Stage};
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feature {
    Layouts,
    RichText,
    Media,
    Forms,
    Submissions,
    Locales,
    Drafts,
    Menus,
    Redirects,
    Taxonomies,
    Seo,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capabilities {
    pub reads: BTreeSet<Feature>,
    pub writes: BTreeSet<Feature>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaMode {
    Check,
    Generate,
    Prompt,
    Spec,
}

/// Everything a stage body needs. Cheap to clone.
#[derive(Clone)]
pub struct StageCtx {
    pub project: Arc<Project>,
    pub config: Arc<ProjectConfig>,
    pub events: EventSink,
    pub cancel: CancellationToken,
    pub run: RunId,
    pub stage: Stage,
    /// `--refresh`: redo work whose output already exists.
    pub refresh: bool,
}

impl StageCtx {
    /// Records the item's status for this stage and emits `ItemResult`.
    pub fn item_status(&self, id: ItemId, kind: ItemKind, status: ItemStatus) -> Result<()> {
        self.project
            .store()
            .set_status(self.run, self.stage, id, &status)?;
        self.events.emit(Event::ItemResult {
            stage: self.stage,
            item: id,
            kind,
            status,
        });
        Ok(())
    }

    /// Opens (or re-opens) an issue in this run and emits `IssueOpened`.
    pub fn issue(
        &self,
        kind: IssueKind,
        severity: Severity,
        item: Option<ItemId>,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Result<()> {
        let issue = Issue {
            id: Uuid::now_v7(),
            kind,
            severity,
            item,
            stage: self.stage,
            message: message.into(),
            details,
            resolution: Resolution::Open,
        };
        let issue = self.project.store().open_issue(self.run, issue)?;
        self.events.emit(Event::IssueOpened { issue });
        Ok(())
    }

    /// Items of `kind` written by `connector` (the source adapter's canonical rows, or
    /// `media`). A row that doesn't decode as `T` gets Failed `undecodable row` and an
    /// error issue rather than being left out.
    pub fn canonical_items<T: serde::de::DeserializeOwned>(
        &self,
        kind: ItemKind,
        connector: &str,
    ) -> Result<Vec<(ItemId, porter_model::SourceRef, T)>> {
        let (ok, bad) = self
            .project
            .store()
            .items_decodable_with_failures::<T>(kind)?;
        for (id, _, e) in bad.into_iter().filter(|(_, s, _)| s.connector == connector) {
            self.issue(
                IssueKind::SchemaMismatch,
                Severity::Error,
                Some(id),
                format!(
                    "stored {} row can't be read: {e} (re-run normalise)",
                    kind.as_str()
                ),
                serde_json::json!({"kind": kind}),
            )?;
            self.item_status(
                id,
                kind,
                ItemStatus::Failed {
                    error: "undecodable row".into(),
                },
            )?;
        }
        Ok(ok
            .into_iter()
            .filter(|(_, s, _)| s.connector == connector)
            .collect())
    }

    pub fn progress(&self, done: u64, total: Option<u64>, label: impl Into<String>) {
        self.events.emit(Event::Progress {
            stage: self.stage,
            done,
            total,
            label: label.into(),
        });
    }
}

/// Runs one stage: begin run, call `f`, tally this run's statuses and issues,
/// end run, emit `StageFinished`. If `f` errors the run is closed as interrupted
/// (so nothing is auto-resolved) and the error propagates.
pub async fn run_stage<F, Fut>(
    project: Arc<Project>,
    config: Arc<ProjectConfig>,
    events: EventSink,
    cancel: CancellationToken,
    stage: Stage,
    f: F,
) -> Result<Summary>
where
    F: FnOnce(StageCtx) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let run = project.store().begin_run(stage)?;
    events.emit(Event::StageStarted { stage, run });
    let ctx = StageCtx {
        project: project.clone(),
        config,
        events: events.clone(),
        cancel: cancel.clone(),
        run,
        stage,
        refresh: false,
    };
    let outcome = f(ctx).await;
    let mut summary = project.store().run_tally(run, stage)?;
    summary.interrupted = outcome.is_err() || cancel.is_cancelled();
    let ended = project.store().end_run(run, &summary);
    events.emit(Event::StageFinished {
        stage,
        run,
        summary: summary.clone(),
    });
    outcome?;
    ended?;
    Ok(summary)
}

#[async_trait]
pub trait Connector: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    async fn analyse(&self, ctx: &StageCtx) -> Result<()>;
    async fn extract(&self, ctx: &StageCtx) -> Result<()>;
}

pub trait Source: Send + Sync {
    /// The connector name normalise writes canonical items under. Load and verify read
    /// canonical items by `source.adapter`, so this must equal the registry name.
    fn canonical(&self) -> &'static str;
    fn connectors(&self, cfg: &ProjectConfig) -> Result<Vec<Box<dyn Connector>>>;
    fn normalise(&self, ctx: &StageCtx) -> Result<()>;
}

/// A reference held by a loaded document, as the destination stores it: what verify
/// compares with the references the source model says the document should have.
#[derive(Clone, Debug, PartialEq)]
pub struct DestReference {
    /// Dotted path to where it sits; the first segment is the top-level field.
    pub field: String,
    pub collection: String,
    pub id: serde_json::Value,
    /// The document's parent in the destination's page tree.
    pub parent: bool,
}

/// What `Destination::references` found. `complete` is false when some references may
/// be stored in a form the destination can't recognise without more information (bare
/// ids with no schema to say which fields are relations): verify then reports a missing
/// reference as a warning, not an error.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedRefs {
    pub refs: Vec<DestReference>,
    pub complete: bool,
}

#[async_trait]
pub trait Destination: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn schema(&self, mode: SchemaMode, ctx: &StageCtx) -> Result<()>;
    async fn load(&self, ctx: &StageCtx, apply: bool) -> Result<()>;
    async fn count(&self, ctx: &StageCtx, dest_type: &str) -> Result<u64>;
    /// Docs in `dest_type` that carry `key_field` (None: the destination's own key for
    /// loaded items), i.e. the ones a migration wrote, not content that was there before.
    async fn count_keyed(
        &self,
        ctx: &StageCtx,
        dest_type: &str,
        key_field: Option<&str>,
    ) -> Result<u64>;
    async fn fetch_loaded(&self, ctx: &StageCtx, id: ItemId) -> Result<Option<serde_json::Value>>;
    /// Every relation, link, upload and parent in `loaded` (a doc from `fetch_loaded`
    /// that landed in `collection`). Verify's only view into the destination's shapes.
    fn references(
        &self,
        ctx: &StageCtx,
        collection: &str,
        loaded: &serde_json::Value,
    ) -> Result<LoadedRefs>;
}
