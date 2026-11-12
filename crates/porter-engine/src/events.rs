use crate::store::RunId;
use porter_model::{Issue, ItemId, ItemKind, ItemStatus, Stage};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Counts for one stage run, stored on the run row by `Store::end_run`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub done: u64,
    pub partial: u64,
    pub skipped: u64,
    pub failed: u64,
    pub errors: u64,
    pub warnings: u64,
    pub infos: u64,
    pub interrupted: bool,
}

/// 0 clean, 2 failed items or error issues, 3 interrupted (resumable).
/// (1, "could not run", is the caller's: `run_stage` returned `Err`.)
pub fn exit_code(s: &Summary) -> i32 {
    if s.interrupted {
        3
    } else if s.failed > 0 || s.errors > 0 {
        2
    } else {
        0
    }
}

/// Progress stream for the CLI (`--json` NDJSON), TUI and desktop app.
/// Wire shape: `{"v":1,"event":"<kebab>",...}`.
#[derive(Clone, Debug)]
pub enum Event {
    StageStarted {
        stage: Stage,
        run: RunId,
    },
    Progress {
        stage: Stage,
        done: u64,
        total: Option<u64>,
        label: String,
    },
    ItemResult {
        stage: Stage,
        item: ItemId,
        kind: ItemKind,
        status: ItemStatus,
    },
    IssueOpened {
        issue: Issue,
    },
    StageFinished {
        stage: Stage,
        run: RunId,
        summary: Summary,
    },
}

#[derive(Serialize)]
#[serde(remote = "Event", tag = "event", rename_all = "kebab-case")]
enum EventDef {
    StageStarted {
        stage: Stage,
        run: RunId,
    },
    Progress {
        stage: Stage,
        done: u64,
        total: Option<u64>,
        label: String,
    },
    ItemResult {
        stage: Stage,
        item: ItemId,
        kind: ItemKind,
        status: ItemStatus,
    },
    IssueOpened {
        issue: Issue,
    },
    StageFinished {
        stage: Stage,
        run: RunId,
        summary: Summary,
    },
}

impl Serialize for Event {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Versioned<'a> {
            v: u8,
            #[serde(flatten, serialize_with = "EventDef::serialize")]
            e: &'a Event,
        }
        Versioned { v: 1, e: self }.serialize(s)
    }
}

#[derive(Clone)]
pub struct EventSink(pub broadcast::Sender<Event>);

impl EventSink {
    pub fn new(capacity: usize) -> EventSink {
        EventSink(broadcast::channel(capacity).0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
    }

    /// Fire and forget: no subscribers (or a lagging one) never fails a stage.
    pub fn emit(&self, e: Event) {
        let _ = self.0.send(e);
    }
}
