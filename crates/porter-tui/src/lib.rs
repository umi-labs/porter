#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod actions;
mod app;
mod keymap;
mod lists;
#[cfg(test)]
mod lists_tests;
mod msg;
mod runtime;
mod setup;
mod term;
mod ui;

pub use runtime::run_tui;
