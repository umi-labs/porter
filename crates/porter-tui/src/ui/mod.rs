use crate::app::{App, Modal, Panel};
use ratatui::text::Line;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
};

mod detail;
mod project;
pub mod stages;

pub const MIN_SIZE: (u16, u16) = (80, 24);

/// Accent for the focused border; plain bold when NO_COLOR is set.
fn accent() -> Style {
    if std::env::var_os("NO_COLOR").is_some() {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    }
}

/// Colour for a state glyph; nothing when NO_COLOR is set.
fn tint(c: Option<Color>) -> Style {
    match c {
        Some(c) if std::env::var_os("NO_COLOR").is_none() => Style::default().fg(c),
        _ => Style::default(),
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
    self::project::render(app, f, project);
    self::stages::render(app, f, stages);
    f.render_widget(panel("Issues", focus(Panel::Issues)), issues);
    f.render_widget(panel("Items", focus(Panel::Items)), items);
    self::detail::render(app, f, detail);
    let log_block = panel("Log", false);
    let inner = log_block.inner(log);
    f.render_widget(log_block, log);
    let n = inner.height as usize;
    let tail: Vec<Line> = app
        .log
        .iter()
        .rev()
        .take(n)
        .rev()
        .map(|l| Line::raw(l.0.as_str()))
        .collect();
    f.render_widget(Paragraph::new(tail), inner);
    f.render_widget(
        Paragraph::new("tab focus · space run · R all · v verify · c cancel · q quit")
            .style(Style::default().add_modifier(Modifier::DIM)),
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
    use crate::app::update;
    use crate::msg::Msg;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use porter_engine::events::{Event, Summary};
    use porter_engine::session::{Overview, RunOutcome, Session};
    use porter_engine::store::{RunId, RunRow};
    use porter_model::Stage;
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

    fn stages_panel(app: &App) -> String {
        let mut t = Terminal::new(TestBackend::new(42, 8)).unwrap();
        t.draw(|f| stages::render(app, f, f.area())).unwrap();
        t.backend().to_string()
    }

    /// An app at 120x40 whose Extract stage has the given last run / live state.
    fn app_with(summary: Option<Summary>, ran: bool) -> (App, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let mut app = App::new(Some(Arc::new(Session::open(d.path()).unwrap())), (120, 40));
        app.focus = Panel::Stages;
        let mut ov = Overview::default();
        if ran {
            ov.last_runs.push((
                Stage::Extract,
                RunRow {
                    id: "r".into(),
                    porter_version: "2.0.0".into(),
                    started_at: "2026-11-12T10:00:00Z".into(),
                    ended_at: summary.as_ref().map(|_| "2026-11-12T10:01:00Z".into()),
                    summary,
                },
            ));
        }
        app.panels.overview = Some(ov);
        (app, d)
    }

    fn sum(f: fn(&mut Summary)) -> Option<Summary> {
        let mut s = Summary {
            done: 10,
            ..Default::default()
        };
        f(&mut s);
        Some(s)
    }

    #[test]
    fn stages_never_run() {
        let (app, _d) = app_with(None, false);
        insta::assert_snapshot!(stages_panel(&app));
    }

    #[test]
    fn stages_clean() {
        let (app, _d) = app_with(sum(|_| {}), true);
        insta::assert_snapshot!(stages_panel(&app));
    }

    #[test]
    fn stages_warnings() {
        let (app, _d) = app_with(sum(|s| s.warnings = 2), true);
        insta::assert_snapshot!(stages_panel(&app));
    }

    #[test]
    fn stages_errors_never_a_tick() {
        let (app, _d) = app_with(sum(|s| s.errors = 1), true);
        let out = stages_panel(&app);
        assert!(!out.contains('✓') && out.contains('✗'));
        insta::assert_snapshot!(out);
    }

    #[test]
    fn stages_failed_items() {
        let (app, _d) = app_with(sum(|s| s.failed = 3), true);
        insta::assert_snapshot!(stages_panel(&app));
    }

    #[test]
    fn stages_interrupted() {
        let (app, _d) = app_with(sum(|s| s.interrupted = true), true);
        insta::assert_snapshot!(stages_panel(&app));
    }

    #[test]
    fn stages_could_not_run() {
        let (mut app, _d) = app_with(sum(|_| {}), true);
        app.running = Some(porter_engine::session::RunInfo {
            plan: porter_engine::session::RunPlan::All {
                keep_going: false,
                apply: false,
            },
        });
        update(
            &mut app,
            Msg::RunDone(RunOutcome {
                code: 1,
                per_stage: vec![(Stage::Extract, 1, Some("no such file".into()))],
                ..Default::default()
            }),
        );
        let out = stages_panel(&app);
        assert!(
            !out.contains('✓'),
            "a stage that could not run is not clean"
        );
        insta::assert_snapshot!(out);
    }

    #[test]
    fn stages_running_with_progress() {
        let (mut app, _d) = app_with(None, false);
        update(
            &mut app,
            Msg::Engine(Event::StageStarted {
                stage: Stage::Extract,
                run: RunId(uuid::Uuid::nil()),
            }),
        );
        update(
            &mut app,
            Msg::Engine(Event::Progress {
                stage: Stage::Extract,
                done: 120,
                total: Some(5000),
                label: "posts".into(),
            }),
        );
        insta::assert_snapshot!(stages_panel(&app));
    }

    #[test]
    fn full_screen_running_120x40() {
        let (mut app, _d) = app_with(sum(|s| s.warnings = 1), true);
        update(
            &mut app,
            Msg::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
        );
        update(
            &mut app,
            Msg::Engine(Event::Progress {
                stage: Stage::Analyse,
                done: 7,
                total: None,
                label: "files".into(),
            }),
        );
        insta::assert_snapshot!(render(&app, 120, 40));
    }
}
