//! URLs: the old-URL table (old URL, status, target, new URL), virtualised, with the
//! filters unresolved, chains and 404. The count under the filters is the store's.

use gpui::{Context, InteractiveElement, IntoElement, ParentElement, Styled, div, uniform_list};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Selectable, h_flex, v_flex};
use porter_engine::session::RouteFilter;

use super::overview::error;
use crate::project_window::ProjectWindow;
use crate::vm::urls::Flag;

const FILTERS: [(RouteFilter, &str); 4] = [
    (RouteFilter::All, "All"),
    (
        RouteFilter::Unresolved,
        "Unresolved (leads nowhere on the new site)",
    ),
    (RouteFilter::Chains, "Chains"),
    (RouteFilter::NotFound, "404"),
];

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    let current = w.urls.filter();
    let bar = h_flex()
        .gap_2()
        .items_center()
        .children(FILTERS.into_iter().map(|(f, label)| {
            Button::new(label)
                .ghost()
                .label(label)
                .selected(f == current)
                .on_click(cx.listener(move |this, _, _, cx| this.urls_filter(f, cx)))
        }))
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(w.urls.summary()),
        );
    let header = h_flex()
        .w_full()
        .gap_2()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(theme.muted_foreground)
        .border_b_1()
        .border_color(theme.border)
        .child(div().flex_1().child("Old URL"))
        .child(div().w_16().child("Status"))
        .child(div().w_24().child("Target"))
        .child(div().flex_1().child("New URL"));
    let body: gpui::AnyElement = if let Some(e) = w.urls.error() {
        error(e, cx).into_any_element()
    } else if let Some(why) = w.urls.empty_note() {
        div()
            .p_2()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(why)
            .into_any_element()
    } else {
        uniform_list(
            "urls-table",
            w.urls.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                if let Some(last) = range.end.checked_sub(1) {
                    this.urls_pump(last);
                }
                let theme = cx.theme();
                range
                    .filter_map(|i| {
                        let r = this.urls.row(i)?;
                        let flagged = match r.flag {
                            Some(Flag::Unresolved) | Some(Flag::NotFound) => theme.danger,
                            Some(Flag::Chain) => theme.warning,
                            None => theme.foreground,
                        };
                        Some(
                            h_flex()
                                .id(("url-row", i))
                                .w_full()
                                .gap_2()
                                .px_2()
                                .text_sm()
                                .whitespace_nowrap()
                                .child(div().flex_1().min_w_0().overflow_hidden().child(r.old))
                                .child(div().w_16().child(r.status))
                                .child(div().w_24().text_color(flagged).child(match r.flag {
                                    Some(f) => format!("{} ({})", r.target, f.label()),
                                    None => r.target.to_string(),
                                }))
                                .child(div().flex_1().min_w_0().overflow_hidden().child(r.new_url)),
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
        .p_2()
        .gap_2()
        .child(bar)
        .child(header)
        .child(div().flex_1().min_h_0().child(body))
}
