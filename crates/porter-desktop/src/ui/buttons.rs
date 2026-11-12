//! The three button looks. Load (dry run) is the primary action; Apply is never primary:
//! it is a dashed ghost with a lock.

use gpui::prelude::*;
use gpui::{App, ElementId, FontWeight, ParentElement, SharedString, Stateful, Styled, div, px};

use crate::theme::{radius, tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Accent fill: the one main action of a view.
    Primary,
    /// Outlined.
    Secondary,
    /// Dashed ghost with a lock: Apply load…
    ApplyGhost,
}

/// A 32px button; the caller chains `.on_click(..)`. Kept a plain element rather than
/// gpui-component's `Button`, whose fixed height clips and whose look is not the design's.
pub fn button(
    id: impl Into<ElementId>,
    kind: Kind,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<gpui::Div> {
    let t = tokens(cx);
    let base = div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(32.))
        .px(px(10.))
        .rounded(radius::SM)
        .border_1()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .whitespace_nowrap();
    let label = label.into();
    match kind {
        Kind::Primary => base
            .border_color(t.accent)
            .bg(t.accent)
            .text_color(t.on_accent)
            .hover(|s| s.opacity(0.9))
            .child(label),
        Kind::Secondary => base
            .border_color(t.border_strong)
            .bg(t.bg)
            .text_color(t.text)
            .hover(|s| s.bg(t.hover))
            .child(label),
        Kind::ApplyGhost => base
            .border_dashed()
            .border_color(t.border_strong)
            .text_color(t.muted)
            .hover(|s| s.bg(t.hover))
            .child("🔒")
            .child(label),
    }
}
