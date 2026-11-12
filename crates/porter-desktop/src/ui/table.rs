//! Table and state building blocks the sections share: 28px rows with a problem bar,
//! header rows, truncating cells with a tooltip, filter chips that carry their count,
//! inline reasons, and the empty, loading and error states of Desktop-States.
//!
//! Pure choices (which colour a level gets) live in small functions with tests; the
//! GPUI elements only paint them.

use gpui::prelude::*;
use gpui::{
    App, Div, ElementId, FontWeight, Hsla, SharedString, Stateful, Styled, div, px,
    transparent_black,
};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme, h_flex, v_flex};
use porter_model::Severity;

use crate::theme::{ROW_TABLE, Tokens, radius, space, text, tokens};

/// How a column takes its width: the rest of the row, or a fixed number of pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Col {
    Flex,
    Fixed(f32),
}

pub fn sized<T: Styled>(d: T, c: Col) -> T {
    match c {
        Col::Flex => d.flex_1().min_w_0(),
        Col::Fixed(w) => d.w(px(w)).flex_none(),
    }
}

/// The colours a row with a problem takes: (bar, background). Rows without one are bare.
pub fn level_colors(t: &Tokens, level: Option<Severity>) -> (Hsla, Hsla) {
    match level {
        Some(Severity::Error) => (t.err, t.err_bg),
        Some(Severity::Warning) => (t.warn, t.warn_bg),
        Some(Severity::Info) | None => (transparent_black(), transparent_black()),
    }
}

pub fn header(cols: &[(&'static str, Col)], cx: &App) -> Div {
    let t = tokens(cx);
    h_flex()
        .w_full()
        .flex_none()
        .h(ROW_TABLE)
        .items_center()
        .gap(space::S2)
        .px(space::S2)
        .pl(space::S2 + px(3.))
        .text_size(text::MICRO)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.muted)
        .border_b_1()
        .border_color(t.border)
        .children(
            cols.iter()
                .map(|(label, c)| sized(div().truncate(), *c).child(*label)),
        )
}

/// One 28px table row. A problem gives it a 3px bar and a tinted background; the
/// selected row takes the accent tint.
pub fn row(
    id: impl Into<ElementId>,
    level: Option<Severity>,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let (bar, bg) = level_colors(&t, level);
    h_flex()
        .id(id)
        .w_full()
        .flex_none()
        .h(ROW_TABLE)
        .items_center()
        .gap(space::S2)
        .px(space::S2)
        .border_l(px(3.))
        .border_color(bar)
        .bg(if selected { t.accent_bg } else { bg })
        .text_size(text::DENSE)
        .text_color(t.text)
        .whitespace_nowrap()
        .overflow_hidden()
}

/// Text that is cut at the end when it does not fit; the whole text is the tooltip.
pub fn cell(id: impl Into<ElementId>, s: impl Into<SharedString>, col: Col) -> Stateful<Div> {
    let s: SharedString = s.into();
    let tip = s.clone();
    sized(div().id(id), col)
        .truncate()
        .when(!tip.is_empty(), move |d| {
            d.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        })
        .child(s)
}

/// The shape-and-glyph mark of a level: round "!" for an error, square "!" for a warning,
/// a ring "i" for info. Never colour alone.
pub fn mark(level: Severity, cx: &App) -> Div {
    let t = tokens(cx);
    let b = div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(14.))
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD);
    match level {
        Severity::Error => b
            .rounded_full()
            .bg(t.err_solid)
            .text_color(t.on_err)
            .child("!"),
        Severity::Warning => b
            .rounded(px(2.))
            .border_1()
            .border_color(t.warn)
            .text_color(t.warn)
            .child("!"),
        Severity::Info => b
            .rounded_full()
            .border_1()
            .border_color(t.muted)
            .text_color(t.muted)
            .child("i"),
    }
}

/// The reason a row has its problem, under the row, in the row's colours. It wraps.
pub fn reason(level: Severity, msg: impl Into<SharedString>, cx: &App) -> Div {
    let t = tokens(cx);
    let (bar, bg) = level_colors(&t, Some(level));
    h_flex()
        .w_full()
        .items_start()
        .gap(space::S2)
        .py(px(5.))
        .pl(space::S2 + px(3.))
        .pr(space::S2)
        .border_l(px(3.))
        .border_color(bar)
        .bg(bg)
        .text_size(text::DENSE)
        .text_color(t.text)
        .child(mark(level, cx))
        .child(div().flex_1().min_w_0().child(msg.into()))
}

/// A filter chip that carries the count it filters to (none: a chip with no count).
pub fn toggle(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    count: Option<String>,
    on: bool,
    level: Option<Severity>,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    let count_color = match level {
        Some(Severity::Error) => t.err,
        Some(Severity::Warning) => t.warn,
        _ => t.muted,
    };
    h_flex()
        .id(id)
        .flex_none()
        .h(px(24.))
        .px(px(10.))
        .gap(px(6.))
        .items_center()
        .rounded(radius::XS)
        .border_1()
        .border_color(if on { t.accent } else { t.border_strong })
        .bg(if on { t.accent_bg } else { transparent_black() })
        .text_size(text::DENSE)
        .text_color(t.text)
        .cursor_pointer()
        .hover(|d| d.bg(t.hover))
        .whitespace_nowrap()
        .child(label.into())
        .children(count.map(|c| {
            div()
                .text_color(count_color)
                .font_weight(FontWeight::SEMIBOLD)
                .child(c)
        }))
}

/// The title of a section: its name, and what is in it.
pub fn page_head(title: &'static str, meta: impl IntoElement, cx: &App) -> Div {
    let t = tokens(cx);
    h_flex()
        .w_full()
        .flex_none()
        .gap(space::S3)
        .items_baseline()
        .child(
            div()
                .text_size(text::HEADING)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(title),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(text::DENSE)
                .text_color(t.muted)
                .child(meta),
        )
}

/// Nothing here, and what to do about it. Names the command that fills it.
pub fn empty(title: impl Into<SharedString>, body: impl Into<SharedString>, cx: &App) -> Div {
    let t = tokens(cx);
    v_flex()
        .w_full()
        .items_center()
        .py(space::S8)
        .px(space::S4)
        .gap(space::S2)
        .child(
            div()
                .text_size(text::HEADING)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(440.))
                .text_center()
                .text_size(text::BODY)
                .text_color(t.muted)
                .child(body.into()),
        )
}

/// Still reading. The bars are flat, never animated, and no state chip shows: a loading
/// card must not look like "not run".
pub fn loading(label: impl Into<SharedString>, cx: &App) -> Div {
    let t = tokens(cx);
    v_flex()
        .w_full()
        .p(space::S4)
        .gap(space::S3)
        .child(
            div()
                .text_size(text::DENSE)
                .text_color(t.muted)
                .child(label.into()),
        )
        .children((0..4).map(|i| {
            div()
                .h(px(12.))
                .w(px(420. - 60. * i as f32))
                .max_w_full()
                .rounded(radius::XS)
                .bg(t.panel)
        }))
}

/// An engine error, shown as it came: mono, whole, never reworded.
pub fn failure(msg: &str, cx: &App) -> Div {
    let t = tokens(cx);
    v_flex()
        .w_full()
        .gap(space::S2)
        .p(space::S3)
        .rounded(radius::MD)
        .border_1()
        .border_color(t.err)
        .bg(t.err_bg)
        .child(
            h_flex()
                .gap(space::S2)
                .items_center()
                .text_size(text::DENSE)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.err)
                .child(mark(Severity::Error, cx))
                .child("Couldn't read this"),
        )
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(text::DENSE)
                .text_color(t.text)
                .child(msg.to_string()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_errors_and_warnings_tint_a_row() {
        let t = Tokens::light();
        assert_eq!(level_colors(&t, Some(Severity::Error)), (t.err, t.err_bg));
        assert_eq!(
            level_colors(&t, Some(Severity::Warning)),
            (t.warn, t.warn_bg)
        );
        let bare = level_colors(&t, None);
        assert_eq!(bare, level_colors(&t, Some(Severity::Info)));
        assert_eq!(bare.0, transparent_black());
        // Error and warning never share a colour, in either theme.
        let d = Tokens::dark();
        assert_ne!(
            level_colors(&d, Some(Severity::Error)).0,
            level_colors(&d, Some(Severity::Warning)).0
        );
    }
}
