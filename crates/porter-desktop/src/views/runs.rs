//! Runs: the live log of the current run (newest first) and the run history per stage.

use gpui::prelude::*;
use gpui::{Context, FontWeight, IntoElement, ParentElement, Styled, div, px, uniform_list};
use gpui_component::{ActiveTheme, v_flex};

use super::overview::error;
use crate::project_window::ProjectWindow;
use crate::theme::{ROW_TABLE, radius, space, text, tokens};
use crate::ui::chip::Chip;
use crate::ui::table::{self, Col};
use crate::vm::pipeline::name;
use crate::vm::runs::history_row;

fn heading(t: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .text_size(text::HEADING)
        .font_weight(FontWeight::SEMIBOLD)
        .child(t.into())
}

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let t = tokens(cx);
    let n = w.runs.log().len();
    let log = uniform_list(
        "run-log",
        n,
        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
            let log = this.runs.log();
            let mono = cx.theme().mono_font_family.clone();
            range
                .map(|i| {
                    let line = log
                        .len()
                        .checked_sub(1 + i)
                        .and_then(|j| log.get(j))
                        .cloned()
                        .unwrap_or_default();
                    // A line is one 28px row; a long one is cut here and is whole in the file.
                    div()
                        .h(ROW_TABLE)
                        .flex_none()
                        .flex()
                        .items_center()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_size(text::DENSE)
                        .font_family(mono.clone())
                        .child(line)
                })
                .collect::<Vec<_>>()
        }),
    )
    .size_full();
    v_flex()
        .size_full()
        .p(space::S4)
        .gap(space::S4)
        .child(table::page_head("Runs", "live log and history", cx))
        .child(heading("Live log"))
        .child(
            div()
                .h_64()
                .flex_none()
                .px(space::S2)
                .border_1()
                .border_color(t.border)
                .rounded(radius::MD)
                .child(if n == 0 {
                    table::empty(
                        "Nothing has run in this window yet",
                        "Run a stage from the Overview and its log appears here.",
                        cx,
                    )
                    .into_any_element()
                } else {
                    log.into_any_element()
                }),
        )
        .child(heading("History"))
        .child(history(w, cx))
}

fn history(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> gpui::AnyElement {
    let all = match &w.runs.history {
        None => return table::loading("Reading project.db\u{2026}", cx).into_any_element(),
        Some(Err(e)) => return error(e, cx).into_any_element(),
        Some(Ok(h)) => h,
    };
    let t = tokens(cx);
    let cols = [
        ("Started", Col::Fixed(190.)),
        ("Ended", Col::Fixed(190.)),
        ("Outcome", Col::Fixed(210.)),
        ("Counts", Col::Flex),
    ];
    // Not linked: a run's counts (done, failed, errors, warnings) describe that run, and
    // Issues has no per-run filter, so a link would land on rows that are not its own.
    let sections = all.iter().enumerate().map(|(si, (stage, runs))| {
        let rows = runs.iter().enumerate().map(|(i, r)| {
            let h = history_row(r);
            let id = si * 1000 + i;
            table::row(("run-row", id), None, false, cx)
                .border_b_1()
                .border_color(t.border)
                .child(table::cell(
                    ("run-start", id),
                    h.started_at,
                    Col::Fixed(190.),
                ))
                .child(table::cell(("run-end", id), h.ended_at, Col::Fixed(190.)))
                .child(
                    div()
                        .w(px(210.))
                        .flex_none()
                        .child(Chip::new(h.chip).label(h.outcome)),
                )
                .child(table::cell(("run-counts", id), h.counts, Col::Flex).text_color(t.muted))
        });
        v_flex()
            .gap_1()
            .child(
                div()
                    .pt(space::S2)
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{} ({})", name(*stage), runs.len())),
            )
            .child(if runs.is_empty() {
                div()
                    .h(ROW_TABLE)
                    .flex()
                    .items_center()
                    .text_size(text::DENSE)
                    .text_color(t.muted)
                    .child("never run")
                    .into_any_element()
            } else {
                v_flex()
                    .child(table::header(&cols, cx))
                    .children(rows)
                    .into_any_element()
            })
    });
    v_flex()
        .id("history")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .children(sections)
        .into_any_element()
}
