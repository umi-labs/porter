use super::panel;
use crate::app::{App, Panel};
use ratatui::{Frame, layout::Rect, widgets::Paragraph};

pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let block = panel("Project", app.focus == Panel::Project);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(s) = &app.session else { return };
    let c = s.config();
    let ov = app.panels.overview.clone().unwrap_or_default();
    let text = format!(
        "{}  {} → {}\n{} err · {} warn · {} info",
        c.project.name,
        c.source.adapter,
        c.destination.adapter,
        ov.open_errors,
        ov.open_warnings,
        ov.open_infos
    );
    f.render_widget(Paragraph::new(text), inner);
}
