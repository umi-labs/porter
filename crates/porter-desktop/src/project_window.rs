//! The project window: top bar, sidebar, and the selected section. All engine work goes
//! through the `Bridge`; this view drains its channel, feeds the view models and redraws.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Task,
    Window, div, hsla,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Disableable, Root, h_flex, v_flex};
use porter_engine::events::Event;
use porter_engine::report::Coverage;
use porter_engine::session::{RunPlan, Session, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::{ItemId, Stage};

use crate::bridge::{Bridge, DataResult, UiEvent, coalesce};
use crate::sidebar::{self, Section, View};
use crate::views;
use crate::vm::apply::{ApplySheet, age};
use crate::vm::content::{Content, NodeKey, Query};
use crate::vm::pipeline::{CLOSE_WAIT, CloseStep, KindCount, Pipeline, name};
use crate::vm::runs::Runs;

/// How often the window drains engine events: at most 20 redraws a second.
const POLL: Duration = Duration::from_millis(50);

pub struct ProjectWindow {
    pub(crate) bridge: Bridge,
    pub(crate) section: Section,
    pub(crate) pipeline: Pipeline,
    pub(crate) runs: Runs,
    pub(crate) content: Content,
    pub(crate) coverage: Option<Result<Option<Coverage>, String>>,
    pub(crate) kinds: Option<Result<Vec<KindCount>, String>>,
    /// The apply confirmation sheet, while open.
    apply: Option<ApplySheet>,
    apply_input: Entity<InputState>,
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
        // Redraw on typing so the Apply button follows the box. Enter does nothing:
        // applying always takes a click.
        let subs = vec![cx.subscribe(&apply_input, |_, _, _: &InputEvent, cx| cx.notify())];
        let mut this = ProjectWindow {
            bridge,
            section: Section::default(),
            pipeline: Pipeline::default(),
            runs: Runs::default(),
            content: Content::default(),
            coverage: None,
            kinds: None,
            apply: None,
            apply_input,
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
        this.refresh();
        this
    }

    pub fn select(&mut self, s: Section, cx: &mut Context<Self>) {
        self.section = s;
        if s == Section::Content {
            let q = self.content.open();
            self.ask(q);
        }
        cx.notify();
    }

    fn refresh(&mut self) {
        self.pipeline.refresh_requested();
        self.bridge.refresh();
        let q = self.content.invalidate();
        self.ask(q);
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
            DataResult::Preview {
                confirm: false,
                url,
                preview,
            } => {
                self.apply_input
                    .update(cx, |s, cx| s.set_value("", window, cx));
                self.apply = Some(ApplySheet::new(url, preview));
            }
            // The recheck after Apply was pressed: only acts while the sheet is open.
            DataResult::Preview {
                confirm: true,
                url,
                preview,
            } => {
                let Some(sheet) = &mut self.apply else {
                    return;
                };
                match sheet.finish(&url, preview) {
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
        match self.pipeline.start(&plan) {
            Ok(()) => {
                self.runs.start_run(describe(&plan));
                self.bridge.run(plan);
            }
            Err(e) => self.runs.push(e),
        }
        cx.notify();
    }

    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.pipeline.cancel() {
            self.runs.push("cancelling…".into());
            self.bridge.cancel();
        }
        cx.notify();
    }

    fn open_apply(&mut self, cx: &mut Context<Self>) {
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
        let theme = cx.theme();
        let busy = self.pipeline.busy();
        let session = self.bridge.session();
        let config = session.config();
        let name = if config.project.name.is_empty() {
            session.project().dir().display().to_string()
        } else {
            config.project.name.clone()
        };
        let route = format!("{} → {}", config.source.adapter, config.destination.adapter);
        h_flex()
            .w_full()
            .px_4()
            .py_2()
            .gap_3()
            .justify_between()
            .bg(theme.title_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex().gap_3().child(div().text_lg().child(name)).child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(route),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("run-all")
                            .label("Run all")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start(
                                    RunPlan::All {
                                        keep_going: false,
                                        apply: false,
                                    },
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new("dry-load")
                            .label("Load (dry run)")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start(RunPlan::Stage(Stage::Load, dry()), cx)
                            })),
                    )
                    .child(
                        Button::new("apply-load")
                            .danger()
                            .label("Apply load…")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.open_apply(cx))),
                    ),
            )
    }

    fn apply_sheet(&self, sheet: &ApplySheet, cx: &mut Context<Self>) -> impl IntoElement {
        let typed = self.apply_input.read(cx).value();
        let ok = sheet.check(&typed).is_ok() && !sheet.confirming() && !self.pipeline.busy();
        let theme = cx.theme();
        let url = if sheet.url.is_empty() {
            "(no destination.url set)".to_string()
        } else {
            sheet.url.clone()
        };
        let mut lines = vec![];
        match &sheet.preview {
            None => lines.push("No dry run yet: run Load (dry run) first.".to_string()),
            Some(p) => {
                let when = format!("Dry run from {}", age(p.at, SystemTime::now()));
                match &p.stale {
                    Some(why) => lines.push(format!("{when}; {why}. Run Load (dry run) again.")),
                    None => {
                        let total: u64 = p.counts.values().sum();
                        lines.push(format!("{when}. Items the load would send ({total}):"));
                        lines.extend(p.counts.iter().map(|(k, n)| format!("  {k}: {n}")));
                    }
                }
            }
        }
        let prompt = match sheet.host() {
            Some(h) => format!("Type {h} to confirm"),
            None => "destination.url has no host; apply is not possible".into(),
        };
        let card = v_flex()
            .w_128()
            .max_h_full()
            .p_6()
            .gap_3()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .child(div().text_lg().child("Apply load"))
            .child(
                div()
                    .text_color(theme.danger)
                    .child("This writes to a live CMS."),
            )
            .child(div().child(format!("Destination: {url}")))
            .children(lines.into_iter().map(|l| div().text_sm().child(l)))
            .child(div().text_sm().child(prompt))
            .child(Input::new(&self.apply_input))
            .children(
                sheet
                    .error
                    .clone()
                    .map(|e| div().text_sm().text_color(theme.danger).child(e)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("apply-cancel")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.close_apply(cx))),
                    )
                    .child(
                        Button::new("apply-confirm")
                            .danger()
                            .label(if sheet.confirming() {
                                "Checking…"
                            } else {
                                "Apply"
                            })
                            .disabled(!ok)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_apply(cx))),
                    ),
            );
        overlay(card)
    }
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
            View::NotBuilt(name) => not_built(name, cx).into_any_element(),
            View::Overview => views::overview::render(self, cx).into_any_element(),
            View::Content => views::content::render(self, cx).into_any_element(),
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

fn not_built(name: &'static str, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    div()
        .p_6()
        .text_color(cx.theme().muted_foreground)
        .child(format!("{name} is not built yet."))
}
