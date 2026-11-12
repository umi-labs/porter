use crate::msg::{Cmd, DataMsg, Msg, RefreshScope};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use porter_engine::events::Event;
use porter_engine::session::{Overview, RunInfo, RunOutcome, RunPlan, STAGES, Session, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::{ItemStatus, Severity, Stage};
use std::collections::VecDeque;
use std::sync::Arc;

const LOG_CAP: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Project,
    Stages,
    Issues,
    Items,
}

impl Panel {
    const ALL: [Panel; 4] = [Panel::Project, Panel::Stages, Panel::Issues, Panel::Items];

    fn step(self, by: usize) -> Panel {
        let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Self::ALL[(i + by) % Self::ALL.len()]
    }
}

pub type Progress = (u64, Option<u64>, String);

/// Counts of the run in flight, built from events (the stored summary replaces them
/// once the stage finishes and the overview is re-queried).
#[derive(Default, Debug, PartialEq)]
pub struct Live {
    pub items: u64,
    pub failed: u64,
    pub errors: u64,
    pub warnings: u64,
}

#[derive(Default)]
pub struct StagesState {
    pub sel: usize,
    /// The stage in flight; set when its run is requested, so the row flips at once.
    pub running: Option<Stage>,
    progress: [Option<Progress>; 6],
    could_not_run: [Option<String>; 6],
    pub live: Live,
}

fn idx(s: Stage) -> usize {
    STAGES.iter().position(|x| *x == s).unwrap_or(0)
}

impl StagesState {
    pub fn progress_of(&self, s: Stage) -> Option<&Progress> {
        self.progress[idx(s)].as_ref()
    }

    pub fn could_not_run_of(&self, s: Stage) -> Option<&str> {
        self.could_not_run[idx(s)].as_deref()
    }

    pub fn selected(&self) -> Stage {
        STAGES[self.sel.min(STAGES.len() - 1)]
    }
}

/// Per-panel state; Tasks 4-5 add Issues and Items.
#[derive(Default)]
pub struct Panels {
    pub overview: Option<Overview>,
    pub stages: StagesState,
}

#[derive(Debug, PartialEq)]
pub enum Modal {
    ConfirmQuit,
}

pub struct LogLine(pub String);

pub struct App {
    /// None when the folder has no porter.toml.
    pub session: Option<Arc<Session>>,
    pub focus: Panel,
    pub panels: Panels,
    pub modal: Option<Modal>,
    pub log: VecDeque<LogLine>,
    pub running: Option<RunInfo>,
    pub size: (u16, u16),
    pub quit: bool,
    /// A cancel was requested for the run in flight.
    pub cancelling: bool,
    /// Engine events changed state since the last draw; the next tick redraws.
    pub stale: bool,
    /// Set by `update`; the runtime redraws and clears it.
    pub dirty: bool,
}

impl App {
    pub fn new(session: Option<Arc<Session>>, size: (u16, u16)) -> App {
        App {
            session,
            focus: Panel::Project,
            panels: Panels::default(),
            modal: None,
            log: VecDeque::new(),
            running: None,
            size,
            quit: false,
            cancelling: false,
            stale: false,
            dirty: true,
        }
    }

    pub(crate) fn push_log(&mut self, s: String) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(LogLine(s));
    }

    fn on_event(&mut self, e: Event) {
        let st = &mut self.panels.stages;
        match e {
            Event::StageStarted { stage, .. } => {
                st.running = Some(stage);
                st.progress[idx(stage)] = None;
                st.could_not_run[idx(stage)] = None;
                st.live = Live::default();
            }
            Event::Progress {
                stage,
                done,
                total,
                label,
            } => st.progress[idx(stage)] = Some((done, total, label)),
            Event::ItemResult { status, .. } => {
                st.live.items += 1;
                if matches!(status, ItemStatus::Failed { .. }) {
                    st.live.failed += 1;
                }
            }
            Event::IssueOpened { issue } => {
                match issue.severity {
                    Severity::Error => st.live.errors += 1,
                    Severity::Warning => st.live.warnings += 1,
                    Severity::Info => {}
                }
                self.push_log(format!(
                    "{:?} {:?}: {}",
                    issue.severity, issue.stage, issue.message
                ));
            }
            Event::StageFinished { stage, summary, .. } => {
                st.progress[idx(stage)] = None;
                if st.running == Some(stage) {
                    st.running = None;
                }
                self.push_log(format!(
                    "{stage:?} finished: {} done, {} failed, {} errors, {} warnings",
                    summary.done, summary.failed, summary.errors, summary.warnings
                ));
            }
        }
    }

    fn on_run_done(&mut self, o: RunOutcome) {
        let plan = self.running.take().map(|r| r.plan);
        self.cancelling = false;
        let st = &mut self.panels.stages;
        st.running = None;
        st.progress = Default::default();
        let mut failures = 0;
        for (stage, code, err) in &o.per_stage {
            if *code == 1 {
                failures += 1;
                st.could_not_run[idx(*stage)] =
                    Some(err.clone().unwrap_or_else(|| "could not run".into()));
            }
        }
        // `Session::run` refused before any stage ran: the messages are the reason.
        if let (0, 1, Some(RunPlan::Stage(stage, _))) = (failures, o.code, &plan) {
            st.could_not_run[idx(*stage)] = Some(o.messages.join("; "));
        }
        for (stage, code, err) in o.per_stage {
            if let (3, Some(e)) = (code, err) {
                self.push_log(format!("{stage:?} interrupted: {e}"));
            }
        }
        for m in o.messages {
            self.push_log(m);
        }
    }

    fn start(&mut self, plan: RunPlan) -> Vec<Cmd> {
        if self.session.is_none() {
            return vec![];
        }
        if self.running.is_some() {
            self.push_log("a stage is already running".into());
            return vec![];
        }
        self.panels.stages.live = Live::default();
        if let RunPlan::Stage(s, _) = &plan {
            self.panels.stages.running = Some(*s);
            self.panels.stages.could_not_run[idx(*s)] = None;
        }
        self.running = Some(RunInfo { plan: plan.clone() });
        vec![Cmd::Run(plan)]
    }

    fn start_stage(&mut self, s: Stage) -> Vec<Cmd> {
        self.start(RunPlan::Stage(
            s,
            StageOpts {
                refresh: false,
                apply: false,
                schema_mode: SchemaMode::Check,
            },
        ))
    }

    fn cancel(&mut self) -> Vec<Cmd> {
        if self.running.is_none() || self.cancelling {
            return vec![];
        }
        self.cancelling = true;
        self.push_log("cancelling…".into());
        vec![Cmd::Cancel]
    }
}

/// Pure: no I/O. Returns the commands for the runtime to execute.
pub fn update(app: &mut App, msg: Msg) -> Vec<Cmd> {
    match msg {
        // Engine events only mark state stale; the tick turns that into one redraw.
        Msg::Tick => {
            if std::mem::take(&mut app.stale) {
                app.dirty = true;
            }
            return vec![];
        }
        Msg::Engine(e) => {
            let finished = matches!(e, Event::StageFinished { .. });
            app.on_event(e);
            app.stale = true;
            return if finished {
                vec![Cmd::Refresh(RefreshScope::All)]
            } else {
                vec![]
            };
        }
        Msg::Resize(w, h) => app.size = (w, h),
        Msg::Data(DataMsg::Overview(o)) => app.panels.overview = Some(*o),
        Msg::Data(DataMsg::Error(e)) => app.push_log(format!("error: {e}")),
        Msg::RunDone(o) => {
            app.on_run_done(o);
            app.dirty = true;
            return vec![Cmd::Refresh(RefreshScope::All)];
        }
        Msg::Key(k) => {
            app.dirty = true;
            return key(app, k);
        }
    }
    app.dirty = true;
    vec![]
}

fn key(app: &mut App, k: KeyEvent) -> Vec<Cmd> {
    if k.kind == KeyEventKind::Release {
        return vec![];
    }
    if app.modal == Some(Modal::ConfirmQuit) {
        return match k.code {
            KeyCode::Char('y') => {
                app.quit = true;
                app.cancelling = true;
                vec![Cmd::Cancel, Cmd::Quit]
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                app.modal = None;
                vec![]
            }
            _ => vec![],
        };
    }
    let stages = app.focus == Panel::Stages;
    match k.code {
        KeyCode::Tab => app.focus = app.focus.step(1),
        KeyCode::BackTab => app.focus = app.focus.step(3),
        KeyCode::Char('1') => app.focus = Panel::Project,
        KeyCode::Char('2') => app.focus = Panel::Stages,
        KeyCode::Char('3') => app.focus = Panel::Issues,
        KeyCode::Char('4') => app.focus = Panel::Items,
        KeyCode::Char('j') | KeyCode::Down if stages => {
            let st = &mut app.panels.stages;
            st.sel = (st.sel + 1).min(STAGES.len() - 1);
        }
        KeyCode::Char('k') | KeyCode::Up if stages => {
            app.panels.stages.sel = app.panels.stages.sel.saturating_sub(1);
        }
        KeyCode::Char(' ') if stages => {
            let s = app.panels.stages.selected();
            return app.start_stage(s);
        }
        KeyCode::Char('R') => {
            return app.start(RunPlan::All {
                keep_going: false,
                apply: false,
            });
        }
        KeyCode::Char('v') => return app.start_stage(Stage::Verify),
        KeyCode::Char('q') => return quit(app),
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            // First press cancels a run; a second (or an idle press) quits.
            if app.running.is_some() && !app.cancelling {
                return app.cancel();
            }
            app.quit = true;
            return if app.running.is_some() {
                vec![Cmd::Cancel, Cmd::Quit]
            } else {
                vec![Cmd::Quit]
            };
        }
        KeyCode::Char('c') => return app.cancel(),
        _ => {}
    }
    vec![]
}

fn quit(app: &mut App) -> Vec<Cmd> {
    if app.running.is_some() {
        app.modal = Some(Modal::ConfirmQuit);
        vec![]
    } else {
        app.quit = true;
        vec![Cmd::Quit]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::RefreshScope;
    use porter_engine::session::{RunPlan, StageOpts};
    use porter_engine::stage::SchemaMode;

    fn press(app: &mut App, c: KeyCode) -> Vec<Cmd> {
        update(app, Msg::Key(KeyEvent::new(c, KeyModifiers::NONE)))
    }

    #[test]
    fn focus_cycles_with_tab() {
        let mut app = App::new(None, (80, 24));
        let mut seen = vec![app.focus];
        for _ in 0..4 {
            press(&mut app, KeyCode::Tab);
            seen.push(app.focus);
        }
        assert_eq!(
            seen,
            [
                Panel::Project,
                Panel::Stages,
                Panel::Issues,
                Panel::Items,
                Panel::Project
            ]
        );
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, Panel::Items);
    }

    #[test]
    fn quit_when_idle_sets_quit() {
        let mut app = App::new(None, (80, 24));
        let cmds = press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
        assert_eq!(cmds, [Cmd::Quit]);
    }

    #[test]
    fn quit_while_running_opens_confirm() {
        let mut app = App::new(None, (80, 24));
        app.running = Some(RunInfo {
            plan: RunPlan::Stage(
                porter_model::Stage::Load,
                StageOpts {
                    refresh: false,
                    apply: false,
                    schema_mode: SchemaMode::Check,
                },
            ),
        });
        assert!(press(&mut app, KeyCode::Char('q')).is_empty());
        assert!(!app.quit);
        assert_eq!(app.modal, Some(Modal::ConfirmQuit));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.modal, None);
        press(&mut app, KeyCode::Char('q'));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(app.quit);
        assert_eq!(cmds, [Cmd::Cancel, Cmd::Quit]);
    }

    fn live_app() -> (App, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let s = Session::open(d.path()).unwrap();
        (App::new(Some(Arc::new(s)), (120, 40)), d)
    }

    fn ctrl_c(app: &mut App) -> Vec<Cmd> {
        update(
            app,
            Msg::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        )
    }

    #[test]
    fn space_on_stage_runs_it() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('j'));
        let cmds = press(&mut app, KeyCode::Char(' '));
        assert!(matches!(
            cmds.as_slice(),
            [Cmd::Run(RunPlan::Stage(porter_model::Stage::Extract, o))] if !o.apply && !o.refresh
        ));
        assert!(app.running.is_some());
        assert_eq!(
            app.panels.stages.running,
            Some(porter_model::Stage::Extract)
        );
    }

    #[test]
    fn r_runs_all_and_v_verifies() {
        let (mut app, _d) = live_app();
        assert_eq!(
            press(&mut app, KeyCode::Char('R')),
            [Cmd::Run(RunPlan::All {
                keep_going: false,
                apply: false
            })]
        );
        let (mut app, _d) = live_app();
        assert!(matches!(
            press(&mut app, KeyCode::Char('v')).as_slice(),
            [Cmd::Run(RunPlan::Stage(porter_model::Stage::Verify, _))]
        ));
    }

    #[test]
    fn run_while_running_is_refused_with_message() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(press(&mut app, KeyCode::Char(' ')).len(), 1);
        assert!(press(&mut app, KeyCode::Char(' ')).is_empty());
        assert!(press(&mut app, KeyCode::Char('R')).is_empty());
        assert!(
            app.log
                .back()
                .is_some_and(|l| l.0.contains("already running"))
        );
    }

    #[test]
    fn ctrl_c_cancels_then_quits() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(ctrl_c(&mut app), [Cmd::Cancel]);
        assert!(!app.quit);
        assert!(app.log.back().is_some_and(|l| l.0.contains("cancelling")));
        assert_eq!(ctrl_c(&mut app), [Cmd::Cancel, Cmd::Quit]);
        assert!(app.quit);
    }

    #[test]
    fn ctrl_c_idle_quits() {
        let (mut app, _d) = live_app();
        assert_eq!(ctrl_c(&mut app), [Cmd::Quit]);
    }

    #[test]
    fn progress_events_coalesce() {
        use porter_engine::events::Event;
        use porter_model::{ItemId, ItemKind, ItemStatus, Stage};
        let (mut app, _d) = live_app();
        app.dirty = false;
        for i in 0..5000u64 {
            update(
                &mut app,
                Msg::Engine(Event::Progress {
                    stage: Stage::Load,
                    done: i,
                    total: Some(5000),
                    label: format!("item {i}"),
                }),
            );
            update(
                &mut app,
                Msg::Engine(Event::ItemResult {
                    stage: Stage::Load,
                    item: ItemId(uuid::Uuid::nil()),
                    kind: ItemKind::Document,
                    status: ItemStatus::Done,
                }),
            );
        }
        assert!(!app.dirty, "engine events must not redraw by themselves");
        assert_eq!(
            app.panels.stages.progress_of(Stage::Load),
            Some(&(4999, Some(5000), "item 4999".to_string()))
        );
        assert_eq!(app.panels.stages.live.items, 5000);
        update(&mut app, Msg::Tick);
        assert!(app.dirty);
        app.dirty = false;
        update(&mut app, Msg::Tick);
        assert!(!app.dirty, "an idle tick does not redraw");
    }

    #[test]
    fn issues_go_to_bounded_log() {
        use porter_engine::events::Event;
        use porter_model::{Issue, IssueKind, Resolution, Severity, Stage};
        let (mut app, _d) = live_app();
        for _ in 0..(LOG_CAP + 50) {
            update(
                &mut app,
                Msg::Engine(Event::IssueOpened {
                    issue: Issue {
                        id: uuid::Uuid::nil(),
                        kind: IssueKind::MissingMedia,
                        severity: Severity::Error,
                        item: None,
                        stage: Stage::Load,
                        message: "boom".into(),
                        details: serde_json::Value::Null,
                        resolution: Resolution::Open,
                    },
                }),
            );
        }
        assert_eq!(app.log.len(), LOG_CAP);
        assert_eq!(app.panels.stages.live.errors, (LOG_CAP + 50) as u64);
    }

    #[test]
    fn stage_finished_and_run_done_refresh() {
        use porter_engine::events::{Event, Summary};
        use porter_engine::store::RunId;
        let (mut app, _d) = live_app();
        let cmds = update(
            &mut app,
            Msg::Engine(Event::StageFinished {
                stage: porter_model::Stage::Load,
                run: RunId(uuid::Uuid::nil()),
                summary: Summary::default(),
            }),
        );
        assert_eq!(cmds, [Cmd::Refresh(RefreshScope::All)]);
        let cmds = update(&mut app, Msg::RunDone(Default::default()));
        assert_eq!(cmds, [Cmd::Refresh(RefreshScope::All)]);
    }

    #[test]
    fn could_not_run_is_kept_verbatim() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char(' '));
        update(
            &mut app,
            Msg::RunDone(RunOutcome {
                code: 1,
                per_stage: vec![(
                    porter_model::Stage::Analyse,
                    1,
                    Some("no such file: export.xml".into()),
                )],
                ..Default::default()
            }),
        );
        assert!(app.running.is_none());
        assert_eq!(
            app.panels
                .stages
                .could_not_run_of(porter_model::Stage::Analyse),
            Some("no such file: export.xml")
        );
    }
}
