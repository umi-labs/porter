//! Overview: the pipeline strip with Run and Cancel per stage, coverage from the
//! analysis, and open issues by kind.

use gpui::{
    Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme, Theme, h_flex, v_flex};
use porter_engine::session::{RunPlan, StageState};
use porter_model::{Severity, Stage};

use crate::project_window::{ProjectWindow, dry};
use crate::theme::{radius, text, tokens};
use crate::ui::buttons::{Kind, button};
use crate::ui::chip::{Chip, ChipState, STALE_OPACITY};
use crate::ui::severity::SeverityChip;
use crate::vm::pipeline::{StageRow, Tone, relative_time};

pub fn tone_color(t: Tone, theme: &Theme) -> Hsla {
    match t {
        Tone::Muted => theme.muted_foreground,
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
        .child(v_flex().gap_1().child(heading("Pipeline")).child(muted(
            "Stages re-run independently. A changed upstream stage marks the ones after it stale.",
            cx,
        )))
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

/// A card button that does nothing while disabled.
fn action(
    id: (&'static str, usize),
    label: &'static str,
    enabled: bool,
    cx: &mut Context<ProjectWindow>,
    f: impl Fn(&mut ProjectWindow, &mut Context<ProjectWindow>) + 'static,
) -> impl IntoElement {
    let b = button(id, Kind::Secondary, label, cx);
    if enabled {
        b.on_click(cx.listener(move |this, _, _, cx| f(this, cx)))
    } else {
        b.opacity(0.5).cursor_default()
    }
}

fn stage_card(w: &ProjectWindow, r: StageRow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let t = tokens(cx);
    let busy = w.pipeline.busy();
    let stage = r.stage;
    let i = stage as usize;
    let running = matches!(r.state, StageState::Running);
    let counts = r.counts();
    let mut chip = Chip::new(ChipState::from(&r.state)).label(r.label.clone());
    if r.stale.is_some() {
        chip = chip.stale();
    }
    // Stale is a modifier: the old chip and counts stay, dimmed, and the reason says why.
    let mut result = v_flex()
        .gap_1()
        .min_w_0()
        .child(chip)
        .children(counts.map(|c| small(c, t.text)))
        .children((!r.status.is_empty()).then(|| small(r.status.clone(), t.muted)));
    if r.stale.is_some() {
        result = result.opacity(STALE_OPACITY);
    }
    let button = match &r.state {
        StageState::Running => action(
            ("cancel", i),
            "Cancel",
            !w.pipeline.cancelling,
            cx,
            |this, cx| this.cancel(cx),
        )
        .into_any_element(),
        // A writing load resumes through the Apply sheet and its gates, never directly.
        StageState::Interrupted if stage == Stage::Load && !r.dry_run => {
            action(("resume", i), "Resume…", !busy, cx, |this, cx| {
                this.open_apply(cx)
            })
            .into_any_element()
        }
        // The engine skips what is already done.
        StageState::Interrupted => action(("resume", i), "Resume", !busy, cx, move |this, cx| {
            this.start(RunPlan::Stage(stage, dry()), cx)
        })
        .into_any_element(),
        _ => action(
            ("run", i),
            if stage == Stage::Load {
                "Dry run"
            } else {
                "Run"
            },
            !busy,
            cx,
            move |this, cx| this.start(RunPlan::Stage(stage, dry()), cx),
        )
        .into_any_element(),
    };
    v_flex()
        .flex_1()
        .min_w_0()
        .p_3()
        .gap_2()
        .rounded(radius::MD)
        .border_1()
        .border_color(if running { t.accent } else { t.border })
        .bg(if running { t.accent_bg } else { t.surface })
        .child(
            h_flex()
                .gap_2()
                .min_w_0()
                .text_size(text::MICRO)
                .text_color(t.faint)
                .child(format!("{}", i + 1))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(text::BODY)
                        .text_color(t.text)
                        .child(capitalised(&r.name)),
                ),
        )
        .child(result)
        .children(
            r.stale
                .clone()
                .map(|why| small(format!("↻ {why}"), t.muted)),
        )
        .children(r.last_run.clone().map(|when| {
            let shown = relative_time(&when, chrono::Utc::now());
            small(shown, t.faint)
                .id(("last-run", i))
                .tooltip(move |window, cx| Tooltip::new(when.clone()).build(window, cx))
        }))
        .child(h_flex().pt_1().child(button))
}

fn small(s: impl Into<SharedString>, color: Hsla) -> gpui::Div {
    div()
        .text_size(text::DENSE)
        .line_height(px(16.))
        .text_color(color)
        .min_w_0()
        .child(s.into())
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
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
            .child(div().w_24().child(SeverityChip(k.worst)))
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
