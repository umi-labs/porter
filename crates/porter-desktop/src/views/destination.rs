//! Destination: the schema check's results grouped by collection, and the schema modes.
//! Generate, Prompt and Spec run as normal runs and then open the folder they wrote to.
//! A collection's counts open Issues filtered to the schema checks.

use gpui::prelude::*;
use gpui::{Context, FontWeight, IntoElement, ParentElement, Styled, div, px};
use gpui_component::button::Button;
use gpui_component::{Disableable, h_flex, v_flex};
use porter_model::{IssueKind, Severity};

use super::overview::error;
use crate::project_window::ProjectWindow;
use crate::theme::{space, text, tokens};
use crate::ui::table::{self, toggle};
use crate::vm::destination::{Mode, groups};
use crate::vm::nav::Link;

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let t = tokens(cx);
    let busy = w.pipeline.busy();
    let buttons = h_flex()
        .gap_2()
        .flex_wrap()
        .children(Mode::ALL.into_iter().map(|m| {
            Button::new(m.label())
                .label(m.label())
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.run_schema(m, cx)))
        }));
    let help = div().text_size(text::DENSE).text_color(t.muted).child(
        "Check compares the mapping with the Payload config. Generate writes collection \
             files to generated/payload/; Prompt and Spec write reports/schema-prompt.md and \
             reports/schema-spec.{md,html}. Nothing is written to the Payload site.",
    );
    let body: gpui::AnyElement = if let Some(e) = w.issues.error() {
        error(e, cx).into_any_element()
    } else if !w.issues.loaded() {
        table::loading("Reading the schema check\u{2026}", cx).into_any_element()
    } else {
        let g = groups(w.issues.all());
        if g.is_empty() {
            table::empty(
                "No open schema check issues",
                "Run Check schema to compare the mapping with the Payload config again.",
                cx,
            )
            .into_any_element()
        } else {
            v_flex()
                .gap(space::S4)
                .children(g.into_iter().enumerate().map(|(gi, g)| {
                    // Each count opens the issues it counts.
                    let counts = [
                        (Severity::Error, g.errors, "errors"),
                        (Severity::Warning, g.warnings, "warnings"),
                        (Severity::Info, g.infos, "info"),
                    ];
                    v_flex()
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .gap(space::S3)
                                .items_center()
                                .pb(space::S1)
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(text::HEADING)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(g.collection.clone()),
                                )
                                .children(
                                    counts.into_iter().enumerate().filter(|(_, c)| c.1 > 0).map(
                                        |(i, (sev, n, word))| {
                                            toggle(
                                                ("dest-count", gi * 3 + i),
                                                word,
                                                Some(n.to_string()),
                                                false,
                                                Some(sev),
                                                cx,
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, window, cx| {
                                                    let link = Link {
                                                        kind: Some(IssueKind::SchemaMismatch),
                                                        severity: Some(sev),
                                                    };
                                                    this.follow(link, window, cx)
                                                }),
                                            )
                                        },
                                    ),
                                ),
                        )
                        .children(
                            g.messages
                                .into_iter()
                                .map(|(sev, m)| table::reason(sev, m, cx)),
                        )
                }))
                .into_any_element()
        }
    };
    v_flex()
        .id("destination")
        .size_full()
        .overflow_y_scroll()
        .p(space::S4)
        .gap(space::S4)
        .child(table::page_head(
            "Destination",
            "schema check and schema files",
            cx,
        ))
        .child(buttons)
        .child(help)
        .children(
            w.dest_note
                .clone()
                .map(|n| div().text_size(text::BODY).child(n)),
        )
        .child(
            div()
                .text_size(text::HEADING)
                .font_weight(FontWeight::SEMIBOLD)
                .child("Schema check results"),
        )
        .child(body)
}
