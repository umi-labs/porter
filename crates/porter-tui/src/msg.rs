use crate::lists::Group;
use crossterm::event::KeyEvent;
use porter_engine::events::Event;
use porter_engine::session::{
    IssueQuery, ItemDetail, ItemQuery, ItemRow, Overview, Page, Paged, RunOutcome, RunPlan,
};
use porter_model::{Issue, ItemId, Resolution};
use uuid::Uuid;

pub enum Msg {
    Key(KeyEvent),
    Resize(u16, u16),
    Engine(Event),
    Data(DataMsg),
    Tick,
    RunDone(RunOutcome),
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
    OpenEditor,
    OpenReport(String),
    Copy(String),
    Quit,
}
