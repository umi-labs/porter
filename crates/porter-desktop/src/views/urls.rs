//! URLs: the old-URL table (old URL, status, target, new URL, result), virtualised, with
//! the filters as chips that carry their counts. Rows are 28px; long URLs cut in the middle
//! and keep their last two segments, the whole URL is the tooltip. The numbers are the
//! store's, and a chip's number filters to exactly those rows.

use gpui::prelude::*;
use gpui::{Context, IntoElement, ParentElement, Styled, div, px, uniform_list};
use gpui_component::{h_flex, v_flex};
use porter_engine::session::RouteFilter;

use crate::project_window::ProjectWindow;
use crate::theme::{space, text, tokens};
use crate::ui::buttons::{Kind, button};
use crate::ui::chip::Chip;
use crate::ui::path_label::PathLabel;
use crate::ui::table::{self, Col, sized, toggle};
use crate::vm::urls::Flag;

use super::overview::error;

const FILTERS: [(RouteFilter, &str, Option<porter_model::Severity>); 4] = [
    (RouteFilter::All, "All", None),
    (
        RouteFilter::Unresolved,
        "Unresolved",
        Some(porter_model::Severity::Error),
    ),
    (
        RouteFilter::Chains,
        "Redirect chains",
        Some(porter_model::Severity::Warning),
    ),
    (RouteFilter::NotFound, "Not found in source", None),
];

const COLS: [(&str, Col); 5] = [
    ("Old URL", Col::Flex),
    ("Status", Col::Fixed(52.)),
    ("Target", Col::Fixed(72.)),
    ("New URL", Col::Flex),
    ("Result", Col::Fixed(150.)),
];

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let t = tokens(cx);
    let current = w.urls.filter();
    let chips = h_flex()
        .gap(space::S2)
        .flex_wrap()
        .children(
            FILTERS
                .into_iter()
                .enumerate()
                .map(|(i, (f, label, level))| {
                    let n = w.urls.count(f);
                    // A chip with nothing behind it is not tinted.
                    let level = level.filter(|_| n.is_some_and(|n| n > 0));
                    toggle(
                        ("url-filter", i),
                        label,
                        // "…" until this filter has been read: never a guess.
                        Some(n.map_or("…".to_string(), |n| n.to_string())),
                        f == current,
                        level,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.urls_filter(f, cx)))
                }),
        );
    let note = div().text_size(text::DENSE).text_color(t.muted).child(
        "\u{201c}Result\u{201d} comes from porter\u{2019}s route table, not from the new site. \
             Verify has not checked the live site.",
    );
    let body: gpui::AnyElement = if let Some(e) = w.urls.error() {
        error(e, cx).into_any_element()
    } else if let Some(text) = w.urls.loading_text() {
        table::loading(text, cx).into_any_element()
    } else if let Some(e) = w.urls.empty() {
        let mut b = table::empty(e.title, e.body, cx);
        if e.clear_filter {
            b = b.child(
                button("urls-clear", Kind::Secondary, "Clear filter", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.urls_filter(RouteFilter::All, cx))),
            );
        }
        b.into_any_element()
    } else {
        uniform_list(
            "urls-table",
            w.urls.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                if let Some(last) = range.end.checked_sub(1) {
                    this.urls_pump(last);
                }
                range
                    .filter_map(|i| {
                        let r = this.urls.row(i)?;
                        let t = tokens(cx);
                        let (state, word) = Flag::chip(r.flag);
                        let muted = |s: &str| {
                            if s.is_empty() || s == "—" {
                                t.muted
                            } else {
                                t.text
                            }
                        };
                        Some(
                            table::row(("url-row", i), r.flag.and_then(Flag::level), false, cx)
                                .child(sized(
                                    div().child(PathLabel::new(("url-old", i), r.old.clone())),
                                    Col::Flex,
                                ))
                                .child(table::cell(
                                    ("url-status", i),
                                    r.status.clone(),
                                    Col::Fixed(52.),
                                ))
                                .child(table::cell(("url-target", i), r.target, Col::Fixed(72.)))
                                .child(
                                    sized(div(), Col::Flex).child(
                                        PathLabel::new(("url-new", i), r.new_url.clone())
                                            .missing(false),
                                    ),
                                )
                                .child(
                                    div()
                                        .w(px(150.))
                                        .flex_none()
                                        .text_color(muted(&r.new_url))
                                        .child(Chip::new(state).label(word)),
                                ),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .into_any_element()
    };
    v_flex()
        .size_full()
        .p(space::S4)
        .gap(space::S3)
        .child(table::page_head("URLs", w.urls.summary(), cx))
        .child(chips)
        .child(note)
        .child(table::header(&COLS, cx))
        .child(div().flex_1().min_h_0().child(body))
}
