use crate::keymap::{self, Action};
use crate::lists::{Filter, ISSUES_PAGE, IssueList, IssuesState, ItemsState, Window, kind_name};
use crate::msg::{Cmd, DataMsg, Msg, PreviewInfo, RefreshScope};
use crate::setup::{Failure, Setup};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use porter_engine::events::{Event, Summary};
use porter_engine::session::{
    ItemDetail, Overview, RunInfo, RunOutcome, RunPlan, STAGES, Session, StageOpts,
};
use porter_engine::stage::SchemaMode;
use porter_model::{Issue, ItemId, ItemStatus, Resolution, Severity, Stage};
use std::collections::VecDeque;
use std::sync::Arc;
use uuid::Uuid;

pub const LOG_CAP: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Project,
    Stages,
    Issues,
    Items,
}

impl Panel {
    pub const ALL: [Panel; 4] = [Panel::Project, Panel::Stages, Panel::Issues, Panel::Items];

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
    /// How a stage that just finished ended, until the overview re-query lands: no frame
    /// may show the previous run's state for a stage whose new run is already over.
    finished: [Option<Summary>; 6],
    pub live: Live,
    /// The run in flight (or just ended) loads without apply: its load is a dry run.
    pub dry_load: bool,
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

    pub fn finished_of(&self, s: Stage) -> Option<&Summary> {
        self.finished[idx(s)].as_ref()
    }

    pub fn selected(&self) -> Stage {
        STAGES[self.sel.min(STAGES.len() - 1)]
    }
}

/// Per-panel state.
#[derive(Default)]
pub struct Panels {
    pub overview: Option<Overview>,
    pub stages: StagesState,
    pub issues: IssuesState,
    pub items: ItemsState,
}

/// What the Detail pane shows when it has been entered with Enter; Esc pops.
pub enum DetailView {
    /// A plain message (the result of re-checking porter.toml).
    Notice(String),
    Issues(IssueList),
    Issue(Issue),
    Item(Box<ItemDetail>),
}

#[derive(Debug, PartialEq)]
pub struct ResolveModal {
    pub issue: Uuid,
    pub title: String,
    /// 0 handled manually, 1 won't fix, 2 reopen.
    pub choice: usize,
    pub note: String,
    pub error: Option<String>,
}

/// What `L` is about to write, shown before anything runs.
#[derive(Debug, PartialEq)]
pub struct ApplyModal {
    pub url: String,
    /// The last dry run; None when there was none.
    pub preview: Option<PreviewInfo>,
}

#[derive(Debug, PartialEq)]
pub enum Modal {
    ConfirmQuit,
    ConfirmApply(ApplyModal),
    Resolve(ResolveModal),
}

pub struct LogLine(pub String);

pub struct App {
    /// None when the folder has no porter.toml.
    pub session: Option<Arc<Session>>,
    /// The setup form, while there is no project to show.
    pub setup: Option<Setup>,
    pub focus: Panel,
    pub panels: Panels,
    pub modal: Option<Modal>,
    pub detail: Vec<DetailView>,
    /// Overview queries in flight; the live finished state stays until the last lands.
    pending_overview: u32,
    seq: u64,
    /// The item query in flight and the view that asked for it.
    item_req: Option<(u64, ViewKey)>,
    pub log: VecDeque<LogLine>,
    pub show_log: bool,
    pub help: bool,
    pub help_scroll: usize,
    pub running: Option<RunInfo>,
    pub size: (u16, u16),
    pub quit: bool,
    /// Quitting because of SIGTERM (exit 143).
    pub terminated: bool,
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
            setup: None,
            focus: Panel::Project,
            panels: Panels::default(),
            modal: None,
            detail: vec![],
            pending_overview: 0,
            seq: 0,
            item_req: None,
            log: VecDeque::new(),
            show_log: true,
            help: false,
            help_scroll: 0,
            running: None,
            size,
            quit: false,
            terminated: false,
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

    pub(crate) fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Asks for an item, remembering which view asked so the answer can be checked.
    fn request_item(&mut self, id: ItemId) -> Cmd {
        let seq = self.next_seq();
        self.item_req = Some((seq, view_key(self)));
        Cmd::Item { id, seq }
    }

    fn overview_cmd(&mut self) -> Cmd {
        self.pending_overview += 1;
        Cmd::Refresh(RefreshScope::All)
    }

    /// Re-queries what is on screen: the overview, the issue counts, the visible page of
    /// items and of any open issue list. Never more than one page per list.
    pub fn refresh_cmds(&mut self) -> Vec<Cmd> {
        let mut v = vec![self.overview_cmd()];
        let seq = self.next_seq();
        self.panels.issues.seq = seq;
        v.push(Cmd::IssueGroups {
            mode: self.panels.issues.mode,
            seq,
        });
        let it = &mut self.panels.items;
        let off = it.win.refresh();
        v.push(it.cmd(off));
        for d in &mut self.detail {
            if let DetailView::Issues(l) = d {
                let off = l.win.refresh();
                v.push(l.cmd(off));
            }
        }
        v
    }

    fn set_focus(&mut self, p: Panel) {
        self.focus = p;
        self.detail.clear();
    }

    fn on_event(&mut self, e: Event) {
        let st = &mut self.panels.stages;
        match e {
            // Late: the run already ended, so nothing is running.
            Event::StageStarted { .. } if self.running.is_none() => {}
            Event::StageStarted { stage, .. } => {
                st.running = Some(stage);
                st.progress[idx(stage)] = None;
                st.could_not_run[idx(stage)] = None;
                st.finished[idx(stage)] = None;
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
                st.finished[idx(stage)] = Some(summary.clone());
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
        for (stage, sum) in &o.summaries {
            st.finished[idx(*stage)] = Some(sum.clone());
        }
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
        if o.seeded {
            self.push_log("mapping seeded in porter.toml".into());
        }
        if let Some((stage, why)) = o.skipped {
            self.push_log(format!("{} skipped: {why}", crate::ui::stages::name(stage)));
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
        self.panels.stages.dry_load = match &plan {
            RunPlan::Stage(_, o) => !o.apply,
            RunPlan::All { apply, .. } => !apply,
        };
        if let RunPlan::Stage(s, _) = &plan {
            self.panels.stages.running = Some(*s);
            self.panels.stages.could_not_run[idx(*s)] = None;
            self.panels.stages.finished[idx(*s)] = None;
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
            return if finished { app.refresh_cmds() } else { vec![] };
        }
        Msg::Resize(w, h) => app.size = (w, h),
        Msg::Lagged(n) => {
            app.push_log(format!("dropped {n} events"));
            app.dirty = true;
            // A dropped event may have been a stage ending: re-read what is on screen.
            return app.refresh_cmds();
        }
        Msg::Data(d) => {
            app.dirty = true;
            return on_data(app, d);
        }
        Msg::RunDone(o) => {
            app.on_run_done(o);
            app.dirty = true;
            return vec![app.overview_cmd()];
        }
        Msg::Key(k) => {
            app.dirty = true;
            return key(app, k);
        }
        Msg::Term => {
            app.quit = true;
            app.terminated = true;
            return vec![Cmd::Quit];
        }
    }
    app.dirty = true;
    vec![]
}

fn key(app: &mut App, k: KeyEvent) -> Vec<Cmd> {
    if k.kind == KeyEventKind::Release {
        return vec![];
    }
    // Ctrl-C works in every state, the exclusive ones (prompts, filter, setup) included:
    // it closes what is open, then cancels the run or quits.
    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
        app.modal = None;
        if let Some(f) = filter_mut(app) {
            f.editing = false;
        }
        return perform(app, Action::CtrlC, k);
    }
    match keymap::lookup(&keymap::active(app), &k) {
        Some(a) => perform(app, a, k),
        // Only `y` confirms an apply; every other key backs out.
        None if matches!(app.modal, Some(Modal::ConfirmApply(_))) => {
            app.modal = None;
            vec![]
        }
        None => typed(app, k),
    }
}

/// Runs what a key is bound to; the table decides which keys reach here.
fn perform(app: &mut App, a: Action, k: KeyEvent) -> Vec<Cmd> {
    match a {
        Action::FocusNext => app.set_focus(app.focus.step(1)),
        Action::FocusPrev => app.set_focus(app.focus.step(3)),
        Action::FocusNth => {
            if let KeyCode::Char(c) = k.code
                && let Some(p) = c.to_digit(10).and_then(|n| Panel::ALL.get(n as usize - 1))
            {
                app.set_focus(*p);
            }
        }
        Action::Down => return move_by(app, 1),
        Action::Up => return move_by(app, -1),
        Action::PageDown => return move_by(app, 20),
        Action::PageUp => return move_by(app, -20),
        Action::Open => return open(app),
        Action::Back => {
            if app.detail.pop().is_none() {
                return clear_filter(app);
            }
        }
        Action::RunStage => {
            let s = app.panels.stages.selected();
            return app.start_stage(s);
        }
        Action::RunAll => {
            return app.start(RunPlan::All {
                keep_going: false,
                apply: false,
            });
        }
        Action::Verify => return app.start_stage(Stage::Verify),
        Action::Cancel => return app.cancel(),
        Action::CtrlC => {
            app.help = false;
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
        Action::Quit => return quit(app),
        Action::ToggleLog => app.show_log = !app.show_log,
        Action::Help => app.help = true,
        Action::CloseHelp => {
            app.help = false;
            app.help_scroll = 0;
        }
        Action::LoadDry => {
            return app.start(RunPlan::Stage(
                Stage::Load,
                StageOpts {
                    refresh: false,
                    apply: false,
                    schema_mode: SchemaMode::Check,
                },
            ));
        }
        // Apply is never started from here: the counts are read first, then `y` confirms.
        Action::LoadApply => {
            if app.session.is_none() {
                return vec![];
            }
            if app.running.is_some() {
                app.push_log("a stage is already running".into());
                return vec![];
            }
            return vec![Cmd::PreviewCounts];
        }
        Action::ApplyYes => return apply_confirmed(app),
        Action::Report => {
            let name = match (app.focus, app.panels.stages.selected()) {
                (Panel::Stages, Stage::Extract) => "extract",
                (Panel::Stages, Stage::Verify) => "verify",
                _ => "analysis",
            };
            return vec![Cmd::OpenReport(name.into())];
        }
        Action::Edit => {
            if app.running.is_some() {
                app.push_log("a stage is running: not opening the editor until it ends".into());
                return vec![];
            }
            return vec![Cmd::OpenEditor];
        }
        Action::Copy => match selected_id(app) {
            Some(id) => {
                app.push_log(format!("copied {id} (OSC 52: needs terminal support)"));
                return vec![Cmd::Copy(id)];
            }
            None => app.push_log("nothing selected to copy".into()),
        },
        Action::Filter => {
            if let Some(f) = filter_mut(app) {
                f.editing = true;
            }
        }
        Action::CycleFilter => return cycle_filter(app),
        Action::Resolve => {
            let m = match app.detail.last() {
                Some(DetailView::Issues(l)) => l.win.at(l.sel).map(resolve_modal),
                Some(DetailView::Issue(i)) => Some(resolve_modal(i)),
                _ => None,
            };
            app.modal = m.or(app.modal.take());
        }
        Action::Jump => {
            if let Some(id) = issue_item(app) {
                return vec![app.request_item(id)];
            }
        }
        Action::ConfirmYes => {
            app.quit = true;
            app.cancelling = true;
            return vec![Cmd::Cancel, Cmd::Quit];
        }
        Action::CloseModal => app.modal = None,
        Action::ResolveNext | Action::ResolvePrev => {
            if let Some(Modal::Resolve(m)) = &mut app.modal {
                m.choice = (m.choice + if a == Action::ResolveNext { 1 } else { 2 }) % 3;
            }
        }
        Action::ResolveBackspace => {
            if let Some(Modal::Resolve(m)) = &mut app.modal
                && m.choice == 0
            {
                m.note.pop();
            }
        }
        Action::ResolveConfirm => return resolve_confirm(app),
        Action::SetupNext
        | Action::SetupPrev
        | Action::SetupEnter
        | Action::SetupCycleNext
        | Action::SetupCyclePrev
        | Action::SetupDone
        | Action::SetupBackspace
        | Action::SetupSave => return setup_action(app, a),
        Action::FilterCancel => return clear_filter(app),
        Action::FilterCommit => return commit_filter(app),
        Action::FilterBackspace => {
            if let Some(f) = filter_mut(app) {
                f.text.pop();
            }
            reset_filter_sel(app);
        }
    }
    vec![]
}

/// The setup form's keys. Saving checks the required fields here (pure) and leaves the
/// file work, which blocks, to the runtime.
fn setup_action(app: &mut App, a: Action) -> Vec<Cmd> {
    let Some(st) = &mut app.setup else {
        return vec![];
    };
    match a {
        Action::SetupNext => st.step(1),
        Action::SetupPrev => st.step(-1),
        Action::SetupEnter => st.enter(),
        Action::SetupCycleNext => st.cycle(1),
        Action::SetupCyclePrev => st.cycle(-1),
        Action::SetupDone => st.editing = false,
        Action::SetupBackspace => st.pop(),
        Action::SetupSave if st.saving => {}
        Action::SetupSave => {
            st.editing = false;
            let missing: Failure = st.missing();
            if !missing.fields.is_empty() {
                st.failure = missing;
                return vec![];
            }
            st.failure = Failure::default();
            st.saving = true;
            return vec![Cmd::SaveSetup(st.values.clone())];
        }
        _ => {}
    }
    vec![]
}

/// Keys the table does not bind: text typed into the `/` filter or the resolve note.
fn typed(app: &mut App, k: KeyEvent) -> Vec<Cmd> {
    let KeyCode::Char(c) = k.code else {
        return vec![];
    };
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        return vec![];
    }
    if let Some(st) = &mut app.setup {
        st.push(c);
        return vec![];
    }
    if let Some(Modal::Resolve(m)) = &mut app.modal {
        if m.choice == 0 {
            m.note.push(c);
        }
        return vec![];
    }
    if app.modal.is_none()
        && !app.help
        && let Some(f) = filter_mut(app)
        && f.editing
    {
        f.text.push(c);
        reset_filter_sel(app);
    }
    vec![]
}

/// Results the runtime ran off the render loop. Pages that answer a replaced query or a
/// page that is no longer wanted are dropped; a page that left the selection outside the
/// window (the list shrank) asks for the right one.
fn on_data(app: &mut App, d: DataMsg) -> Vec<Cmd> {
    match d {
        DataMsg::Overview(o) => {
            app.panels.overview = Some(*o);
            landed_overview(app);
        }
        DataMsg::OverviewFailed(e) => {
            app.push_log(format!("error: {e}"));
            landed_overview(app);
        }
        DataMsg::Error(e) => app.push_log(format!("error: {e}")),
        DataMsg::Items { seq, page, paged } => {
            let it = &mut app.panels.items;
            if it.win.apply(seq, page.offset, paged) {
                it.sel = it.sel.min(it.win.total.saturating_sub(1));
                it.fsel = it.fsel.min(it.shown().len().saturating_sub(1));
                let sel = it.sel;
                return it.win.move_to(sel).map(|o| it.cmd(o)).into_iter().collect();
            }
        }
        DataMsg::IssueGroups { seq, groups } => {
            let st = &mut app.panels.issues;
            if seq == st.seq {
                st.groups = groups;
                st.sel = st.sel.min(st.shown().len().saturating_sub(1));
            }
        }
        DataMsg::IssuePage { seq, page, paged } => {
            for d in &mut app.detail {
                if let DetailView::Issues(l) = d
                    && l.win.apply(seq, page.offset, paged.clone())
                {
                    l.sel = l.sel.min(l.win.total.saturating_sub(1));
                    let sel = l.sel;
                    return l.win.move_to(sel).map(|o| l.cmd(o)).into_iter().collect();
                }
            }
        }
        // Only the latest request, and only while the view that made it is on screen.
        DataMsg::Item { seq, detail } => {
            if app.item_req == Some((seq, view_key(app))) {
                app.item_req = None;
                app.detail.push(DetailView::Item(detail));
            }
        }
        DataMsg::Resolved(Ok((id, r))) => {
            app.push_log(format!("issue {id} resolved: {r:?}"));
            for d in &mut app.detail {
                match d {
                    DetailView::Issue(i) if i.id == id => i.resolution = r.clone(),
                    DetailView::Issues(l) => {
                        for i in l.win.rows.iter_mut().filter(|i| i.id == id) {
                            i.resolution = r.clone();
                        }
                    }
                    _ => {}
                }
            }
            return app.refresh_cmds();
        }
        DataMsg::Preview(preview) => {
            let url = app
                .session
                .as_ref()
                .map(|s| s.config().destination.url.clone())
                .unwrap_or_default();
            app.modal = Some(Modal::ConfirmApply(ApplyModal { url, preview }));
        }
        DataMsg::SetupSaved(Ok(session)) => {
            app.session = Some(session);
            app.setup = None;
            app.focus = Panel::Project;
            app.push_log("wrote porter.toml".into());
            return app.refresh_cmds();
        }
        DataMsg::SetupSaved(Err(failure)) => {
            if let Some(st) = &mut app.setup {
                st.saving = false;
                st.failure = failure;
            }
        }
        DataMsg::ApplyRechecked(info) => {
            // Only while the prompt is still open; a fresh read of "no dry run" refuses too.
            if matches!(app.modal, Some(Modal::ConfirmApply(_))) {
                return match info {
                    Some(p) => apply_decide(app, Some(p)),
                    None => {
                        app.modal = None;
                        app.push_log("apply refused: no dry run yet — press l first".into());
                        vec![]
                    }
                };
            }
        }
        DataMsg::Config(msgs) => {
            for m in &msgs {
                app.push_log(format!("porter.toml: {m}"));
            }
            if matches!(app.detail.last(), Some(DetailView::Notice(_))) {
                app.detail.pop();
            }
            app.detail.push(DetailView::Notice(msgs.join("\n")));
        }
        DataMsg::Reported(Ok(p)) => app.push_log(format!("opened {}", p.display())),
        DataMsg::Reported(Err(e)) => app.push_log(format!("error: {e}")),
        DataMsg::Resolved(Err(e)) => app.push_log(format!("could not resolve: {e}")),
    }
    vec![]
}

/// The stored overview is now current; drop the live finished state once no query is left.
fn landed_overview(app: &mut App) {
    app.pending_overview = app.pending_overview.saturating_sub(1);
    if app.pending_overview == 0 {
        app.panels.stages.finished = Default::default();
    }
}

/// Moves the selection of whatever list the keys drive: an open issue list, the stages,
/// or the focused panel.
fn move_by(app: &mut App, d: i64) -> Vec<Cmd> {
    if app.help {
        app.help_scroll = (app.help_scroll as i64 + d).max(0) as usize;
        return vec![];
    }
    if let Some(DetailView::Issues(l)) = app.detail.last_mut() {
        return l.step(d);
    }
    match app.focus {
        Panel::Items => app.panels.items.step(d),
        Panel::Issues => {
            let st = &mut app.panels.issues;
            let n = st.shown().len();
            st.sel = if d < 0 {
                st.sel.saturating_sub(d.unsigned_abs() as usize)
            } else {
                st.sel + d as usize
            }
            .min(n.saturating_sub(1));
            vec![]
        }
        Panel::Stages => {
            let st = &mut app.panels.stages;
            st.sel = (st.sel as i64 + d).clamp(0, STAGES.len() as i64 - 1) as usize;
            vec![]
        }
        Panel::Project => vec![],
    }
}

/// Enter: drill into an issue list's issue, an issue's item, or the focused list's selection.
fn open(app: &mut App) -> Vec<Cmd> {
    match app.detail.last() {
        Some(DetailView::Issues(l)) => {
            if let Some(i) = l.win.at(l.sel).cloned() {
                app.detail.push(DetailView::Issue(i));
            }
            return vec![];
        }
        Some(DetailView::Issue(_)) => {
            return issue_item(app)
                .map(|id| app.request_item(id))
                .into_iter()
                .collect();
        }
        _ => {}
    }
    if app.focus == Panel::Items {
        let Some(id) = app.panels.items.selected().map(|r| r.id) else {
            return vec![];
        };
        app.detail.clear();
        return vec![app.request_item(id)];
    }
    let Some(g) = app.panels.issues.selected() else {
        return vec![];
    };
    let (kind, mode) = (g.kind, app.panels.issues.mode);
    let seq = app.next_seq();
    let mut win = Window::new(ISSUES_PAGE);
    win.reset(seq);
    let list = IssueList {
        kind,
        mode,
        win,
        sel: 0,
    };
    let cmd = list.cmd(0);
    app.detail = vec![DetailView::Issues(list)];
    vec![cmd]
}

fn cycle_filter(app: &mut App) -> Vec<Cmd> {
    if app.focus == Panel::Items {
        let it = &mut app.panels.items;
        it.mode = it.mode.next();
        it.fsel = 0;
        return vec![];
    }
    let mode = app.panels.issues.mode.next();
    let seq = app.next_seq();
    let st = &mut app.panels.issues;
    st.mode = mode;
    st.sel = 0;
    st.seq = seq;
    vec![Cmd::IssueGroups { mode, seq }]
}

fn filter_mut(app: &mut App) -> Option<&mut Filter> {
    match app.focus {
        Panel::Items => Some(&mut app.panels.items.filter),
        Panel::Issues => Some(&mut app.panels.issues.filter),
        _ => None,
    }
}

/// Drops the `/` text of the focused list; an items text the server was given is re-queried.
fn clear_filter(app: &mut App) -> Vec<Cmd> {
    if app.focus == Panel::Issues {
        let st = &mut app.panels.issues;
        st.filter.text.clear();
        st.filter.editing = false;
        st.sel = 0;
        return vec![];
    }
    let it = &mut app.panels.items;
    it.filter.text.clear();
    it.filter.editing = false;
    it.fsel = 0;
    if it.filter.server.take().is_some() {
        return reload_items(app);
    }
    vec![]
}

fn reload_items(app: &mut App) -> Vec<Cmd> {
    let seq = app.next_seq();
    let it = &mut app.panels.items;
    it.win.reset(seq);
    it.sel = 0;
    it.fsel = 0;
    vec![it.cmd(0)]
}

/// Enter in the `/` filter: the text stays on the loaded page and, for items, is also
/// handed to the server so it narrows the whole result.
fn commit_filter(app: &mut App) -> Vec<Cmd> {
    let items = app.focus == Panel::Items;
    let Some(f) = filter_mut(app) else {
        return vec![];
    };
    f.editing = false;
    if items {
        let text = Some(f.text.trim().to_string()).filter(|t| !t.is_empty());
        if text != f.server {
            f.server = text;
            return reload_items(app);
        }
    }
    reset_filter_sel(app);
    vec![]
}

fn reset_filter_sel(app: &mut App) {
    match app.focus {
        Panel::Items => app.panels.items.fsel = 0,
        Panel::Issues => app.panels.issues.sel = 0,
        _ => {}
    }
}

fn resolve_modal(i: &Issue) -> Modal {
    Modal::Resolve(ResolveModal {
        issue: i.id,
        title: format!("{}: {}", kind_name(i.kind), i.message),
        choice: 0,
        note: String::new(),
        error: None,
    })
}

fn resolve_confirm(app: &mut App) -> Vec<Cmd> {
    let Some(Modal::Resolve(m)) = &mut app.modal else {
        return vec![];
    };
    let r = match m.choice {
        0 if m.note.trim().is_empty() => {
            m.error = Some("a note is required for \"handled manually\"".into());
            return vec![];
        }
        0 => Resolution::HandledManually {
            note: m.note.trim().to_string(),
        },
        1 => Resolution::WontFix,
        _ => Resolution::Open,
    };
    let id = m.issue;
    app.modal = None;
    vec![Cmd::Resolve(id, r)]
}

/// `y` in the apply prompt: the verdict is re-read now (the dry run may have gone stale
/// while the prompt was open); `DataMsg::ApplyRechecked` then decides.
fn apply_confirmed(app: &mut App) -> Vec<Cmd> {
    let Some(Modal::ConfirmApply(m)) = &app.modal else {
        return vec![];
    };
    if m.preview.as_ref().is_none_or(|p| p.stale.is_some()) {
        return apply_decide(app, None);
    }
    vec![Cmd::RecheckApply]
}

/// Runs the real load only when a dry run showed what it would write and is still fresh.
fn apply_decide(app: &mut App, fresh: Option<PreviewInfo>) -> Vec<Cmd> {
    let Some(Modal::ConfirmApply(m)) = app.modal.take() else {
        return vec![];
    };
    let current = fresh.or(m.preview);
    match &current {
        None => {
            app.push_log("apply refused: no dry run yet — press l first".into());
            return vec![];
        }
        Some(p) if p.stale.is_some() => {
            app.push_log("apply refused: dry run is stale — press l to refresh".into());
            return vec![];
        }
        Some(_) => {}
    }
    app.start(RunPlan::Stage(
        Stage::Load,
        StageOpts {
            refresh: false,
            apply: true,
            schema_mode: SchemaMode::Check,
        },
    ))
}

/// Which view is on screen: focus, Detail depth and what it has selected.
type ViewKey = (Panel, usize, Option<String>);

fn view_key(app: &App) -> ViewKey {
    (app.focus, app.detail.len(), selected_id(app))
}

/// The item of the issue open in Detail.
fn issue_item(app: &App) -> Option<ItemId> {
    match app.detail.last() {
        Some(DetailView::Issue(i)) => i.item,
        _ => None,
    }
}

/// The id of what the focused list or Detail view has selected.
fn selected_id(app: &App) -> Option<String> {
    match app.detail.last() {
        Some(DetailView::Issue(i)) => return Some(i.id.to_string()),
        Some(DetailView::Issues(l)) => return l.win.at(l.sel).map(|i| i.id.to_string()),
        Some(DetailView::Item(d)) => return Some(d.row.id.to_string()),
        _ => {}
    }
    match app.focus {
        Panel::Items => app.panels.items.selected().map(|r| r.id.to_string()),
        _ => None,
    }
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

    fn setup_app() -> App {
        let mut app = App::new(None, (80, 24));
        app.setup = Some(Setup::new("demo"));
        app
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn focus_on(app: &mut App, id: &str) {
        app.setup.as_mut().unwrap().focus = crate::setup::field_index(id).unwrap();
    }

    #[test]
    fn setup_typing_goes_into_the_field_not_actions() {
        let mut app = setup_app();
        focus_on(&mut app, "project.name");
        press(&mut app, KeyCode::Enter);
        assert!(app.setup.as_ref().unwrap().editing);
        // `q`, `j` and `l` are actions outside a field; inside they are text.
        assert!(press(&mut app, KeyCode::Char('q')).is_empty());
        type_text(&mut app, "jl");
        assert!(!app.quit);
        assert_eq!(app.setup.as_ref().unwrap().get("project.name"), "demoqjl");
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.setup.as_ref().unwrap().get("project.name"), "demoqj");
        press(&mut app, KeyCode::Esc);
        assert!(!app.setup.as_ref().unwrap().editing);
        assert_eq!(press(&mut app, KeyCode::Char('q')), [Cmd::Quit]);
    }

    #[test]
    fn setup_choice_cycles_and_expands_its_connector() {
        let mut app = setup_app();
        focus_on(&mut app, "crawl.use");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.setup.as_ref().unwrap().get("crawl.use"), "yes");
        press(&mut app, KeyCode::Down);
        assert_eq!(
            crate::setup::FIELDS[app.setup.as_ref().unwrap().focus].id,
            "crawl.url"
        );
        focus_on(&mut app, "crawl.use");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.setup.as_ref().unwrap().get("crawl.use"), "no");
    }

    #[test]
    fn setup_save_asks_for_required_fields_first() {
        let mut app = setup_app();
        focus_on(&mut app, "destination.url");
        let save = |app: &mut App| {
            update(
                app,
                Msg::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            )
        };
        assert!(save(&mut app).is_empty());
        let st = app.setup.as_ref().unwrap();
        assert!(!st.saving);
        assert!(
            st.failure
                .fields
                .iter()
                .any(|(i, _)| crate::setup::FIELDS[*i].id == "destination.url")
        );
        let mut app = setup_app();
        app.setup
            .as_mut()
            .unwrap()
            .set("destination.url", "https://cms.example.com");
        let cmds = save(&mut app);
        assert!(
            matches!(cmds.as_slice(), [Cmd::SaveSetup(v)] if v.len() == crate::setup::FIELDS.len())
        );
        assert!(app.setup.as_ref().unwrap().saving);
        assert!(
            save(&mut app).is_empty(),
            "no second save while one is running"
        );
    }

    #[test]
    fn setup_saved_opens_the_main_screen_with_a_session() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Setup::new("demo");
        s.set("destination.url", "https://cms.example.com");
        crate::setup::save(d.path(), &s.values).unwrap();
        let session = Arc::new(Session::open(d.path()).unwrap());
        let mut app = setup_app();
        app.setup.as_mut().unwrap().saving = true;
        let cmds = update(&mut app, Msg::Data(DataMsg::SetupSaved(Ok(session))));
        assert!(app.session.is_some() && app.setup.is_none());
        assert!(matches!(cmds.first(), Some(Cmd::Refresh(_))), "{cmds:?}");
        // Main-screen keys work now: no restart needed.
        assert!(!press(&mut app, KeyCode::Char('R')).is_empty());
    }

    #[test]
    fn setup_failure_keeps_the_form() {
        let mut app = setup_app();
        app.setup.as_mut().unwrap().saving = true;
        let failure = crate::setup::Failure {
            fields: vec![(0, "project.name is missing".into())],
            general: vec!["boom".into()],
        };
        update(
            &mut app,
            Msg::Data(DataMsg::SetupSaved(Err(failure.clone()))),
        );
        let st = app.setup.as_ref().unwrap();
        assert!(app.session.is_none() && !st.saving);
        assert_eq!(st.failure, failure);
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
        // The overview, the issue counts and the one visible page of items: nothing more.
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], Cmd::Refresh(RefreshScope::All));
        assert!(matches!(cmds[1], Cmd::IssueGroups { .. }));
        assert!(matches!(cmds[2], Cmd::Items { .. }));
        let cmds = update(&mut app, Msg::RunDone(Default::default()));
        assert_eq!(cmds, [Cmd::Refresh(RefreshScope::All)]);
    }

    #[test]
    fn a_stage_start_after_the_run_ended_is_ignored() {
        use porter_engine::events::Event;
        use porter_engine::store::RunId;
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('R'));
        update(&mut app, Msg::RunDone(Default::default()));
        update(
            &mut app,
            Msg::Engine(Event::StageStarted {
                stage: porter_model::Stage::Load,
                run: RunId(uuid::Uuid::nil()),
            }),
        );
        assert!(
            app.panels.stages.running.is_none(),
            "stage shown running after RunDone"
        );
    }

    #[test]
    fn run_done_says_mapping_seeded_and_verify_skipped() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('R'));
        update(
            &mut app,
            Msg::RunDone(RunOutcome {
                seeded: true,
                skipped: Some((porter_model::Stage::Verify, "dry run".into())),
                ..Default::default()
            }),
        );
        let log: Vec<_> = app.log.iter().map(|l| l.0.as_str()).collect();
        assert!(log.contains(&"mapping seeded in porter.toml"), "{log:?}");
        assert!(log.contains(&"verify skipped: dry run"), "{log:?}");
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

    #[test]
    fn resolve_note_backspace_works() {
        let mut app = App::new(None, (80, 24));
        app.modal = Some(Modal::Resolve(ResolveModal {
            issue: Uuid::nil(),
            title: "t".into(),
            choice: 0,
            note: String::new(),
            error: None,
        }));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('b'));
        press(&mut app, KeyCode::Backspace);
        let Some(Modal::Resolve(m)) = &app.modal else {
            panic!()
        };
        assert_eq!(m.note, "a");
    }

    #[test]
    fn ctrl_c_works_in_filter_resolve_box_and_quit_prompt() {
        // Filter box: editing stops and the idle press quits.
        let mut app = App::new(None, (80, 24));
        app.focus = Panel::Items;
        press(&mut app, KeyCode::Char('/'));
        assert!(app.panels.items.filter.editing);
        assert_eq!(ctrl_c(&mut app), [Cmd::Quit]);
        assert!(app.quit && !app.panels.items.filter.editing);
        // Resolve box: closed, then quits.
        let mut app = App::new(None, (80, 24));
        app.modal = Some(Modal::Resolve(ResolveModal {
            issue: Uuid::nil(),
            title: "t".into(),
            choice: 0,
            note: String::new(),
            error: None,
        }));
        assert_eq!(ctrl_c(&mut app), [Cmd::Quit]);
        assert!(app.quit && app.modal.is_none());
        // Quit prompt (a run in flight): closed and the run cancelled; again quits.
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('R'));
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.modal, Some(Modal::ConfirmQuit));
        assert_eq!(ctrl_c(&mut app), [Cmd::Cancel]);
        assert!(app.modal.is_none() && !app.quit);
        assert_eq!(ctrl_c(&mut app), [Cmd::Cancel, Cmd::Quit]);
        // Setup field: quits, never types a `c`.
        let mut app = setup_app();
        focus_on(&mut app, "project.name");
        press(&mut app, KeyCode::Enter);
        assert_eq!(ctrl_c(&mut app), [Cmd::Quit]);
    }

    /// SIGTERM goes through the same quit path as `q`: the runtime then cancels the run
    /// and waits for it (`finish_run`) before exiting 143, whatever is open.
    #[test]
    fn sigterm_quits_through_the_awaited_path() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('R'));
        app.help = true;
        app.modal = Some(Modal::ConfirmQuit);
        assert_eq!(update(&mut app, Msg::Term), [Cmd::Quit]);
        assert!(app.quit && app.terminated);
    }

    #[test]
    fn ctrl_c_works_under_help() {
        let mut app = App::new(None, (80, 24));
        app.help = true;
        let c = update(
            &mut app,
            Msg::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert!(app.quit && !app.help && c.contains(&Cmd::Quit));
    }

    #[test]
    fn detail_list_keys_work_whatever_the_focus() {
        use crate::lists::{IssueList, Mode};
        let mut app = App::new(None, (80, 24));
        let mut win = Window::new(10);
        win.reset(1);
        win.apply(
            1,
            0,
            porter_engine::session::Paged {
                rows: vec![],
                total: 5,
            },
        );
        app.detail.push(DetailView::Issues(IssueList {
            kind: porter_model::IssueKind::MissingMedia,
            mode: Mode::All,
            win,
            sel: 0,
        }));
        app.focus = Panel::Project;
        press(&mut app, KeyCode::Char('j'));
        let Some(DetailView::Issues(l)) = app.detail.last() else {
            panic!()
        };
        assert_eq!(l.sel, 1);
        app.focus = Panel::Stages;
        press(&mut app, KeyCode::Char('k'));
        let Some(DetailView::Issues(l)) = app.detail.last() else {
            panic!()
        };
        assert_eq!(l.sel, 0);
        assert_eq!(app.panels.stages.sel, 0, "stages did not move");
    }

    fn apply_cmd() -> Cmd {
        Cmd::Run(RunPlan::Stage(
            porter_model::Stage::Load,
            StageOpts {
                refresh: false,
                apply: true,
                schema_mode: SchemaMode::Check,
            },
        ))
    }

    fn counts() -> std::collections::BTreeMap<String, u64> {
        [("posts".to_string(), 12u64), ("media".to_string(), 3)].into()
    }

    fn fresh() -> PreviewInfo {
        PreviewInfo {
            counts: counts(),
            at: std::time::SystemTime::now(),
            stale: None,
        }
    }

    #[test]
    fn l_is_a_dry_run() {
        let (mut app, _d) = live_app();
        assert!(matches!(
            press(&mut app, KeyCode::Char('l')).as_slice(),
            [Cmd::Run(RunPlan::Stage(porter_model::Stage::Load, o))] if !o.apply
        ));
    }

    #[test]
    fn apply_needs_explicit_y() {
        for other in [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Char('n'),
            KeyCode::Char('L'),
            KeyCode::Char(' '),
        ] {
            let (mut app, _d) = live_app();
            assert_eq!(press(&mut app, KeyCode::Char('L')), [Cmd::PreviewCounts]);
            assert!(app.modal.is_none(), "nothing writes before the counts land");
            update(&mut app, Msg::Data(DataMsg::Preview(Some(fresh()))));
            let Some(Modal::ConfirmApply(m)) = &app.modal else {
                panic!("{:?}", app.modal)
            };
            assert_eq!(m.preview.as_ref().map(|p| &p.counts), Some(&counts()));
            assert!(press(&mut app, other).is_empty(), "{other:?}");
            assert!(app.modal.is_none() && app.running.is_none(), "{other:?}");
        }
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('L'));
        update(&mut app, Msg::Data(DataMsg::Preview(Some(fresh()))));
        assert_eq!(press(&mut app, KeyCode::Char('y')), [Cmd::RecheckApply]);
        assert!(
            app.running.is_none(),
            "nothing writes until the recheck lands"
        );
        assert_eq!(
            update(&mut app, Msg::Data(DataMsg::ApplyRechecked(Some(fresh())))),
            [apply_cmd()]
        );
        assert!(app.modal.is_none() && app.running.is_some());
    }

    #[test]
    fn apply_without_dry_run_says_so() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('L'));
        update(&mut app, Msg::Data(DataMsg::Preview(None)));
        assert!(matches!(&app.modal, Some(Modal::ConfirmApply(m)) if m.preview.is_none()));
        assert!(press(&mut app, KeyCode::Char('y')).is_empty());
        assert!(app.running.is_none() && app.modal.is_none());
        assert!(
            app.log
                .back()
                .is_some_and(|l| l.0.contains("press l first")),
            "refusal is logged"
        );
    }

    fn stale_apply(why: &str) -> (App, tempfile::TempDir) {
        let (mut app, d) = live_app();
        press(&mut app, KeyCode::Char('L'));
        let mut p = fresh();
        p.stale = Some(why.into());
        update(&mut app, Msg::Data(DataMsg::Preview(Some(p))));
        (app, d)
    }

    #[test]
    fn stale_preview_refuses_apply() {
        let (mut app, _d) = stale_apply("normalise ran after it");
        assert!(press(&mut app, KeyCode::Char('y')).is_empty());
        assert!(app.running.is_none() && app.modal.is_none());
        assert!(
            app.log
                .back()
                .is_some_and(|l| l.0.contains("stale — press l")),
            "{:?}",
            app.log.back().map(|l| &l.0)
        );
    }

    #[test]
    fn staleness_is_rechecked_on_y() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('L'));
        update(&mut app, Msg::Data(DataMsg::Preview(Some(fresh()))));
        assert_eq!(press(&mut app, KeyCode::Char('y')), [Cmd::RecheckApply]);
        // Went stale while the prompt was open: refused, nothing runs.
        let mut p = fresh();
        p.stale = Some("normalise ran after it".into());
        assert!(update(&mut app, Msg::Data(DataMsg::ApplyRechecked(Some(p)))).is_empty());
        assert!(app.running.is_none() && app.modal.is_none());
        assert!(app.log.back().is_some_and(|l| l.0.contains("stale")));
        // A late answer with no prompt open does nothing.
        assert!(update(&mut app, Msg::Data(DataMsg::ApplyRechecked(Some(fresh())))).is_empty());
        assert!(app.running.is_none());
    }

    /// A finished dry-run load recorded with the config as it is now, and its preview.
    fn dry_run(s: &Session, dir: &std::path::Path) {
        let store = s.project().store();
        let cfg = porter_engine::config::ProjectConfig::load(&dir.join("porter.toml")).unwrap();
        let fp = porter_engine::fingerprint::fingerprint(&cfg, Stage::Load);
        let run = store.begin_run_with(Stage::Load, Some(&fp)).unwrap();
        store.mark_dry_run(run).unwrap();
        store.end_run(run, &Default::default()).unwrap();
        let p = dir.join("raw/load-preview");
        std::fs::create_dir_all(p.join("posts")).unwrap();
        std::fs::write(p.join("posts/a.json"), "{}").unwrap();
        std::fs::write(p.join(".done"), "").unwrap();
    }

    #[test]
    fn normalise_run_after_dry_run_makes_preview_stale() {
        let (app, d) = live_app();
        let s = app.session.clone().unwrap();
        dry_run(&s, d.path());
        let p = s.load_preview_counts().unwrap();
        assert_eq!(s.preview_stale(&p), None);
        let store = s.project().store();
        let run = store.begin_run(Stage::Normalise).unwrap();
        store.end_run(run, &Default::default()).unwrap();
        assert_eq!(
            s.preview_stale(&p).as_deref(),
            Some("normalise re-ran after it")
        );
    }

    #[test]
    fn config_edit_makes_preview_stale() {
        // The staleness rule itself lives in the session; the runtime feeds it to the prompt.
        let (app, d) = live_app();
        let s = app.session.clone().unwrap();
        dry_run(&s, d.path());
        let p = s.load_preview_counts().unwrap();
        assert_eq!(s.preview_stale(&p), None);
        let toml = d.path().join("porter.toml");
        let t = std::fs::read_to_string(&toml).unwrap();
        // A comment is not a change to what load reads.
        std::fs::write(&toml, t.clone() + "\n# edited\n").unwrap();
        assert_eq!(s.preview_stale(&p), None);
        std::fs::write(
            &toml,
            t + "\n[[redirects]]\nfrom = \"/a\"\nto = \"/b\"\ncode = 301\n",
        )
        .unwrap();
        assert_eq!(
            s.preview_stale(&p).as_deref(),
            Some("redirects changed since")
        );
        let info = crate::actions::preview_info(&s).unwrap();
        assert!(info.stale.is_some());
    }

    #[test]
    fn apply_refused_while_running() {
        let (mut app, _d) = live_app();
        press(&mut app, KeyCode::Char('l'));
        assert!(press(&mut app, KeyCode::Char('L')).is_empty());
        assert!(
            app.log
                .back()
                .is_some_and(|l| l.0.contains("already running"))
        );
    }

    #[test]
    fn editor_never_opens_during_a_run() {
        let (mut app, _d) = live_app();
        assert_eq!(press(&mut app, KeyCode::Char('e')), [Cmd::OpenEditor]);
        press(&mut app, KeyCode::Char('R'));
        assert!(press(&mut app, KeyCode::Char('e')).is_empty());
        assert!(app.log.back().is_some_and(|l| l.0.contains("running")));
    }

    #[test]
    fn config_messages_show_in_detail() {
        let (mut app, _d) = live_app();
        update(
            &mut app,
            Msg::Data(DataMsg::Config(vec!["config OK".into()])),
        );
        assert!(matches!(app.detail.last(), Some(DetailView::Notice(t)) if t == "config OK"));
        update(
            &mut app,
            Msg::Data(DataMsg::Config(vec!["a".into(), "b".into()])),
        );
        assert!(matches!(app.detail.last(), Some(DetailView::Notice(t)) if t == "a\nb"));
        press(&mut app, KeyCode::Esc);
        assert!(app.detail.is_empty());
    }

    #[test]
    fn report_follows_the_focused_stage() {
        let (mut app, _d) = live_app();
        assert_eq!(
            press(&mut app, KeyCode::Char('o')),
            [Cmd::OpenReport("analysis".into())]
        );
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(
            press(&mut app, KeyCode::Char('o')),
            [Cmd::OpenReport("extract".into())]
        );
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(
            press(&mut app, KeyCode::Char('o')),
            [Cmd::OpenReport("verify".into())]
        );
    }

    #[test]
    fn y_copies_the_selected_id() {
        let mut app = App::new(None, (80, 24));
        app.focus = Panel::Issues;
        assert!(
            press(&mut app, KeyCode::Char('y')).is_empty(),
            "nothing selected"
        );
        let i = Issue {
            id: Uuid::from_u128(7),
            kind: porter_model::IssueKind::MissingMedia,
            severity: Severity::Error,
            item: None,
            stage: porter_model::Stage::Load,
            message: "m".into(),
            details: serde_json::Value::Null,
            resolution: Resolution::Open,
        };
        app.detail.push(DetailView::Issue(i.clone()));
        assert_eq!(
            press(&mut app, KeyCode::Char('y')),
            [Cmd::Copy(i.id.to_string())]
        );
    }

    /// An item answer lands only on the view that asked for it: after Esc (or any move
    /// away) it is dropped instead of opening over whatever is on screen now.
    #[test]
    fn item_result_is_dropped_once_its_view_is_gone() {
        let item = porter_model::ItemId(Uuid::from_u128(9));
        let issue = Issue {
            id: Uuid::from_u128(7),
            kind: porter_model::IssueKind::MissingMedia,
            severity: Severity::Error,
            item: Some(item),
            stage: porter_model::Stage::Load,
            message: "m".into(),
            details: serde_json::Value::Null,
            resolution: Resolution::Open,
        };
        let detail = |seq| {
            Msg::Data(DataMsg::Item {
                seq,
                detail: Box::new(ItemDetail {
                    row: porter_engine::session::ItemRow {
                        id: item,
                        kind: porter_model::ItemKind::Document,
                        label: "/a".into(),
                        title: None,
                        statuses: vec![],
                    },
                    source: porter_model::SourceRef {
                        connector: "wxr".into(),
                        source_id: "1".into(),
                        url: None,
                    },
                    data: serde_json::Value::Null,
                    issues: vec![],
                }),
            })
        };
        let seq_of = |c: &[Cmd]| match c {
            [Cmd::Item { id, seq }] if *id == item => *seq,
            other => panic!("{other:?}"),
        };
        let mut app = App::new(None, (80, 24));
        app.detail.push(DetailView::Issue(issue.clone()));
        let seq = seq_of(&press(&mut app, KeyCode::Char('g')));
        press(&mut app, KeyCode::Esc);
        update(&mut app, detail(seq));
        assert!(app.detail.is_empty(), "landed on a view that is gone");
        // Still on the issue that asked: it opens.
        app.detail.push(DetailView::Issue(issue));
        let seq = seq_of(&press(&mut app, KeyCode::Char('g')));
        update(&mut app, detail(seq));
        assert!(matches!(app.detail.last(), Some(DetailView::Item(_))));
    }

    #[test]
    fn filter_typing_wins_over_detail_keys() {
        use crate::lists::{IssueList, Mode};
        let mut app = App::new(None, (80, 24));
        let mut win = Window::new(10);
        win.reset(1);
        win.apply(
            1,
            0,
            porter_engine::session::Paged {
                rows: vec![],
                total: 5,
            },
        );
        app.detail.push(DetailView::Issues(IssueList {
            kind: porter_model::IssueKind::MissingMedia,
            mode: Mode::All,
            win,
            sel: 2,
        }));
        app.focus = Panel::Issues;
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.panels.issues.filter.text, "j");
        let Some(DetailView::Issues(l)) = app.detail.last() else {
            panic!()
        };
        assert_eq!(l.sel, 2, "j typed into the filter did not move the list");
    }
}
