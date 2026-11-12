//! The project window: top bar, sidebar, and the selected section. All engine work goes
//! through the `Bridge`; this view drains its channel, feeds the view models and redraws.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext, Context, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Task,
    Window, div, hsla, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState, Paste};
use gpui_component::{ActiveTheme, Root, h_flex, v_flex};
use porter_engine::events::Event;
use porter_engine::report::Coverage;
use porter_engine::session::{MediaRow, RouteFilter, RunOutcome, RunPlan, Session, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::{ItemId, Stage};

use crate::bridge::{Bridge, DataResult, UiEvent, coalesce};
use crate::sidebar::{self, Section, View};
use crate::theme::{radius, text, tokens};
use crate::thumbs::{Cache, Thumb};
use crate::ui::buttons::{Kind, button};
use crate::views;
use crate::vm::apply::{ApplySheet, age};
use crate::vm::content::{Content, NodeKey, Query};
use crate::vm::destination::Mode;
use crate::vm::issues::{Choice, Issues, Jump, jump_target};
use crate::vm::mapping::Mapping;
use crate::vm::media::{self, Media};
use crate::vm::pipeline::{CLOSE_WAIT, CloseStep, KindCount, Pipeline, name};
use crate::vm::runs::Runs;
use crate::vm::urls::{self, Urls};

/// How often the window drains engine events: at most 20 redraws a second.
const POLL: Duration = Duration::from_millis(50);

pub struct ProjectWindow {
    pub(crate) bridge: Bridge,
    pub(crate) section: Section,
    pub(crate) pipeline: Pipeline,
    pub(crate) runs: Runs,
    pub(crate) content: Content,
    pub(crate) media: Media,
    pub(crate) urls: Urls,
    pub(crate) mapping: Mapping,
    pub(crate) issues: Issues,
    pub(crate) resolve_choice: Choice,
    /// The choice of the bulk resolve in flight, for its report.
    resolving: Choice,
    pub(crate) issues_filter: Entity<InputState>,
    pub(crate) resolve_note: Entity<InputState>,
    /// The schema mode whose run is in flight: its folder opens when it finishes.
    pending_mode: Option<Mode>,
    /// How the last Destination action went, verbatim.
    pub(crate) dest_note: Option<String>,
    pub(crate) mapping_inputs: views::mapping::Inputs,
    thumbs: Arc<Cache>,
    pub(crate) coverage: Option<Result<Option<Coverage>, String>>,
    pub(crate) kinds: Option<Result<Vec<KindCount>, String>>,
    /// The apply confirmation sheet, while open.
    apply: Option<ApplySheet>,
    apply_input: Entity<InputState>,
    /// Cancel takes focus when the sheet opens; Enter there cancels, never applies.
    apply_cancel: FocusHandle,
    _subs: Vec<Subscription>,
    /// Closing: the run was cancelled and the window goes once it has stopped.
    closing: bool,
    /// Asking whether to cancel a writing load and close.
    close_prompt: bool,
    _close_timeout: Option<Task<()>>,
    _poll: Task<()>,
}

impl ProjectWindow {
    pub fn new(session: Arc<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (bridge, rx) = Bridge::new(session);
        let apply_input = cx.new(|cx| InputState::new(window, cx));
        let mapping_inputs = views::mapping::Inputs::new(window, cx);
        let issues_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter by message or kind"));
        let resolve_note = cx.new(|cx| InputState::new(window, cx).placeholder("Note"));
        // Redraw on typing so the Apply button follows the box. Enter does nothing:
        // applying always takes a click.
        let subs = vec![
            cx.subscribe(&apply_input, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&issues_filter, |this, input, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.issues.text = input.read(cx).value().to_string();
                    cx.notify();
                }
            }),
        ];
        let mut this = ProjectWindow {
            bridge,
            section: Section::default(),
            pipeline: Pipeline::default(),
            runs: Runs::default(),
            content: Content::default(),
            media: Media::default(),
            urls: Urls::default(),
            mapping: Mapping::default(),
            issues: Issues::default(),
            resolve_choice: Choice::HandledManually,
            resolving: Choice::HandledManually,
            issues_filter,
            resolve_note,
            pending_mode: None,
            dest_note: None,
            mapping_inputs,
            thumbs: Arc::default(),
            coverage: None,
            kinds: None,
            apply: None,
            apply_input,
            apply_cancel: cx.focus_handle(),
            _subs: subs,
            closing: false,
            close_prompt: false,
            _close_timeout: None,
            _poll: poll(rx, window, cx),
        };
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| this.request_close(false, window, cx))
                .unwrap_or(true)
        });
        // Cmd-Q skips the window's close hook. Quit handlers get 100 ms (gpui's
        // SHUTDOWN_TIMEOUT) and cannot hold the app open: cancel the run and give it that
        // long, best effort.
        let quit = cx.on_app_quit(|this, cx| {
            if this.pipeline.cancel_on_quit() {
                this.cancel(cx);
            }
            cx.background_executor().timer(gpui::SHUTDOWN_TIMEOUT)
        });
        this._subs.push(quit);
        this.refresh();
        this
    }

    pub fn select(&mut self, s: Section, cx: &mut Context<Self>) {
        self.section = s;
        match s {
            Section::Content => {
                let q = self.content.open();
                self.ask(q);
            }
            Section::Media => {
                let q = self.media.open();
                self.ask_media(q);
            }
            Section::Urls => {
                let q = self.urls.open();
                self.ask_urls(q);
            }
            Section::Issues | Section::Destination => self.read_issues(true),
            Section::Mapping if self.mapping.open() => self.bridge.mapping_open(),
            _ => {}
        }
        cx.notify();
    }

    fn refresh(&mut self) {
        self.pipeline.refresh_requested();
        self.bridge.refresh();
        let q = self.content.invalidate();
        self.ask(q);
        let q = self.media.invalidate();
        self.ask_media(q);
        let q = self.urls.invalidate();
        self.ask_urls(q);
        self.read_issues(false);
        // A stage changed what the documents hold: check the mapping again.
        if self.mapping.invalidate() {
            self.bridge.mapping_open();
        }
    }

    /// Applies a mapping edit and saves it off the UI thread.
    pub(crate) fn mapping_edit(
        &mut self,
        f: impl FnOnce(&mut porter_engine::mapping::edit::MappingDoc),
    ) {
        if let Some(doc) = self.mapping.edit(f) {
            self.bridge.mapping_save(doc);
        }
    }

    pub(crate) fn mapping_reload(&mut self, cx: &mut Context<Self>) {
        if self.mapping.reload() {
            self.bridge.mapping_open();
        }
        cx.notify();
    }

    pub(crate) fn mapping_note(&mut self, msg: String, cx: &mut Context<Self>) {
        self.mapping.note(msg);
        cx.notify();
    }

    /// Selects a type (and maybe one of its fields) or a block row, and fills the editor.
    pub(crate) fn mapping_select(
        &mut self,
        ty: Option<String>,
        field: Option<String>,
        block: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mapping.selected_type = ty;
        self.mapping.selected_field = field;
        self.mapping.selected_block = block;
        self.mapping_inputs.fill(&self.mapping, window, cx);
        cx.notify();
    }

    /// Reads the issues: the first time the section is shown, or again after the store
    /// changed (`first` false), when something already read them.
    fn read_issues(&mut self, first: bool) {
        let go = if first {
            self.issues.open()
        } else {
            self.issues.invalidate()
        };
        if go {
            self.bridge.issues(self.issues.generation);
        }
    }

    /// Resolves the selected issues with the form's choice and note.
    pub(crate) fn issues_resolve(&mut self, cx: &mut Context<Self>) {
        let note = self.resolve_note.read(cx).value().to_string();
        match self.issues.plan(self.resolve_choice, &note) {
            Ok((ids, r)) => {
                self.issues.note = Some(format!("resolving {}…", ids.len()));
                self.resolving = self.resolve_choice;
                self.bridge.resolve(ids, r);
            }
            Err(e) => self.issues.note = Some(e),
        }
        cx.notify();
    }

    pub(crate) fn issues_jump(&mut self, id: ItemId, cx: &mut Context<Self>) {
        self.issues.note = Some("looking up the item…".into());
        self.bridge.jump(id);
        cx.notify();
    }

    pub(crate) fn issues_filters(&mut self, f: impl FnOnce(&mut Issues), cx: &mut Context<Self>) {
        f(&mut self.issues);
        cx.notify();
    }

    /// Runs a schema mode as a normal run; its output folder opens when it finishes.
    pub(crate) fn run_schema(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.dest_note = None;
        match self.start_run(mode.plan()) {
            Ok(()) => self.pending_mode = Some(mode),
            Err(e) => self.dest_note = Some(e),
        }
        cx.notify();
    }

    /// After a schema run: open its folder when it finished cleanly, else say why not.
    fn schema_done(&mut self, o: &RunOutcome) {
        let Some(mode) = self.pending_mode.take() else {
            return;
        };
        let session = self.bridge.session();
        let dir = mode.output_dir(session.project().dir(), &session.project().reports_dir());
        if let (0, Some(d)) = (o.code, &dir) {
            self.bridge.open_path(d.clone());
        }
        self.dest_note = Some(mode.finished_note(o, dir.as_deref()));
    }

    fn ask_media(&self, qs: Vec<media::Query>) {
        for q in qs {
            self.bridge.media(self.media.generation, q);
        }
    }

    fn ask_urls(&self, qs: Vec<urls::Query>) {
        for q in qs {
            self.bridge.urls(self.urls.generation, q);
        }
    }

    pub(crate) fn media_select(&mut self, id: ItemId, cx: &mut Context<Self>) {
        let q = self.media.select(id);
        self.ask_media(q);
        cx.notify();
    }

    /// Called while the grid draws: reads the next page when the last rows are in view.
    /// Changes only what the footer says, so it needs no redraw of its own.
    pub(crate) fn media_pump(&mut self, row: usize, cols: usize) {
        if self.media.near_end(row, cols) {
            let q = self.media.more();
            self.ask_media(q);
        }
    }

    /// The thumbnail for `row` if it is ready; otherwise starts decoding it.
    pub(crate) fn thumb(&self, row: &MediaRow) -> Option<Thumb> {
        let t = self.thumbs.get(&row.hash);
        if t.is_none() {
            self.bridge.thumb(&self.thumbs, row);
        }
        t
    }

    pub(crate) fn urls_filter(&mut self, f: RouteFilter, cx: &mut Context<Self>) {
        let q = self.urls.set_filter(f);
        self.ask_urls(q);
        cx.notify();
    }

    /// Called while the table draws, like `media_pump`.
    pub(crate) fn urls_pump(&mut self, last_visible: usize) {
        if last_visible + 20 >= self.urls.len() {
            let q = self.urls.more();
            self.ask_urls(q);
        }
    }

    /// Sends the content section's reads to the bridge.
    fn ask(&self, qs: Vec<Query>) {
        for q in qs {
            self.bridge.content(self.content.generation, q);
        }
    }

    pub(crate) fn content_toggle(&mut self, key: NodeKey, cx: &mut Context<Self>) {
        let q = self.content.toggle(key);
        self.ask(q);
        cx.notify();
    }

    pub(crate) fn content_more(&mut self, key: NodeKey, cx: &mut Context<Self>) {
        let q = self.content.more(key);
        self.ask(q);
        cx.notify();
    }

    pub(crate) fn content_select(&mut self, id: ItemId, cx: &mut Context<Self>) {
        let q = self.content.select(id);
        self.ask(q);
        cx.notify();
    }

    fn handle(&mut self, e: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            UiEvent::Engine(e) => {
                let finished = matches!(e, Event::StageFinished { .. });
                self.pipeline.on_event(&e);
                self.runs.on_event(&e);
                if finished {
                    self.refresh();
                }
            }
            UiEvent::Lagged(n) => {
                self.runs.push(format!("dropped {n} events"));
                // A dropped event may have been a stage ending: re-read everything.
                self.refresh();
            }
            UiEvent::RunDone(o) => {
                self.schema_done(&o);
                for line in self.pipeline.on_run_done(o) {
                    self.runs.push(line);
                }
                if self.closing {
                    window.remove_window();
                    return;
                }
                self.refresh();
            }
            UiEvent::Data(d) => self.on_data(d, window, cx),
        }
    }

    fn on_data(&mut self, d: DataResult, window: &mut Window, cx: &mut Context<Self>) {
        match d {
            DataResult::Overview(o) => {
                if let Some(e) = self.pipeline.overview_landed(o) {
                    self.runs.push(e);
                }
            }
            DataResult::IssueKinds(k) => self.kinds = Some(k),
            DataResult::Coverage(c) => self.coverage = Some(c),
            DataResult::History(h) => self.runs.history = Some(h),
            DataResult::Content { generation, reply } => {
                let q = self.content.land(generation, reply);
                self.ask(q);
            }
            DataResult::Media { generation, reply } => {
                let q = self.media.land(generation, reply);
                self.ask_media(q);
            }
            DataResult::Urls { generation, reply } => self.urls.land(generation, reply),
            DataResult::Issues { generation, result } => self.issues.land(generation, result),
            DataResult::Resolved(Err(e)) => self.issues.note = Some(self.issues.resolve_failed(&e)),
            DataResult::Resolved(Ok(results)) => {
                self.issues.note = Some(self.issues.resolved(self.resolving, results));
                // The overview counts and the table both changed.
                self.refresh();
            }
            DataResult::Jump(id, kind) => match kind.and_then(jump_target) {
                Ok(Jump::Content) => {
                    self.select(Section::Content, cx);
                    let q = self.content.select(id);
                    self.ask(q);
                    self.issues.note = None;
                }
                Ok(Jump::Media) => {
                    self.select(Section::Media, cx);
                    let q = self.media.select(id);
                    self.ask_media(q);
                    self.issues.note = None;
                }
                Err(e) => self.issues.note = Some(e),
            },
            DataResult::Opened(path, r) => {
                if let Err(e) = r {
                    self.dest_note = Some(format!("could not open {}: {e}", path.display()));
                }
            }
            DataResult::Mapping(r) => {
                self.mapping.land(r);
                self.mapping_inputs.fill(&self.mapping, window, cx);
            }
            // Redraw only: `handle`'s caller notifies after every drain.
            DataResult::ThumbReady => {}
            DataResult::Preview {
                confirm: false,
                facts,
            } => {
                self.apply_input
                    .update(cx, |s, cx| s.set_value("", window, cx));
                self.apply = Some(ApplySheet::new(facts));
                window.focus(&self.apply_cancel);
            }
            // The recheck after Apply was pressed: every gate again, while the sheet is open.
            DataResult::Preview {
                confirm: true,
                facts,
            } => {
                let Some(sheet) = &mut self.apply else {
                    return;
                };
                match sheet.finish(facts) {
                    Ok(plan) => {
                        self.apply = None;
                        self.start(plan, cx);
                    }
                    Err(why) => self.runs.push(format!("apply refused: {why}")),
                }
            }
        }
    }

    pub(crate) fn start(&mut self, plan: RunPlan, cx: &mut Context<Self>) {
        if let Err(e) = self.start_run(plan) {
            self.runs.push(e);
        }
        cx.notify();
    }

    /// Starts `plan`, or says why not (one run at a time).
    fn start_run(&mut self, plan: RunPlan) -> Result<(), String> {
        self.pipeline.start(&plan)?;
        self.runs.start_run(describe(&plan));
        self.bridge.run(plan);
        Ok(())
    }

    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.pipeline.cancel() {
            self.runs.push("cancelling…".into());
            self.bridge.cancel();
        }
        cx.notify();
    }

    pub(crate) fn open_apply(&mut self, cx: &mut Context<Self>) {
        if !self.pipeline.busy() {
            self.bridge.preview(false);
        }
        cx.notify();
    }

    fn confirm_apply(&mut self, cx: &mut Context<Self>) {
        let typed = self.apply_input.read(cx).value();
        if self.pipeline.busy() {
            return;
        }
        if let Some(sheet) = &mut self.apply
            && sheet.begin_confirm(&typed).is_ok()
        {
            // Freshness is read again now; the sheet may have been open a while.
            self.bridge.preview(true);
        }
        cx.notify();
    }

    /// True when the window may close now. With a run in flight it cancels and waits for
    /// a safe stop (at most `CLOSE_WAIT`), asking first if the run writes to the CMS.
    fn request_close(
        &mut self,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.pipeline.close_step(confirmed, self.closing) {
            CloseStep::Close => return true,
            CloseStep::ConfirmApply => self.close_prompt = true,
            CloseStep::CancelAndWait => {
                self.close_prompt = false;
                self.closing = true;
                self.cancel(cx);
                self._close_timeout = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(CLOSE_WAIT).await;
                    let _ = this.update_in(cx, |_, window, _| window.remove_window());
                }));
            }
            CloseStep::Wait => {}
        }
        cx.notify();
        false
    }

    fn close_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let theme = cx.theme();
        let card = v_flex()
            .w_128()
            .p_6()
            .gap_3()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded_lg();
        let card = if self.closing {
            card.child(div().text_lg().child("Closing"))
                .child(div().child(format!(
                    "cancelling… the window closes once the run stops safely (at most {} s).",
                    CLOSE_WAIT.as_secs()
                )))
        } else if self.close_prompt {
            let url = self.bridge.session().config().destination.url.clone();
            let host = url::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or(url);
            card.child(div().text_lg().child("A load is writing"))
                .child(div().text_color(theme.danger).child(format!(
                    "A load is writing to {host}. Cancel it and close the window?"
                )))
                .child(
                    div()
                        .text_sm()
                        .child("It stops at the next safe point and can be resumed."),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(Button::new("close-keep").label("Keep running").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.close_prompt = false;
                                cx.notify();
                            }),
                        ))
                        .child(
                            Button::new("close-cancel")
                                .danger()
                                .label("Cancel and close")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if this.request_close(true, window, cx) {
                                        window.remove_window();
                                    }
                                })),
                        ),
                )
        } else {
            return None;
        };
        Some(overlay(card).into_any_element())
    }

    fn close_apply(&mut self, cx: &mut Context<Self>) {
        self.apply = None;
        cx.notify();
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        let busy = self.pipeline.busy();
        let session = self.bridge.session();
        let config = session.config();
        let name = if config.project.name.is_empty() {
            session.project().dir().display().to_string()
        } else {
            config.project.name.clone()
        };
        let has_dest = !config.destination.adapter.is_empty();
        let dest = if has_dest {
            match host_of(&config.destination.url) {
                Some(h) => format!("{} · {h}", config.destination.adapter),
                None => config.destination.adapter.clone(),
            }
        } else {
            "no destination".into()
        };
        let enabled = |b: gpui::Stateful<gpui::Div>, on: bool| {
            if on {
                b
            } else {
                b.opacity(0.5).cursor_default()
            }
        };
        let run_all = button("run-all", Kind::Secondary, "Run all", cx);
        let dry_load = button("dry-load", Kind::Primary, "Load (dry run)", cx);
        let apply = button("apply-load", Kind::ApplyGhost, "Apply load…", cx);
        h_flex()
            .w_full()
            .min_w_0()
            .px_4()
            .py_2()
            .gap_3()
            .justify_between()
            .bg(t.surface)
            .border_b_1()
            .border_color(t.border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(
                        div()
                            .flex_shrink()
                            .min_w_0()
                            .truncate()
                            .text_size(text::HEADING)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(
                        h_flex()
                            .flex_shrink()
                            .min_w_0()
                            .gap_1()
                            .text_size(text::DENSE)
                            .text_color(t.muted)
                            .child(div().flex_none().child(config.source.adapter.clone()))
                            .child(div().flex_none().text_color(t.faint).child("→"))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .when(!has_dest, |d| d.text_color(t.warn))
                                    .child(dest),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(enabled(
                        if busy {
                            run_all
                        } else {
                            run_all.on_click(cx.listener(|this, _, _, cx| {
                                this.start(
                                    RunPlan::All {
                                        keep_going: false,
                                        apply: false,
                                    },
                                    cx,
                                )
                            }))
                        },
                        !busy,
                    ))
                    .child(enabled(
                        if busy {
                            dry_load
                        } else {
                            dry_load.on_click(cx.listener(|this, _, _, cx| {
                                this.start(RunPlan::Stage(Stage::Load, dry()), cx)
                            }))
                        },
                        !busy,
                    ))
                    .child(enabled(
                        if busy || !has_dest {
                            apply
                        } else {
                            apply.on_click(cx.listener(|this, _, _, cx| this.open_apply(cx)))
                        },
                        !busy && has_dest,
                    )),
            )
    }

    fn apply_sheet(&self, sheet: &ApplySheet, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        let typed = self.apply_input.read(cx).value();
        let ok = sheet.check(&typed).is_ok() && !sheet.confirming() && !self.pipeline.busy();
        let session = self.bridge.session();
        let adapter = session.config().destination.adapter.clone();
        let host = sheet.host().map(str::to_string);
        let section = |title: &'static str| {
            div()
                .text_size(text::MICRO)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted)
                .child(title)
        };
        let gates = sheet.gates().into_iter().map(|g| {
            let (mark, color) = if g.pass {
                ("✓", t.text)
            } else {
                ("✕", t.err)
            };
            h_flex()
                .gap_2()
                .items_start()
                .child(div().flex_none().w(px(14.)).text_color(color).child(mark))
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_color(color).child(g.title))
                        .child(
                            div()
                                .text_size(text::DENSE)
                                .text_color(t.muted)
                                .child(g.detail),
                        ),
                )
        });
        let warnings = sheet.open_warnings();
        let ack = (warnings > 0).then(|| {
            Checkbox::new("apply-ack")
                .label(format!("I've reviewed the {warnings} open warnings"))
                .checked(sheet.acked())
                .on_click(cx.listener(|this, _: &bool, _, cx| {
                    if let Some(s) = &mut this.apply {
                        s.toggle_ack();
                    }
                    cx.notify();
                }))
        });
        let writes = sheet.writes();
        // What will be written: only from a fresh dry run.
        let will = sheet
            .facts
            .preview
            .as_ref()
            .filter(|_| !sheet.needs_dry_run())
            .map(|p| {
                let row = |k: String, n: String| {
                    h_flex()
                        .justify_between()
                        .gap_3()
                        .text_size(text::DENSE)
                        .child(div().min_w_0().truncate().child(k))
                        .child(div().flex_none().child(n))
                };
                v_flex()
                    .gap_1()
                    .child(section("WILL BE WRITTEN"))
                    .child(
                        div()
                            .text_size(text::DENSE)
                            .text_color(t.muted)
                            .child(format!(
                                "From the dry run {} · these are the writes Apply will make",
                                age(p.at, SystemTime::now())
                            )),
                    )
                    .children(p.counts.iter().map(|(k, n)| row(k.clone(), n.to_string())))
                    .child(
                        row("Total".into(), writes.to_string()).font_weight(FontWeight::SEMIBOLD),
                    )
            });
        let wont = (!sheet.facts.not_written.is_empty()).then(|| {
            v_flex()
                .gap_1()
                .p_2()
                .rounded(radius::SM)
                .bg(t.warn_bg)
                .child(section("NOT WRITTEN"))
                .children(
                    sheet
                        .facts
                        .not_written
                        .iter()
                        .map(|l| div().text_size(text::DENSE).child(l.clone())),
                )
        });
        let confirm: gpui::AnyElement = if sheet.needs_dry_run() {
            // No confirm field until a fresh dry run exists.
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_size(text::DENSE)
                        .text_color(t.muted)
                        .child("There's no confirm field until a fresh dry run exists."),
                )
                .child(if self.pipeline.busy() {
                    button("apply-dry", Kind::Primary, "Run dry run now", cx)
                        .opacity(0.5)
                        .cursor_default()
                } else {
                    button("apply-dry", Kind::Primary, "Run dry run now", cx).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.apply = None;
                            this.start(RunPlan::Stage(Stage::Load, dry()), cx);
                        },
                    ))
                })
                .into_any_element()
        } else {
            let hint = match &host {
                Some(h) if typed.trim().eq_ignore_ascii_case(h) => "Host matches.",
                Some(_) if !typed.is_empty() => "Doesn't match the destination host.",
                Some(_) => "Paste is disabled for this field.",
                None => "destination.url has no host; apply is not possible.",
            };
            v_flex()
                .gap_1()
                .child(div().text_size(text::DENSE).child(match &host {
                    Some(h) => format!("Type {h} to confirm"),
                    None => "Type the destination host to confirm".into(),
                }))
                // Paste is refused here, before the input's own handler sees it: the host is
                // typed, never pasted (keyboard, Edit menu and context menu all send `Paste`).
                .child(
                    div()
                        .capture_action(|_: &Paste, _, cx| cx.stop_propagation())
                        .child(Input::new(&self.apply_input)),
                )
                .child(div().text_size(text::MICRO).text_color(t.muted).child(hint))
                .into_any_element()
        };
        let label = if sheet.confirming() {
            "Checking…".to_string()
        } else {
            format!("Apply {writes} writes")
        };
        // Red only when it can be pressed; no key binding, and Enter never reaches it.
        let apply = div()
            .id("apply-confirm")
            .flex()
            .items_center()
            .h(px(32.))
            .px(px(10.))
            .rounded(radius::SM)
            .border_1()
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .child(label);
        let apply = if ok {
            apply
                .border_color(t.err_solid)
                .bg(t.err_solid)
                .text_color(t.on_err)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.confirm_apply(cx)))
        } else {
            apply.border_color(t.border_strong).text_color(t.muted)
        };
        let cancel = button("apply-cancel", Kind::Secondary, "Cancel", cx)
            .track_focus(&self.apply_cancel)
            .focus(|s| s.border_color(t.accent))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    this.close_apply(cx);
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| this.close_apply(cx)));
        let card = v_flex()
            .id("apply-sheet")
            .w(px(560.))
            .max_w_full()
            .max_h_full()
            .overflow_y_scroll()
            .p_6()
            .gap_4()
            .bg(t.bg)
            .border_1()
            .border_color(t.border)
            .rounded(radius::XL)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    this.close_apply(cx);
                }
            }))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(text::TITLE)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Apply load to the destination"),
                    )
                    .child(div().text_size(text::DENSE).text_color(t.muted).child(format!(
                        "This writes to {} ({adapter}). Re-running updates by source identity, but nothing here is undone automatically.",
                        host.clone().unwrap_or_else(|| sheet.facts.url.clone())
                    ))),
            )
            .child(v_flex().gap_2().child(section("GATES")).children(gates).children(ack))
            .children(will)
            .children(wont)
            .child(confirm)
            .children(
                sheet
                    .error
                    .clone()
                    .map(|e| div().text_size(text::DENSE).text_color(t.err).child(e)),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(text::MICRO)
                            .text_color(t.muted)
                            .child("⎋ Cancel is the default. Apply has no shortcut."),
                    )
                    .child(h_flex().gap_2().child(cancel).child(apply)),
            );
        overlay(card)
    }
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

/// Dims the window and centres `card` over it.
fn overlay(card: impl IntoElement) -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(0., 0., 0., 0.45))
        .child(card)
}

/// A stage run that never writes to the destination.
pub(crate) fn dry() -> StageOpts {
    StageOpts {
        refresh: false,
        apply: false,
        schema_mode: SchemaMode::Check,
    }
}

fn describe(plan: &RunPlan) -> String {
    match plan {
        RunPlan::Stage(Stage::Load, o) if o.apply => "load started (apply)".into(),
        RunPlan::Stage(Stage::Load, _) => "load started (dry run)".into(),
        RunPlan::Stage(s, _) => format!("{} requested", name(*s)),
        RunPlan::All { apply: true, .. } => "run all requested (apply)".into(),
        RunPlan::All { .. } => "run all requested (dry-run load)".into(),
    }
}

/// Drains the bridge every `POLL`, coalescing progress, so a busy engine costs one redraw
/// per tick. The task dies with the window (it is stored in the view).
fn poll(rx: Receiver<UiEvent>, window: &mut Window, cx: &mut Context<ProjectWindow>) -> Task<()> {
    cx.spawn_in(window, async move |this, cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            let batch: Vec<UiEvent> = rx.try_iter().collect();
            if batch.is_empty() {
                continue;
            }
            let applied = this.update_in(cx, |this, window, cx| {
                for e in coalesce(batch) {
                    this.handle(e, window, cx);
                }
                cx.notify();
            });
            if applied.is_err() {
                break;
            }
        }
    })
}

impl Render for ProjectWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main: gpui::AnyElement = match self.section.view() {
            View::Overview => views::overview::render(self, cx).into_any_element(),
            View::Content => views::content::render(self, cx).into_any_element(),
            View::Media => views::media::render(self, cx).into_any_element(),
            View::Urls => views::urls::render(self, cx).into_any_element(),
            View::Mapping => views::mapping::render(self, cx).into_any_element(),
            View::Issues => views::issues::render(self, cx).into_any_element(),
            View::Destination => views::destination::render(self, cx).into_any_element(),
            View::Runs => views::runs::render(self, cx).into_any_element(),
        };
        let sheet = self
            .apply
            .as_ref()
            .map(|s| self.apply_sheet(s, cx).into_any_element());
        let closing = self.close_overlay(cx);
        let theme = cx.theme();
        v_flex()
            .relative()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.top_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar::render(self.section, cx))
                    .child(div().flex_1().h_full().min_w_0().child(main)),
            )
            .children(sheet)
            .children(closing)
            .children(Root::render_dialog_layer(window, cx))
    }
}
