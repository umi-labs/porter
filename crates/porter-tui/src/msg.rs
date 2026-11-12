use crossterm::event::KeyEvent;
use porter_engine::events::Event;
use porter_engine::session::{Overview, RunOutcome, RunPlan};

pub enum Msg {
    Key(KeyEvent),
    Resize(u16, u16),
    Engine(Event),
    Data(DataMsg),
    Tick,
    RunDone(RunOutcome),
}

/// Results of store queries the runtime ran off the render loop.
pub enum DataMsg {
    Overview(Box<Overview>),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RefreshScope {
    All,
}

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Run(RunPlan),
    Cancel,
    Refresh(RefreshScope),
    OpenEditor,
    OpenReport(String),
    Copy(String),
    Quit,
}
