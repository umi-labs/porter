use crate::msg::{Cmd, DataMsg, Msg};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use porter_engine::session::{Overview, RunInfo, Session};
use std::collections::VecDeque;
use std::sync::Arc;

const LOG_CAP: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Project,
    Stages,
    Issues,
    Items,
}

impl Panel {
    const ALL: [Panel; 4] = [Panel::Project, Panel::Stages, Panel::Issues, Panel::Items];

    fn step(self, by: usize) -> Panel {
        let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Self::ALL[(i + by) % Self::ALL.len()]
    }
}

/// Per-panel state; Tasks 3-5 fill these in.
#[derive(Default)]
pub struct Panels {
    pub overview: Option<Overview>,
}

#[derive(Debug, PartialEq)]
pub enum Modal {
    ConfirmQuit,
}

pub struct LogLine(pub String);

pub struct App {
    /// None when the folder has no porter.toml.
    pub session: Option<Arc<Session>>,
    pub focus: Panel,
    pub panels: Panels,
    pub modal: Option<Modal>,
    pub log: VecDeque<LogLine>,
    pub running: Option<RunInfo>,
    pub size: (u16, u16),
    pub quit: bool,
    /// Set by `update`; the runtime redraws and clears it.
    pub dirty: bool,
}

impl App {
    pub fn new(session: Option<Arc<Session>>, size: (u16, u16)) -> App {
        App {
            session,
            focus: Panel::Project,
            panels: Panels::default(),
            modal: None,
            log: VecDeque::new(),
            running: None,
            size,
            quit: false,
            dirty: true,
        }
    }

    fn push_log(&mut self, s: String) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(LogLine(s));
    }
}

/// Pure: no I/O. Returns the commands for the runtime to execute.
pub fn update(app: &mut App, msg: Msg) -> Vec<Cmd> {
    match msg {
        Msg::Tick => return vec![],
        Msg::Resize(w, h) => app.size = (w, h),
        Msg::Engine(e) => app.push_log(format!("{e:?}")),
        Msg::Data(DataMsg::Overview(o)) => app.panels.overview = Some(*o),
        Msg::Data(DataMsg::Error(e)) => app.push_log(format!("error: {e}")),
        Msg::RunDone(_) => app.running = None,
        Msg::Key(k) => {
            app.dirty = true;
            return key(app, k);
        }
    }
    app.dirty = true;
    vec![]
}

fn key(app: &mut App, k: KeyEvent) -> Vec<Cmd> {
    if k.kind == KeyEventKind::Release {
        return vec![];
    }
    if app.modal == Some(Modal::ConfirmQuit) {
        return match k.code {
            KeyCode::Char('y') => {
                app.quit = true;
                vec![Cmd::Cancel, Cmd::Quit]
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                app.modal = None;
                vec![]
            }
            _ => vec![],
        };
    }
    match k.code {
        KeyCode::Tab => app.focus = app.focus.step(1),
        KeyCode::BackTab => app.focus = app.focus.step(3),
        KeyCode::Char('1') => app.focus = Panel::Project,
        KeyCode::Char('2') => app.focus = Panel::Stages,
        KeyCode::Char('3') => app.focus = Panel::Issues,
        KeyCode::Char('4') => app.focus = Panel::Items,
        KeyCode::Char('q') => return quit(app),
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return quit(app),
        _ => {}
    }
    vec![]
}

fn quit(app: &mut App) -> Vec<Cmd> {
    if app.running.is_some() {
        app.modal = Some(Modal::ConfirmQuit);
        vec![]
    } else {
        app.quit = true;
        vec![Cmd::Quit]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_engine::session::{RunPlan, StageOpts};
    use porter_engine::stage::SchemaMode;

    fn press(app: &mut App, c: KeyCode) -> Vec<Cmd> {
        update(app, Msg::Key(KeyEvent::new(c, KeyModifiers::NONE)))
    }

    #[test]
    fn focus_cycles_with_tab() {
        let mut app = App::new(None, (80, 24));
        let mut seen = vec![app.focus];
        for _ in 0..4 {
            press(&mut app, KeyCode::Tab);
            seen.push(app.focus);
        }
        assert_eq!(
            seen,
            [
                Panel::Project,
                Panel::Stages,
                Panel::Issues,
                Panel::Items,
                Panel::Project
            ]
        );
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, Panel::Items);
    }

    #[test]
    fn quit_when_idle_sets_quit() {
        let mut app = App::new(None, (80, 24));
        let cmds = press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
        assert_eq!(cmds, [Cmd::Quit]);
    }

    #[test]
    fn quit_while_running_opens_confirm() {
        let mut app = App::new(None, (80, 24));
        app.running = Some(RunInfo {
            plan: RunPlan::Stage(
                porter_model::Stage::Load,
                StageOpts {
                    refresh: false,
                    apply: false,
                    schema_mode: SchemaMode::Check,
                },
            ),
        });
        assert!(press(&mut app, KeyCode::Char('q')).is_empty());
        assert!(!app.quit);
        assert_eq!(app.modal, Some(Modal::ConfirmQuit));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.modal, None);
        press(&mut app, KeyCode::Char('q'));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(app.quit);
        assert_eq!(cmds, [Cmd::Cancel, Cmd::Quit]);
    }
}
