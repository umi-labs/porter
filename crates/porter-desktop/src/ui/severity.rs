//! Issue severity: error (red circle), warning (amber square), info (grey ring).

use gpui::{App, FontWeight, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px};
use porter_model::Severity;

use crate::theme::{radius, tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Circle,
    Square,
    Ring,
}

pub fn severity_look(s: Severity) -> (&'static str, Mark) {
    match s {
        Severity::Error => ("error", Mark::Circle),
        Severity::Warning => ("warning", Mark::Square),
        Severity::Info => ("info", Mark::Ring),
    }
}

#[derive(IntoElement)]
pub struct SeverityChip(pub Severity);

impl RenderOnce for SeverityChip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = tokens(cx);
        let (word, mark) = severity_look(self.0);
        let (fg, bg) = match self.0 {
            Severity::Error => (t.err, t.err_bg),
            Severity::Warning => (t.warn, t.warn_bg),
            Severity::Info => (t.muted, t.panel),
        };
        let dot = div().flex_none().size(px(8.));
        let dot = match mark {
            Mark::Circle => dot.rounded_full().bg(fg),
            Mark::Square => dot.rounded(px(2.)).bg(fg),
            Mark::Ring => dot.rounded_full().border(px(1.5)).border_color(fg),
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(20.))
            .pl(px(5.))
            .pr(px(7.))
            .rounded(radius::XS)
            .bg(bg)
            .text_color(fg)
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(dot)
            .child(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severities_differ_by_shape_not_only_colour() {
        let marks = [Severity::Error, Severity::Warning, Severity::Info].map(severity_look);
        assert_eq!(marks[0].1, Mark::Circle);
        assert_eq!(marks[1].1, Mark::Square);
        assert_eq!(marks[2].1, Mark::Ring);
        assert_eq!(marks.map(|m| m.0), ["error", "warning", "info"]);
    }
}
