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
        .map(|b| Line::raw(format!("{:<9} {:<38} {}", b.shown, b.what, b.scope.name())))
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
