use crate::app::{App, Modal, Panel};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub const MIN_SIZE: (u16, u16) = (80, 24);

/// Accent for the focused border; plain bold when NO_COLOR is set.
fn accent() -> Style {
    if std::env::var_os("NO_COLOR").is_some() {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    }
}

fn panel(title: &str, focused: bool) -> Block<'_> {
    let b = Block::default().borders(Borders::ALL).title(title);
    if focused { b.border_style(accent()) } else { b }
}

pub fn view(app: &App, f: &mut Frame) {
    let area = f.area();
    if app.session.is_none() {
        f.render_widget(
            Paragraph::new("no porter project here — setup coming in Task 7\n\nq quit"),
            area,
        );
        return;
    }
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        f.render_widget(Paragraph::new("terminal too small (needs 80x24)"), area);
        return;
    }
    let [main, log, footer] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(area);
    let [left, detail] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(main);
    let [project, stages, issues, items] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(8),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ])
    .areas(left);

    let focus = |p| app.focus == p;
    f.render_widget(panel("Project", focus(Panel::Project)), project);
    f.render_widget(panel("Stages", focus(Panel::Stages)), stages);
    f.render_widget(panel("Issues", focus(Panel::Issues)), issues);
    f.render_widget(panel("Items", focus(Panel::Items)), items);
    f.render_widget(panel("Detail", false), detail);
    f.render_widget(panel("Log", false), log);
    f.render_widget(
        Paragraph::new("tab focus · q quit").style(Style::default().add_modifier(Modifier::DIM)),
        footer,
    );

    if app.modal == Some(Modal::ConfirmQuit) {
        let r = centered(area, 44, 5);
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new("A stage is running. Cancel it and quit?\n\ny yes · n no")
                .block(panel("Quit", true)),
            r,
        );
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let [_, v, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(h.min(area.height)),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, r, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(w.min(area.width)),
        Constraint::Fill(1),
    ])
    .areas(v);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_engine::session::Session;
    use ratatui::{Terminal, backend::TestBackend};
    use std::sync::Arc;

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| view(app, f)).unwrap();
        t.backend().to_string()
    }

    #[test]
    fn four_panels_at_80x24() {
        let d = tempfile::tempdir().unwrap();
        porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let s = Session::open(d.path()).unwrap();
        let app = App::new(Some(Arc::new(s)), (80, 24));
        insta::assert_snapshot!(render(&app, 80, 24));
    }

    #[test]
    fn no_project_placeholder() {
        let app = App::new(None, (80, 24));
        assert!(render(&app, 80, 24).contains("setup coming in Task 7"));
    }
}
