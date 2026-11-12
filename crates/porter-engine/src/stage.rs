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
    fn connectors(&self, cfg: &ProjectConfig) -> Result<Vec<Box<dyn Connector>>>;
    fn normalise(&self, ctx: &StageCtx) -> Result<()>;
}

#[async_trait]
pub trait Destination: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn schema(&self, mode: SchemaMode, ctx: &StageCtx) -> Result<()>;
    async fn load(&self, ctx: &StageCtx, apply: bool) -> Result<()>;
    async fn count(&self, ctx: &StageCtx, dest_type: &str) -> Result<u64>;
    async fn fetch_loaded(&self, ctx: &StageCtx, id: ItemId) -> Result<Option<serde_json::Value>>;
}
