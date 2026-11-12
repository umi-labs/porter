//! Overview: the pipeline strip with Run and Cancel per stage, coverage from the
//! analysis, and open issues by kind.

use gpui::{
    Context, Hsla, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, div,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Disableable, Theme, h_flex, v_flex};
use porter_engine::session::RunPlan;
use porter_model::Severity;

use crate::project_window::{ProjectWindow, dry};
use crate::vm::pipeline::{StageRow, Tone};

pub fn tone_color(t: Tone, theme: &Theme) -> Hsla {
    match t {
        Tone::Muted => theme.muted_foreground,
        Tone::Accent => theme.info,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
    }
}

pub fn severity_tone(s: Severity) -> Tone {
    match s {
        Severity::Error => Tone::Danger,
        Severity::Warning => Tone::Warning,
        Severity::Info => Tone::Muted,
    }
}

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let strip = h_flex().w_full().gap_2().items_start().children(
        w.pipeline
            .rows()
            .into_iter()
            .map(|r| stage_card(w, r, cx).into_any_element())
            .collect::<Vec<_>>(),
    );
    v_flex()
        .id("overview")
        .size_full()
        .overflow_y_scroll()
        .p_4()
        .gap_6()
        .child(heading("Pipeline"))
        .child(strip)
        .child(heading("Coverage"))
        .child(coverage(w, cx))
        .child(heading("Open issues by kind"))
        .child(issues(w, cx))
}

fn heading(t: &'static str) -> impl IntoElement {
    div().text_lg().child(t)
}

fn muted(t: impl Into<gpui::SharedString>, cx: &Context<ProjectWindow>) -> gpui::Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(t.into())
}

fn stage_card(w: &ProjectWindow, r: StageRow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    let busy = w.pipeline.busy();
    let running = w.pipeline.running_stage() == Some(r.stage);
    let stage = r.stage;
    let counts = r.summary.as_ref().map(|s| {
        format!(
            "{} done · {} failed · {} errors · {} warnings",
            s.done, s.failed, s.errors, s.warnings
        )
    });
    v_flex()
        .flex_1()
        .min_w_0()
        .p_3()
        .gap_1()
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .child(
            h_flex()
                .gap_2()
                .child(div().text_color(tone_color(r.tone, theme)).child(r.glyph))
                .child(div().child(r.name.clone())),
        )
        .child(
            div()
                .text_sm()
                .text_color(tone_color(r.tone, theme))
                .child(r.status.clone()),
        )
        .children(
            r.last_run
                .clone()
                .map(|t| muted(format!("last run {t}"), cx)),
        )
        .children(counts.map(|c| muted(c, cx)))
        .child(
            h_flex()
                .pt_2()
                .gap_1()
                .child(
                    Button::new(("run", stage as usize))
                        .label("Run")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start(RunPlan::Stage(stage, dry()), cx)
                        })),
                )
                .child(
                    Button::new(("cancel", stage as usize))
                        .ghost()
                        .label("Cancel")
                        .disabled(!running || w.pipeline.cancelling)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                ),
        )
}

fn coverage(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> gpui::AnyElement {
    let c = match &w.coverage {
        None => return muted("Loading…", cx).into_any_element(),
        Some(Err(e)) => return error(e, cx).into_any_element(),
        Some(Ok(None)) => {
            return muted("No coverage yet: analyse writes it.", cx).into_any_element();
        }
        Some(Ok(Some(c))) => c,
    };
    let theme = cx.theme();
    let mut about = vec![];
    if let Some(p) = &c.platform {
        about.push(format!("platform {p}"));
    }
    if !c.builders.is_empty() {
        about.push(format!("builders {}", c.builders.join(", ")));
    }
    if !c.plugins.is_empty() {
        about.push(format!("plugins {}", c.plugins.join(", ")));
    }
    let rows = c.connectors.iter().map(|k| {
        let found = k
            .contributed
            .iter()
            .map(|(what, n)| format!("{what} {n}"))
            .collect::<Vec<_>>()
            .join(" · ");
        v_flex()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_4()
                    .child(div().w_40().child(k.name.clone()))
                    .child(div().text_sm().child(if found.is_empty() {
                        "found nothing".to_string()
                    } else {
                        found
                    })),
            )
            .children(k.gaps.iter().map(|g| {
                div()
                    .text_sm()
                    .text_color(theme.warning)
                    .child(format!("gap: {g}"))
            }))
    });
    v_flex()
        .gap_1()
        .child(muted(about.join(" · "), cx))
        .children(rows)
        .into_any_element()
}

fn issues(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> gpui::AnyElement {
    let totals = w.pipeline.overview.as_ref().map(|o| {
        format!(
            "{} errors · {} warnings · {} info",
            o.open_errors, o.open_warnings, o.open_infos
        )
    });
    let kinds = match &w.kinds {
        None => return muted("Loading…", cx).into_any_element(),
        Some(Err(e)) => return error(e, cx).into_any_element(),
        Some(Ok(k)) if k.is_empty() => {
            return muted("No open issues.", cx).into_any_element();
        }
        Some(Ok(k)) => k,
    };
    let theme = cx.theme();
    let rows = kinds.iter().map(|k| {
        h_flex()
            .gap_4()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .w_4()
                    .text_color(tone_color(severity_tone(k.worst), theme))
                    .child("●"),
            )
            .child(div().w_48().child(k.kind.clone()))
            .child(div().child(k.total.to_string()))
    });
    v_flex()
        .gap_1()
        .children(totals.map(|t| muted(t, cx)))
        .children(rows)
        .into_any_element()
}

pub fn error(e: &str, cx: &Context<ProjectWindow>) -> gpui::Div {
    div()
        .text_sm()
        .text_color(cx.theme().danger)
        .child(format!("error: {e}"))
}
