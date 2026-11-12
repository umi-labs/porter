//! The pipeline strip: one row per stage, from the stored overview plus the live events
//! of the run in flight. Same rules as the terminal UI; the stored state and staleness
//! always come from `session::stage_view`.

use porter_engine::events::{Event, Summary};
use porter_engine::session::{
    Overview, RunOutcome, RunPlan, STAGES, StageState, finished_state, stage_view,
};
use porter_model::{Issue, Severity, Stage};

pub type Progress = (u64, Option<u64>, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Success,
    Warning,
    Danger,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageRow {
    pub stage: Stage,
    pub name: String,
    pub state: StageState,
    /// The chip's words: "12 errors", "Running · 62%", "Dry run · nothing written".
    pub label: String,
    /// The line under the counts: progress, the engine's error verbatim, "Loaded, not
    /// yet verified". Empty when the chip says it all.
    pub status: String,
    /// Why the shown result no longer holds. The card keeps it, dimmed, with ↻ and this.
    pub stale: Option<String>,
    /// When the shown run started ("this session" for one that just ended).
    pub last_run: Option<String>,
    pub summary: Option<Summary>,
    /// The shown load wrote nothing.
    pub dry_run: bool,
}

impl StageRow {
    /// The run's counts; a dry run's are what it would write.
    pub fn counts(&self) -> Option<String> {
        let s = self.summary.as_ref()?;
        Some(if self.dry_run {
            format!("would write {} · {} failed · 0 written", s.done, s.failed)
        } else {
            format!(
                "{} done · {} failed · {} errors · {} warnings",
                s.done, s.failed, s.errors, s.warnings
            )
        })
    }
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
        StageState::DryRun => "dry run",
        StageState::Skipped => "skipped",
    }
}

/// The chip's words for a state.
fn chip_label(
    stage: Stage,
    state: &StageState,
    sum: Option<&Summary>,
    p: Option<&Progress>,
) -> String {
    let n = |f: fn(&Summary) -> u64| sum.map_or(0, f);
    match state {
        StageState::Running => match p {
            Some((done, Some(total), _)) if *total > 0 => {
                format!("Running · {}%", done.saturating_mul(100) / total)
            }
            _ => "Running".into(),
        },
        StageState::Clean if stage == Stage::Load => "Clean · written".into(),
        StageState::Clean => "Clean".into(),
        StageState::Warnings => match n(|s| s.warnings) {
            1 => "Done · 1 warning".into(),
            w => format!("Done · {w} warnings"),
        },
        StageState::Errors => match (n(|s| s.errors), n(|s| s.failed)) {
            (0, f) => format!("{f} failed"),
            (1, _) => "1 error".into(),
            (e, _) => format!("{e} errors"),
        },
        StageState::DryRun => "Dry run · nothing written".into(),
        StageState::CouldNotRun(_) => "Failed".into(),
        StageState::Interrupted => "Interrupted".into(),
        StageState::Never => "Not run".into(),
        StageState::Skipped => "Skipped".into(),
    }
}

impl Pipeline {
    pub fn busy(&self) -> bool {
        self.plan.is_some()
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

    /// Cmd-Q: true when a cancel should go out before the app exits. GPUI gives quit
    /// handlers 100 ms and cannot ask a question, so a writing load is cancelled without
    /// the confirmation (it stops at a safe point and can be resumed); this is as much
    /// of `close_step` as quit can honour.
    pub fn cancel_on_quit(&self) -> bool {
        self.close_step(true, self.cancelling) == CloseStep::CancelAndWait
    }

    /// A load that wrote is only half the story until a verify run comes after it.
    fn verified(&self, ov: &Overview) -> bool {
        if self.finished[idx(Stage::Load)].is_some() {
            return self.finished[idx(Stage::Verify)].is_some();
        }
        let id = |s: Stage| {
            ov.last_runs
                .iter()
                .find(|(x, _)| *x == s)
                .map(|(_, r)| r.id.as_str())
        };
        matches!((id(Stage::Load), id(Stage::Verify)), (Some(l), Some(v)) if v > l)
    }

    /// Live states (running, could not run, just finished) win over the stored run.
    pub fn rows(&self) -> Vec<StageRow> {
        let empty = Overview::default();
        let ov = self.overview.as_ref().unwrap_or(&empty);
        let verified = self.verified(ov);
        STAGES
            .iter()
            .map(|&stage| {
                let i = idx(stage);
                let fin = self.finished[i].as_ref();
                let view = stage_view(ov, stage);
                let stored = view.last_run.as_ref();
                let dry = match fin {
                    Some(_) => stage == Stage::Load && self.dry_load,
                    None => stored.is_some_and(|r| r.dry_run),
                };
                let mut stale = None;
                let state = if self.running == Some(stage) {
                    StageState::Running
                } else if let Some(e) = &self.could_not_run[i] {
                    StageState::CouldNotRun(e.clone())
                } else if let Some(sum) = fin {
                    finished_state(sum, dry)
                } else {
                    // An upstream stage running now: the stored result is about to be old.
                    let live = self
                        .running
                        .filter(|r| idx(*r) < i && !(*r == Stage::Load && self.dry_load));
                    stale = match live {
                        Some(up) if stored.is_some() => Some(format!("{} is re-running", name(up))),
                        _ => view.stale.clone(),
                    };
                    view.state.clone()
                };
                let summary = fin
                    .cloned()
                    .or_else(|| stored.and_then(|r| r.summary.clone()));
                let progress = self.progress[i].as_ref();
                let status = match (progress, &state) {
                    (Some((done, Some(total), label)), _) => {
                        format!("running {done}/{total} {label}")
                    }
                    (Some((done, None, label)), _) => format!("running {done} {label}"),
                    (None, StageState::Running) if self.cancelling => "cancelling…".into(),
                    (None, StageState::Running) => "starting…".into(),
                    (None, StageState::CouldNotRun(e)) => e.clone(),
                    (None, StageState::DryRun) => "nothing written to the destination".into(),
                    (None, StageState::Skipped) => "the last load was a dry run".into(),
                    (None, StageState::Interrupted) => "Resume skips what is already done".into(),
                    (None, StageState::Errors) if dry => "dry run · nothing written".into(),
                    (None, StageState::Clean | StageState::Warnings)
                        if stage == Stage::Load && !verified =>
                    {
                        "Loaded, not yet verified".into()
                    }
                    (None, _) => String::new(),
                };
                StageRow {
                    stage,
                    name: name(stage),
                    label: chip_label(stage, &state, summary.as_ref(), progress),
                    status,
                    stale,
                    last_run: match fin {
                        Some(_) => Some("this session".into()),
                        None => stored.map(|r| r.started_at.clone()),
                    },
                    summary,
                    dry_run: dry,
                    state,
                }
            })
            .collect()
    }
}

/// A run time for a card: "just now", "5 min ago", "2 h ago", or the date once it is a day old.
/// Anything that does not parse is shown as it came.
pub fn relative_time(iso: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let s = (now - at.with_timezone(&chrono::Utc)).num_seconds();
    match s {
        ..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => at.format("%-d %b %Y").to_string(),
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
    use porter_engine::store::RunRow;

    #[test]
    fn relative_times() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z")
            .unwrap()
            .to_utc();
        let t = |iso| relative_time(iso, now);
        assert_eq!(t("2026-11-12T11:59:30.040Z"), "just now");
        assert_eq!(t("2026-11-12T12:05:00Z"), "just now", "clock skew");
        assert_eq!(t("2026-11-12T11:58:03.040Z"), "1 min ago");
        assert_eq!(t("2026-11-12T09:00:00Z"), "3 h ago");
        assert_eq!(t("2026-11-11T11:00:00Z"), "11 Nov 2026");
        assert_eq!(t("garbage"), "garbage");
    }

    fn run(summary: Option<Summary>, dry_run: bool) -> RunRow {
        RunRow {
            id: "r".into(),
            porter_version: "2.0.0".into(),
            started_at: "2026-11-12T10:00:00Z".into(),
            ended_at: summary.as_ref().map(|_| "2026-11-12T10:01:00Z".into()),
            summary,
            dry_run,
            config_fp: None,
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
                StageState::DryRun,
                // Verify after a dry-run load: not run on purpose.
                StageState::Skipped,
            ]
        );
        assert_eq!(rows[0].label, "Clean");
        assert_eq!(rows[1].label, "Done · 2 warnings");
        // Error issues with nothing failed: never green.
        assert_eq!(rows[2].label, "1 error");
        assert_eq!(rows[3].status, "Resume skips what is already done");
        // A clean dry-run load wrote nothing: never the green "written" look.
        assert_eq!(rows[4].label, "Dry run · nothing written");
        assert_eq!(rows[4].status, "nothing written to the destination");
        assert_eq!(
            rows[4].counts().as_deref(),
            Some("would write 5 · 0 failed · 0 written")
        );
        assert_eq!(rows[5].label, "Skipped");
        assert_eq!(rows[5].status, "the last load was a dry run");
        assert!(rows.iter().all(|r| r.stale.is_none()));
        // The shared rule for a single run agrees.
        assert_eq!(
            porter_engine::session::run_state(&run(Some(sum(1, 0, 3, 0)), false)),
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
        assert_eq!(row(&p, Stage::Normalise).label, "Running · 30%");
        // Downstream results are about to be old: dimmed, with why.
        assert_eq!(
            row(&p, Stage::Schema).stale.as_deref(),
            Some("normalise is re-running")
        );
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
        assert_eq!(row(&p, Stage::Extract).label, "Failed");
        assert_eq!(row(&p, Stage::Extract).status, "porter.toml: bad");
    }

    fn with_id(id: &str, r: RunRow) -> RunRow {
        RunRow { id: id.into(), ..r }
    }

    #[test]
    fn stale_keeps_previous_counts() {
        let mut p = Pipeline::default();
        p.overview_landed(Ok(Overview {
            last_runs: vec![
                (
                    Stage::Extract,
                    with_id("2", run(Some(sum(9, 0, 0, 0)), false)),
                ),
                (
                    Stage::Normalise,
                    with_id("1", run(Some(sum(5, 2, 3, 0)), false)),
                ),
            ],
            ..Default::default()
        }));
        let n = row(&p, Stage::Normalise);
        // The old result, not a new state: same chip, same counts, plus why.
        assert_eq!(n.state, StageState::Errors);
        assert_eq!(n.label, "3 errors");
        assert_eq!(n.stale.as_deref(), Some("extract re-ran since"));
        assert_eq!(
            n.counts().as_deref(),
            Some("5 done · 2 failed · 3 errors · 0 warnings")
        );
        assert_eq!(row(&p, Stage::Extract).stale, None);
    }

    #[test]
    fn loaded_not_verified_label() {
        let mut p = Pipeline::default();
        let applied = |verify: Option<&str>| {
            let mut last_runs =
                vec![(Stage::Load, with_id("5", run(Some(sum(4, 0, 0, 0)), false)))];
            if let Some(id) = verify {
                last_runs.push((
                    Stage::Verify,
                    with_id(id, run(Some(sum(4, 0, 0, 0)), false)),
                ));
            }
            Overview {
                last_runs,
                ..Default::default()
            }
        };
        p.overview_landed(Ok(applied(None)));
        let l = row(&p, Stage::Load);
        assert_eq!(l.label, "Clean · written");
        assert_eq!(l.status, "Loaded, not yet verified");
        // A verify from before this load does not count.
        p.overview_landed(Ok(applied(Some("4"))));
        assert_eq!(row(&p, Stage::Load).status, "Loaded, not yet verified");
        p.overview_landed(Ok(applied(Some("6"))));
        assert_eq!(row(&p, Stage::Load).status, "");
        // A dry run never says loaded.
        p.overview_landed(Ok(Overview {
            last_runs: vec![(Stage::Load, run(Some(sum(4, 0, 0, 0)), true))],
            ..Default::default()
        }));
        assert_ne!(row(&p, Stage::Load).status, "Loaded, not yet verified");
        assert_eq!(row(&p, Stage::Verify).state, StageState::Skipped);
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

    #[test]
    fn quit_cancels_any_run_once() {
        let mut p = Pipeline::default();
        assert!(!p.cancel_on_quit());
        p.start(&RunPlan::All {
            keep_going: false,
            apply: true,
        })
        .unwrap();
        // A writing load too: quit cannot ask, so it cancels (resumable).
        assert!(p.cancel_on_quit());
        assert!(p.cancel());
        assert!(!p.cancel_on_quit());
    }
}
