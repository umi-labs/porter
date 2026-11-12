use super::panel;
use crate::app::App;
use ratatui::{Frame, layout::Rect, text::Line, widgets::Paragraph};

/// The newest lines that fit; the buffer itself is bounded by `LOG_CAP`.
pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let block = panel("Log", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let tail: Vec<Line> = app
        .log
        .iter()
        .rev()
        .take(inner.height as usize)
        .rev()
        .map(|l| Line::raw(l.0.as_str()))
        .collect();
    f.render_widget(Paragraph::new(tail), inner);
}
