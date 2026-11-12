use crate::lists::Group;
use crossterm::event::KeyEvent;
use porter_engine::events::Event;
use porter_engine::session::{
    IssueQuery, ItemDetail, ItemQuery, ItemRow, Overview, Page, Paged, RunOutcome, RunPlan,
};
use porter_model::{Issue, ItemId, Resolution};
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

pub enum Msg {
    Key(KeyEvent),
    Resize(u16, u16),
    Engine(Event),
    /// The engine event channel overflowed and this many events never arrived.
    Lagged(u64),
    Data(DataMsg),
    Tick,
    RunDone(RunOutcome),
}

/// The last dry run as the apply prompt needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewInfo {
    pub counts: BTreeMap<String, u64>,
    pub at: std::time::SystemTime,
    /// Why it can no longer be trusted, if so.
    pub stale: Option<String>,
}

/// Results of store queries the runtime ran off the render loop. `seq` ties a page
/// back to the query that asked for it, so answers to a replaced query are dropped.
pub enum DataMsg {
    Overview(Box<Overview>),
    OverviewFailed(String),
    Error(String),
    Items {
        seq: u64,
        page: Page,
        paged: Paged<ItemRow>,
    },
    IssueGroups {
        seq: u64,
        groups: Vec<Group>,
    },
    IssuePage {
        seq: u64,
        page: Page,
        paged: Paged<Issue>,
    },
    Item(Box<ItemDetail>),
    Resolved(Result<(Uuid, Resolution), String>),
    /// Items per collection in the last dry-run preview; None when there was none.
    Preview(Option<PreviewInfo>),
    /// What reloading porter.toml after an edit found ("config OK" when nothing).
    Config(Vec<String>),
    /// A report was written and opened (its path), or why not, verbatim.
    Reported(Result<PathBuf, String>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RefreshScope {
    All,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Run(RunPlan),
    Cancel,
    Refresh(RefreshScope),
    Items {
        q: ItemQuery,
        page: Page,
        seq: u64,
    },
    Issues {
        q: IssueQuery,
        page: Page,
        seq: u64,
    },
    /// Counts per issue kind under the given filter mode.
    IssueGroups {
        mode: crate::lists::Mode,
        seq: u64,
    },
    Item(ItemId),
    Resolve(Uuid, Resolution),
    /// Read the dry-run preview counts the apply confirmation shows.
    PreviewCounts,
    OpenEditor,
    OpenReport(String),
    Copy(String),
    Quit,
}
