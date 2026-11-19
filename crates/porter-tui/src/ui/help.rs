use super::{centered, panel};
use crate::app::App;
use crate::keymap::BINDINGS;
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Clear, Paragraph},
};

/// Every binding in the keymap, one per line; j/k scroll when the terminal is short.
pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let r = centered(area, 78, BINDINGS.len() as u16 + 2);
    f.render_widget(Clear, r);
    let block = panel("Keys (j/k scroll, esc closes)", true);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let max = BINDINGS.len().saturating_sub(inner.height as usize);
    let lines: Vec<Line> = BINDINGS
        .iter()
        .skip(app.help_scroll.min(max))
        .map(|b| Line::raw(line(b)))
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

/// Wide enough for the longest description; the panel (78 wide) still fits the scope.
const WHAT: usize = 44;

/// One help row; a long description is cut with … so it never runs into the scope column.
fn line(b: &crate::keymap::Binding) -> String {
    let what = if b.what.chars().count() > WHAT {
        let cut: String = b.what.chars().take(WHAT - 1).collect();
        format!("{cut}…")
    } else {
        b.what.to_string()
    };
    format!("{:<9} {what:<WHAT$} {}", b.shown, b.scope.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_column_lines_up_for_every_binding() {
        for b in BINDINGS {
            let l = line(b);
            let scope: String = l.chars().skip(9 + 1 + WHAT + 1).collect();
            assert_eq!(scope, b.scope.name(), "{l}");
            assert!(b.what.chars().count() <= WHAT, "{} would be cut", b.what);
        }
    }
}
