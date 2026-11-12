#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
// Skeleton: panels, run keys and editor/report/copy commands land in Tasks 3-7.
#![allow(dead_code)]

mod app;
mod msg;
mod runtime;
mod term;
mod ui;

pub use runtime::run_tui;
