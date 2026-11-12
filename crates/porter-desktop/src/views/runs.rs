//! Runs: the live log of the current run (newest first) and the run history per stage.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, uniform_list,
};
use gpui_component::{ActiveTheme, h_flex, v_flex};

use super::overview::error;
use crate::project_window::ProjectWindow;
use crate::vm::pipeline::name;
use crate::vm::runs::history_row;

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    let n = w.runs.log().len();
    let log = uniform_list(
        "run-log",
        n,
        cx.processor(|this, range: std::ops::Range<usize>, _, _| {
            let log = this.runs.log();
            range
                .map(|i| {
                    let line = log
                        .len()
                        .checked_sub(1 + i)
                        .and_then(|j| log.get(j))
                        .cloned()
                        .unwrap_or_default();
                    div().text_sm().font_family("monospace").child(line)
                })
                .collect::<Vec<_>>()
        }),
    )
    .size_full();
    v_flex()
        .size_full()
        .p_4()
        .gap_4()
        .child(div().text_lg().child("Live log"))
        .child(
            div()
                .h_64()
                .p_2()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .child(if n == 0 {
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Nothing has run in this window yet.")
                        .into_any_element()
                } else {
                    log.into_any_element()
                }),
        )
        .child(div().text_lg().child("History"))
        .child(history(w, cx))
}

fn history(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> gpui::AnyElement {
    let all = match &w.runs.history {
        None => return div().child("Loading…").into_any_element(),
        Some(Err(e)) => return error(e, cx).into_any_element(),
        Some(Ok(h)) => h,
    };
    let theme = cx.theme();
    let sections = all.iter().map(|(stage, runs)| {
        let rows = runs.iter().map(|r| {
            let h = history_row(*stage, r);
            h_flex()
                .gap_4()
                .py_1()
                .text_sm()
                .border_b_1()
                .border_color(theme.border)
                .child(div().w_56().child(h.started_at))
                .child(div().w_56().child(h.ended_at))
                .child(div().w_32().child(h.outcome))
                .child(div().text_color(theme.muted_foreground).child(h.counts))
        });
        v_flex()
            .gap_1()
            .child(
                div()
                    .pt_2()
                    .child(format!("{} ({})", name(*stage), runs.len())),
            )
            .children(if runs.is_empty() {
                Some(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("never run"),
                )
            } else {
                None
            })
            .children(rows)
    });
    v_flex()
        .id("history")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .children(sections)
        .into_any_element()
}
