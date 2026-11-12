use serde::{Deserialize, Serialize};

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
