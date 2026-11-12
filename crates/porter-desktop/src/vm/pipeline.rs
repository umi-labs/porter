//! The pipeline strip: one row per stage, from the stored overview plus the live events
//! of the run in flight. Same rules as the terminal UI; the stored state always comes
//! from `session::stage_state`.

use porter_engine::events::{Event, Summary};
use porter_engine::session::{Overview, RunOutcome, RunPlan, STAGES, StageState, stage_state};
use porter_engine::store::RunRow;
use porter_model::{Issue, Severity, Stage};

pub type Progress = (u64, Option<u64>, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageRow {
    pub stage: Stage,
    pub name: String,
    pub state: StageState,
    pub glyph: &'static str,
    pub tone: Tone,
    /// "running 3/10 items", "clean (dry run)", "could not run: …".
    pub status: String,
    /// When the shown run started ("this session" for one that just ended).
    pub last_run: Option<String>,
    pub summary: Option<Summary>,
}

/// How long a closing window waits for a cancelled run to reach a safe point.
pub const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseStep {
    /// Nothing is running: close now.
    Close,
    /// A load is writing to the destination: ask before cancelling it.
    ConfirmApply,
    /// Cancel the run and keep the window open until it stops (or `CLOSE_WAIT` passes).
    CancelAndWait,
    /// Already cancelling: keep waiting.
    Wait,
}

#[derive(Default)]
pub struct Pipeline {
    pub overview: Option<Overview>,
    /// The run in flight (or refused); None when idle.
    plan: Option<RunPlan>,
    pub cancelling: bool,
    /// The stage in flight.
    running: Option<Stage>,
    progress: [Option<Progress>; 6],
    could_not_run: [Option<String>; 6],
    /// How a stage that just finished ended, until the overview re-query lands.
    finished: [Option<Summary>; 6],
    pending_overview: u32,
    /// The run in flight (or just ended) loads without apply.
    dry_load: bool,
}

pub fn name(s: Stage) -> String {
    format!("{s:?}").to_lowercase()
}

fn idx(s: Stage) -> usize {
    STAGES.iter().position(|x| *x == s).unwrap_or(0)
}

pub fn state_text(s: &StageState) -> &'static str {
    match s {
        StageState::Never => "not run",
        StageState::Running => "running",
        StageState::Clean => "clean",
        StageState::Warnings => "warnings",
        StageState::Errors => "errors",
        StageState::Interrupted => "interrupted",
        StageState::CouldNotRun(_) => "could not run",
    }
}

pub fn glyph(s: &StageState) -> (&'static str, Tone) {
    match s {
        StageState::Never => ("·", Tone::Muted),
        StageState::Running => ("●", Tone::Accent),
        StageState::Clean => ("✓", Tone::Success),
        StageState::Warnings => ("⚠", Tone::Warning),
        StageState::Errors => ("✗", Tone::Danger),
        StageState::Interrupted => ("⏸", Tone::Warning),
        StageState::CouldNotRun(_) => ("‼", Tone::Danger),
    }
}

/// The state a single run amounts to, by the shared `stage_state` rules.
pub fn run_state(stage: Stage, run: &RunRow) -> StageState {
    let one = Overview {
        last_runs: vec![(stage, run.clone())],
        ..Default::default()
    };
    stage_state(&one, stage)
}

/// A summary the engine just reported, judged like a stored run.
fn summary_state(stage: Stage, summary: &Summary) -> StageState {
    run_state(
        stage,
        &RunRow {
            id: String::new(),
            porter_version: String::new(),
            started_at: String::new(),
            ended_at: None,
            summary: Some(summary.clone()),
            dry_run: false,
        },
    )
}

impl Pipeline {
    pub fn busy(&self) -> bool {
        self.plan.is_some()
    }

    pub fn running_stage(&self) -> Option<Stage> {
        self.running
    }

    /// Marks `plan` as started. Refused (with why) while another run is in flight.
    pub fn start(&mut self, plan: &RunPlan) -> Result<(), String> {
        if self.busy() {
            return Err("a stage is already running".into());
        }
        self.cancelling = false;
        self.dry_load = match plan {
            RunPlan::Stage(_, o) => !o.apply,
            RunPlan::All { apply, .. } => !apply,
        };
        if let RunPlan::Stage(s, _) = plan {
            let i = idx(*s);
            self.running = Some(*s);
            self.could_not_run[i] = None;
            self.finished[i] = None;
            self.progress[i] = None;
        }
        self.plan = Some(plan.clone());
        Ok(())
    }

    /// True when a cancel should be sent now.
    pub fn cancel(&mut self) -> bool {
        if !self.busy() || self.cancelling {
            return false;
        }
        self.cancelling = true;
        true
    }

    pub fn on_event(&mut self, e: &Event) {
        match e {
            Event::StageStarted { stage, .. } => {
                let i = idx(*stage);
                self.running = Some(*stage);
                self.progress[i] = None;
                self.could_not_run[i] = None;
                self.finished[i] = None;
            }
            Event::Progress {
                stage,
                done,
                total,
                label,
            } => self.progress[idx(*stage)] = Some((*done, *total, label.clone())),
            Event::StageFinished { stage, summary, .. } => {
                self.progress[idx(*stage)] = None;
                if self.running == Some(*stage) {
                    self.running = None;
                }
                self.finished[idx(*stage)] = Some(summary.clone());
            }
            Event::ItemResult { .. } | Event::IssueOpened { .. } => {}
        }
    }

    /// Ends the run; returns the lines for the run log (TUI parity).
    pub fn on_run_done(&mut self, o: RunOutcome) -> Vec<String> {
        let plan = self.plan.take();
        self.cancelling = false;
        self.running = None;
        self.progress = Default::default();
        for (stage, sum) in &o.summaries {
            self.finished[idx(*stage)] = Some(sum.clone());
        }
        let mut failures = 0;
        for (stage, code, err) in &o.per_stage {
            if *code == 1 {
                failures += 1;
                self.could_not_run[idx(*stage)] =
                    Some(err.clone().unwrap_or_else(|| "could not run".into()));
            }
        }
        // The session refused before any stage ran: the messages are the reason.
        if let (0, 1, Some(RunPlan::Stage(stage, _))) = (failures, o.code, &plan) {
            self.could_not_run[idx(*stage)] = Some(o.messages.join("; "));
        }
        let mut lines = vec![];
        for (stage, code, err) in o.per_stage {
            if let (3, Some(e)) = (code, err) {
                lines.push(format!("{} interrupted: {e}", name(stage)));
            }
        }
        lines.extend(o.messages);
        if o.seeded {
            lines.push("mapping seeded in porter.toml".into());
        }
        if let Some((stage, why)) = o.skipped {
            lines.push(format!("{} skipped: {why}", name(stage)));
        }
        lines
    }

    /// An overview query was sent.
    pub fn refresh_requested(&mut self) {
        self.pending_overview += 1;
    }

    /// An overview query came back. Returns an error line for the log, if it failed.
    pub fn overview_landed(&mut self, o: Result<Overview, String>) -> Option<String> {
        self.pending_overview = self.pending_overview.saturating_sub(1);
        // The stored overview is now current; the live finished state can go once no
        // query is left (an older answer may predate the run).
        if self.pending_overview == 0 {
            self.finished = Default::default();
        }
        match o {
            Ok(o) => {
                self.overview = Some(o);
                None
            }
            Err(e) => Some(format!("error: {e}")),
        }
    }

    /// Live states (running, could not run, just finished) win over the stored run.
    /// What closing the window should do now. `confirmed`: the user already agreed to
    /// cancel a writing load; `closing`: a cancel-and-wait is already under way.
    pub fn close_step(&self, confirmed: bool, closing: bool) -> CloseStep {
        let writing = match &self.plan {
            Some(RunPlan::Stage(_, o)) => o.apply,
            Some(RunPlan::All { apply, .. }) => *apply,
            None => return CloseStep::Close,
        };
        if closing {
            CloseStep::Wait
        } else if writing && !confirmed {
            CloseStep::ConfirmApply
        } else {
            CloseStep::CancelAndWait
        }
    }

    pub fn rows(&self) -> Vec<StageRow> {
        let empty = Overview::default();
        let ov = self.overview.as_ref().unwrap_or(&empty);
        STAGES
            .iter()
            .map(|&stage| {
                let i = idx(stage);
                let fin = self.finished[i].as_ref();
                let stored = ov
                    .last_runs
                    .iter()
                    .find(|(s, _)| *s == stage)
                    .map(|(_, r)| r);
                let state = if self.running == Some(stage) {
                    StageState::Running
                } else if let Some(e) = &self.could_not_run[i] {
                    StageState::CouldNotRun(e.clone())
                } else if let Some(sum) = fin {
                    summary_state(stage, sum)
                } else {
                    stage_state(ov, stage)
                };
                let dry = match fin {
                    Some(_) => stage == Stage::Load && self.dry_load,
                    None => stored.is_some_and(|r| r.dry_run),
                };
                let status = match (&self.progress[i], &state) {
                    (Some((done, Some(total), label)), _) => {
                        format!("running {done}/{total} {label}")
                    }
                    (Some((done, None, label)), _) => format!("running {done} {label}"),
                    (None, StageState::Running) if self.cancelling => "cancelling…".into(),
                    (None, StageState::Running) => "starting…".into(),
                    (None, StageState::CouldNotRun(e)) => format!("could not run: {e}"),
                    (None, StageState::Clean) if dry => "dry run — nothing written".into(),
                    (None, s) if dry => format!("{} (dry run)", state_text(s)),
                    (None, s) => state_text(s).into(),
                };
                // A clean dry run wrote nothing: it must not look like a finished load.
                let (glyph, tone) = match &state {
                    StageState::Clean if dry => ("◌", Tone::Muted),
                    s => glyph(s),
                };
                StageRow {
                    stage,
                    name: name(stage),
                    glyph,
                    tone,
                    status,
                    last_run: match fin {
                        Some(_) => Some("this session".into()),
                        None => stored.map(|r| r.started_at.clone()),
                    },
                    summary: fin
                        .cloned()
                        .or_else(|| stored.and_then(|r| r.summary.clone())),
                    state,
                }
            })
            .collect()
    }
}

/// Open issues per kind with the worst severity in each, most severe kinds first.
#[derive(Clone, Debug, PartialEq)]
pub struct KindCount {
    pub kind: String,
    pub total: u64,
    pub worst: Severity,
}

pub fn issues_by_kind(issues: &[Issue]) -> Vec<KindCount> {
    let rank = |s: Severity| match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    };
    let mut out: Vec<KindCount> = vec![];
    for i in issues {
        let kind = serde_json::to_value(i.kind)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        match out.iter_mut().find(|k| k.kind == kind) {
            Some(k) => {
                k.total += 1;
                if rank(i.severity) < rank(k.worst) {
                    k.worst = i.severity;
                }
            }
            None => out.push(KindCount {
                kind,
                total: 1,
                worst: i.severity,
            }),
        }
    }
    out.sort_by(|a, b| {
        (rank(a.worst), std::cmp::Reverse(a.total), &a.kind).cmp(&(
            rank(b.worst),
            std::cmp::Reverse(b.total),
            &b.kind,
        ))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_engine::session::StageOpts;
    use porter_engine::stage::SchemaMode;

    fn run(summary: Option<Summary>, dry_run: bool) -> RunRow {
        RunRow {
            id: "r".into(),
            porter_version: "2.0.0".into(),
            started_at: "2026-11-12T10:00:00Z".into(),
            ended_at: summary.as_ref().map(|_| "2026-11-12T10:01:00Z".into()),
            summary,
            dry_run,
        }
    }

    fn sum(done: u64, failed: u64, errors: u64, warnings: u64) -> Summary {
        Summary {
            done,
            failed,
            errors,
            warnings,
            ..Default::default()
        }
    }

    fn stage_plan(s: Stage) -> RunPlan {
        RunPlan::Stage(
            s,
            StageOpts {
                refresh: false,
                apply: false,
                schema_mode: SchemaMode::Check,
            },
        )
    }

    fn row(p: &Pipeline, s: Stage) -> StageRow {
        p.rows().into_iter().find(|r| r.stage == s).unwrap()
    }

    #[test]
    fn pipeline_states() {
        let mut p = Pipeline::default();
        p.refresh_requested();
        let ov = Overview {
            last_runs: vec![
                (Stage::Analyse, run(Some(sum(5, 0, 0, 0)), false)),
                (Stage::Extract, run(Some(sum(5, 0, 0, 2)), false)),
                // Error issues with nothing failed: never green.
                (Stage::Normalise, run(Some(sum(5, 0, 1, 0)), false)),
                // Started, never ended.
                (Stage::Schema, run(None, false)),
                (Stage::Load, run(Some(sum(5, 0, 0, 0)), true)),
            ],
            ..Default::default()
        };
        assert_eq!(p.overview_landed(Ok(ov)), None);
        let rows = p.rows();
        assert_eq!(rows.len(), 6);
        let states: Vec<_> = rows.iter().map(|r| r.state.clone()).collect();
        assert_eq!(
            states,
            [
                StageState::Clean,
                StageState::Warnings,
                StageState::Errors,
                StageState::Interrupted,
                StageState::Clean,
                StageState::Never,
            ]
        );
        assert_eq!(rows[0].glyph, "✓");
        assert_eq!(rows[0].tone, Tone::Success);
        assert_eq!(rows[2].tone, Tone::Danger);
        assert_ne!(rows[2].glyph, "✓");
        // A clean dry-run load wrote nothing: never the green "written" look.
        assert_eq!(rows[4].state, StageState::Clean);
        assert_ne!(rows[4].tone, Tone::Success);
        assert_ne!(rows[4].glyph, "✓");
        assert_eq!(rows[4].status, "dry run — nothing written");
        assert_eq!(rows[5].status, "not run");
        // The shared rule for a single run agrees.
        assert_eq!(
            run_state(Stage::Normalise, &run(Some(sum(1, 0, 3, 0)), false)),
            StageState::Errors
        );

        // A run: the row flips at once, shows progress, and a second run is refused.
        p.start(&stage_plan(Stage::Normalise)).unwrap();
        assert_eq!(row(&p, Stage::Normalise).state, StageState::Running);
        assert!(p.start(&stage_plan(Stage::Extract)).is_err());
        p.on_event(&Event::Progress {
            stage: Stage::Normalise,
            done: 3,
            total: Some(10),
            label: "items".into(),
        });
        assert_eq!(row(&p, Stage::Normalise).status, "running 3/10 items");
        // It finishes with error issues and no failed items: red, not green, before the
        // overview re-query has landed.
        p.on_event(&Event::StageFinished {
            stage: Stage::Normalise,
            run: porter_engine::store::RunId(uuid::Uuid::nil()),
            summary: sum(5, 0, 2, 0),
        });
        p.refresh_requested();
        let lines = p.on_run_done(RunOutcome {
            code: 2,
            summaries: vec![(Stage::Normalise, sum(5, 0, 2, 0))],
            seeded: true,
            skipped: Some((Stage::Verify, "dry run".into())),
            ..Default::default()
        });
        assert!(!p.busy());
        assert_eq!(row(&p, Stage::Normalise).state, StageState::Errors);
        assert!(lines.contains(&"mapping seeded in porter.toml".to_string()));
        assert!(lines.contains(&"verify skipped: dry run".to_string()));

        // A run the session refused: the stage says why, verbatim.
        p.start(&stage_plan(Stage::Extract)).unwrap();
        p.on_run_done(RunOutcome {
            code: 1,
            messages: vec!["porter.toml: bad".into()],
            ..Default::default()
        });
        assert_eq!(
            row(&p, Stage::Extract).state,
            StageState::CouldNotRun("porter.toml: bad".into())
        );
        assert_eq!(row(&p, Stage::Extract).glyph, "‼");
    }

    #[test]
    fn issues_grouped_by_kind() {
        use porter_model::{IssueKind, Resolution};
        let mk = |kind, severity| Issue {
            id: uuid::Uuid::nil(),
            stage: Stage::Normalise,
            kind,
            severity,
            item: None,
            message: "m".into(),
            details: serde_json::Value::Null,
            resolution: Resolution::Open,
        };
        let g = issues_by_kind(&[
            mk(IssueKind::MissingMedia, Severity::Warning),
            mk(IssueKind::UnresolvedLink, Severity::Info),
            mk(IssueKind::UnresolvedLink, Severity::Error),
            mk(IssueKind::MissingMedia, Severity::Warning),
        ]);
        assert_eq!(
            g,
            [
                KindCount {
                    kind: "unresolved-link".into(),
                    total: 2,
                    worst: Severity::Error
                },
                KindCount {
                    kind: "missing-media".into(),
                    total: 2,
                    worst: Severity::Warning
                },
            ]
        );
    }

    #[test]
    fn close_waits_for_a_safe_stop() {
        let mut p = Pipeline::default();
        assert_eq!(p.close_step(false, false), CloseStep::Close);

        p.start(&stage_plan(Stage::Load)).unwrap();
        assert_eq!(p.close_step(false, false), CloseStep::CancelAndWait);
        assert_eq!(p.close_step(false, true), CloseStep::Wait);
        p.on_run_done(RunOutcome::default());
        assert_eq!(p.close_step(false, true), CloseStep::Close);

        let apply = RunPlan::Stage(
            Stage::Load,
            StageOpts {
                refresh: false,
                apply: true,
                schema_mode: SchemaMode::Check,
            },
        );
        p.start(&apply).unwrap();
        assert_eq!(p.close_step(false, false), CloseStep::ConfirmApply);
        assert_eq!(p.close_step(true, false), CloseStep::CancelAndWait);
        assert_eq!(p.close_step(true, true), CloseStep::Wait);
        p.on_run_done(RunOutcome::default());

        // Run all with apply writes too.
        p.start(&RunPlan::All {
            keep_going: false,
            apply: true,
        })
        .unwrap();
        assert_eq!(p.close_step(false, false), CloseStep::ConfirmApply);
    }
}
