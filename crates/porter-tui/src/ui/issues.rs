use super::{filter, panel, tint, window};
use crate::app::{App, Panel};
use crate::lists::kind_name;
use porter_model::Severity;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub fn sev_glyph(s: Severity) -> (&'static str, Option<Color>) {
    match s {
        Severity::Error => ("✗", Some(Color::Red)),
        Severity::Warning => ("⚠", Some(Color::Yellow)),
        Severity::Info => ("i", None),
    }
}

pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let st = &app.panels.issues;
    let focused = app.focus == Panel::Issues;
    let title = filter::title("Issues", st.mode, &st.filter);
    let block = panel(&title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let shown = st.shown();
    if shown.is_empty() {
        f.render_widget(
            Paragraph::new("no issues").style(Style::default().add_modifier(Modifier::DIM)),
            inner,
        );
        return;
    }
    let range = window(st.sel, shown.len(), inner.height as usize);
    let start = range.start;
    let lines: Vec<Line> = shown[range]
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let sel = focused && i + start == st.sel;
            let (glyph, c) = sev_glyph(g.worst);
            let mut line = Line::from(vec![
                Span::raw(if sel { "> " } else { "  " }),
                Span::styled(glyph, tint(c)),
                Span::raw(format!(" {:<20} {}", kind_name(g.kind), g.total)),
            ]);
            if sel {
                line = line.style(super::accent());
            }
            line
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
