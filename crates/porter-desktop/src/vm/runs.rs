//! The Runs section: stored run history per stage, and the live log of the current run.

use std::collections::VecDeque;

use porter_engine::events::Event;
use porter_engine::store::RunRow;
use porter_model::Stage;

use super::pipeline::{name, run_state, state_text};

/// Runs per stage, newest first.
pub type History = Vec<(Stage, Vec<RunRow>)>;

/// The live log keeps the newest lines only.
pub const LOG_CAP: usize = 2000;

#[derive(Default)]
pub struct Runs {
    log: VecDeque<String>,
    /// Runs per stage, newest first; None until the first query lands.
    pub history: Option<Result<History, String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow {
    pub started_at: String,
    pub ended_at: String,
    /// "clean", "errors", "clean (dry run)", "interrupted".
    pub outcome: String,
    pub counts: String,
}

impl Runs {
    pub fn push(&mut self, line: String) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    pub fn log(&self) -> &VecDeque<String> {
        &self.log
    }

    /// A new run starts a fresh log.
    pub fn start_run(&mut self, what: String) {
        self.log.clear();
        self.push(what);
    }

    /// The events worth a log line (progress is shown on the strip, not here).
    pub fn on_event(&mut self, e: &Event) {
        match e {
            Event::StageStarted { stage, .. } => self.push(format!("{} started", name(*stage))),
            Event::IssueOpened { issue } => self.push(format!(
                "{:?} {}: {}",
                issue.severity,
                name(issue.stage),
                issue.message
            )),
            Event::StageFinished { stage, summary, .. } => self.push(format!(
                "{} finished: {} done, {} failed, {} errors, {} warnings",
                name(*stage),
                summary.done,
                summary.failed,
                summary.errors,
                summary.warnings
            )),
            Event::Progress { .. } | Event::ItemResult { .. } => {}
        }
    }
}

pub fn history_row(stage: Stage, r: &RunRow) -> HistoryRow {
    let state = state_text(&run_state(stage, r)).to_string();
    HistoryRow {
        started_at: r.started_at.clone(),
        ended_at: r.ended_at.clone().unwrap_or_else(|| "—".into()),
        outcome: if r.dry_run {
            format!("{state} (dry run)")
        } else {
            state
        },
        counts: r.summary.as_ref().map_or_else(String::new, |s| {
            format!(
                "{} done · {} failed · {} errors · {} warnings",
                s.done, s.failed, s.errors, s.warnings
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_bounded() {
        let mut r = Runs::default();
        for i in 0..LOG_CAP + 5 {
            r.push(format!("line {i}"));
        }
        assert_eq!(r.log().len(), LOG_CAP);
        assert_eq!(r.log().front().unwrap(), "line 5");
        assert_eq!(r.log().back().unwrap(), &format!("line {}", LOG_CAP + 4));
        r.start_run("load started".into());
        assert_eq!(r.log().len(), 1);
    }

    #[test]
    fn history_row_labels_dry_runs() {
        let r = RunRow {
            id: "r".into(),
            porter_version: "2.0.0".into(),
            started_at: "t0".into(),
            ended_at: Some("t1".into()),
            summary: Some(Default::default()),
            dry_run: true,
        };
        assert_eq!(history_row(Stage::Load, &r).outcome, "clean (dry run)");
    }

    #[test]
    fn dry_run_history_never_reads_as_a_written_success() {
        let r = RunRow {
            id: "r".into(),
            porter_version: "2.0.0".into(),
            started_at: "t0".into(),
            ended_at: Some("t1".into()),
            summary: Some(Default::default()),
            dry_run: true,
        };
        // The history row is plain text with no tone: it carries no green or ✓, and the
        // outcome always says "(dry run)".
        let h = history_row(Stage::Load, &r);
        assert!(h.outcome.ends_with("(dry run)") && !h.outcome.contains('✓'));
    }
}
