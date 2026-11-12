//! The project window: top bar, sidebar, and the selected section. All engine work goes
//! through the `Bridge`; this view only drains its channel and redraws.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use gpui::{Context, IntoElement, ParentElement, Render, SharedString, Styled, Task, Window, div};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Disableable, Root, h_flex, v_flex};
use porter_engine::session::{Overview, RunOutcome, RunPlan, Session, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::Stage;

use crate::bridge::{Bridge, DataResult, UiEvent, coalesce};
use crate::sidebar::{self, Section, View};

/// How often the window drains engine events: at most 20 redraws a second.
const POLL: Duration = Duration::from_millis(50);

pub struct ProjectWindow {
    pub(crate) bridge: Bridge,
    pub(crate) section: Section,
    pub(crate) overview: Option<Result<Overview, String>>,
    pub(crate) running: bool,
    pub(crate) log: Vec<String>,
    _poll: Task<()>,
}

impl ProjectWindow {
    pub fn new(session: Arc<Session>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (bridge, rx) = Bridge::new(session);
        bridge.refresh();
        ProjectWindow {
            bridge,
            section: Section::default(),
            overview: None,
            running: false,
            log: vec![],
            _poll: poll(rx, cx),
        }
    }

    pub fn select(&mut self, s: Section, cx: &mut Context<Self>) {
        self.section = s;
        cx.notify();
    }

    fn handle(&mut self, e: UiEvent) {
        match e {
            UiEvent::Engine(porter_engine::events::Event::StageFinished { .. }) => {
                self.bridge.refresh()
            }
            UiEvent::Engine(_) => {}
            UiEvent::Lagged(n) => {
                self.log.push(format!("dropped {n} events"));
                self.bridge.refresh();
            }
            UiEvent::Data(DataResult::Overview(o)) => self.overview = Some(o),
            UiEvent::RunDone(o) => self.run_done(o),
        }
    }

    fn run_done(&mut self, o: RunOutcome) {
        self.running = false;
        self.log.extend(o.messages);
        self.bridge.refresh();
    }

    pub(crate) fn start(&mut self, plan: RunPlan, cx: &mut Context<Self>) {
        if self.running {
            self.log.push("a stage is already running".into());
        } else {
            self.running = true;
            self.bridge.run(plan);
        }
        cx.notify();
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
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
                            .disabled(self.running)
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
                            .disabled(self.running)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start(RunPlan::Stage(Stage::Load, dry()), cx)
                            })),
                    )
                    .child(
                        // The apply gate lands with the Overview (task 3); never the default.
                        Button::new("apply-load")
                            .danger()
                            .label("Apply load…")
                            .disabled(true),
                    ),
            )
    }
}

/// A stage run that never writes to the destination.
pub(crate) fn dry() -> StageOpts {
    StageOpts {
        refresh: false,
        apply: false,
        schema_mode: SchemaMode::Check,
    }
}

/// Drains the bridge every `POLL`, coalescing progress, so a busy engine costs one redraw
/// per tick. The task dies with the window (it is stored in the view).
fn poll(rx: Receiver<UiEvent>, cx: &mut Context<ProjectWindow>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            let batch: Vec<UiEvent> = rx.try_iter().collect();
            if batch.is_empty() {
                continue;
            }
            let applied = this.update(cx, |this, cx| {
                for e in coalesce(batch) {
                    this.handle(e);
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
            View::Overview | View::Runs => {
                let text: SharedString = match &self.overview {
                    None => "Loading…".into(),
                    Some(Err(e)) => format!("error: {e}").into(),
                    Some(Ok(o)) => format!(
                        "{} stages run · {} open errors · {} warnings",
                        o.last_runs.len(),
                        o.open_errors,
                        o.open_warnings
                    )
                    .into(),
                };
                div().p_4().child(text).into_any_element()
            }
        };
        let theme = cx.theme();
        v_flex()
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
            .children(Root::render_dialog_layer(window, cx))
    }
}

fn not_built(name: &'static str, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    div()
        .p_6()
        .text_color(cx.theme().muted_foreground)
        .child(format!("{name} is not built yet."))
}
