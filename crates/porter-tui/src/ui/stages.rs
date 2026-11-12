use super::{accent, panel, tint};
use crate::app::{App, Panel, Progress};
use porter_engine::events::{Summary, exit_code};
use porter_engine::session::{Overview, STAGES, StageState, stage_state};
use porter_engine::store::RunRow;
use porter_model::Stage;
use ratatui::{
    Frame,
    layout::Rect,
    style::Color,
    text::{Line, Span},
    widgets::Paragraph,
};

#[derive(Clone, Debug, PartialEq)]
pub struct RunSummaryView {
    pub summary: Option<Summary>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub porter_version: String,
    /// A load that wrote nothing.
    pub dry_run: bool,
}

impl From<&RunRow> for RunSummaryView {
    fn from(r: &RunRow) -> Self {
        RunSummaryView {
            summary: r.summary.clone(),
            started_at: r.started_at.clone(),
            ended_at: r.ended_at.clone(),
            porter_version: r.porter_version.clone(),
            dry_run: r.dry_run,
        }
    }
}

pub struct StageRow {
    pub stage: Stage,
    pub state: StageState,
    pub last_run: Option<RunSummaryView>,
    pub progress: Option<Progress>,
}

pub fn name(s: Stage) -> String {
    format!("{s:?}").to_lowercase()
}

/// Live states (running, could not run) win over the stored last run.
pub fn rows(app: &App) -> Vec<StageRow> {
    let empty = Overview::default();
    let ov = app.panels.overview.as_ref().unwrap_or(&empty);
    let st = &app.panels.stages;
    STAGES
        .iter()
        .map(|&stage| {
            let fin = st.finished_of(stage);
            let state = if st.running == Some(stage) {
                StageState::Running
            } else if let Some(e) = st.could_not_run_of(stage) {
                StageState::CouldNotRun(e.to_string())
            } else if let Some(sum) = fin {
                finished_state(sum)
            } else {
                stage_state(ov, stage)
            };
            let stored: Option<RunSummaryView> = ov
                .last_runs
                .iter()
                .find(|(s, _)| *s == stage)
                .map(|(_, r)| r.into());
            StageRow {
                stage,
                state,
                // A run that just ended shows its own counts, not the previous run's.
                last_run: match fin {
                    Some(sum) => Some(RunSummaryView {
                        summary: Some(sum.clone()),
                        started_at: "this session".into(),
                        ended_at: Some("just now".into()),
                        porter_version: env!("CARGO_PKG_VERSION").into(),
                        dry_run: stage == Stage::Load && st.dry_load,
                    }),
                    None => stored,
                },
                progress: st.progress_of(stage).cloned(),
            }
        })
        .collect()
}

/// Same rules as `stage_state`, from a summary the engine just reported.
fn finished_state(sum: &Summary) -> StageState {
    match exit_code(sum) {
        3 => StageState::Interrupted,
        2 => StageState::Errors,
        _ if sum.warnings > 0 => StageState::Warnings,
        _ => StageState::Clean,
    }
}

pub fn glyph(s: &StageState) -> (&'static str, Option<Color>) {
    match s {
        StageState::Never => ("·", None),
        StageState::Running => ("●", Some(Color::Cyan)),
        StageState::Clean => ("✓", Some(Color::Green)),
        StageState::Warnings => ("⚠", Some(Color::Yellow)),
        StageState::Errors => ("✗", Some(Color::Red)),
        StageState::Interrupted => ("⏸", Some(Color::Yellow)),
        StageState::CouldNotRun(_) => ("‼", Some(Color::Red)),
    }
}

pub fn progress_text(p: &Progress) -> String {
    match p {
        (done, Some(total), label) => format!("{done}/{total} {label}"),
        (done, None, label) => format!("{done} {label}"),
    }
}

fn state_text(s: &StageState) -> &'static str {
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

pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let focused = app.focus == Panel::Stages;
    let block = panel("Stages", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines: Vec<Line> = rows(app)
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let (g, c) = glyph(&r.state);
            let sel = i == app.panels.stages.sel && focused;
            let tail = match (&r.progress, &r.state) {
                (Some(p), _) => progress_text(p),
                (None, StageState::Running) => "starting…".into(),
                (None, StageState::CouldNotRun(e)) => {
                    format!("could not run: {}", e.lines().next().unwrap_or(""))
                }
                (None, s) if r.last_run.as_ref().is_some_and(|l| l.dry_run) => {
                    format!("{} (dry run)", state_text(s))
                }
                (None, s) => state_text(s).into(),
            };
            let mut line = Line::from(vec![
                Span::raw(if sel { "> " } else { "  " }),
                Span::styled(g, tint(c)),
                Span::raw(format!(" {:<10} {tail}", name(r.stage))),
            ]);
            if sel {
                line = line.style(accent());
            }
            line
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
