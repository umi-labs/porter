use crate::actions;
use crate::app::{App, update};
use crate::lists::{ALL_KINDS, Group, Mode, issue_filter};
use crate::msg::{Cmd, DataMsg, Msg, RefreshScope};
use crate::setup::{self, Setup};
use crate::{term, ui};
use anyhow::Result;
use crossterm::event::{Event as TermEvent, EventStream};
use futures::StreamExt;
use porter_engine::events::Event;
use porter_engine::session::{IssueQuery, Page, RunOutcome, Session};
use porter_model::Severity;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{
    broadcast::{
        Receiver,
        error::{RecvError, TryRecvError},
    },
    mpsc,
};
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout};
use tokio_util::sync::CancellationToken;

/// Runs the terminal UI on the project in `dir`; returns the process exit code.
pub async fn run_tui(dir: &Path) -> Result<i32> {
    // Open before touching the terminal so errors print normally.
    let session = if dir.join("porter.toml").exists() {
        Some(Arc::new(Session::open(dir)?))
    } else {
        None
    };
    term::install_panic_hook();
    let mut sigterm = Sigterm::new()?;
    // The guard lives only in this block: the terminal is restored before any note prints.
    let (code, note) = {
        let _guard = term::Guard::enter()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
        let size = crossterm::terminal::size().unwrap_or((80, 24));

        let mut app = App::new(session.clone(), size);
        if session.is_none() {
            let name = std::fs::canonicalize(dir)
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            app.setup = Some(Setup::new(&name));
        }
        let mut engine = session.as_ref().map(|s| s.subscribe());
        let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
        let mut events = EventStream::new();
        let mut tick = interval(Duration::from_millis(100));
        let mut run = RunSlot::default();
        let mut cmds = app.refresh_cmds();

        loop {
            let mut quit = false;
            for c in std::mem::take(&mut cmds) {
                if matches!(c, Cmd::Quit) {
                    quit = true;
                } else if matches!(c, Cmd::OpenEditor) {
                    // Drop the stream so the editor, not us, reads the keyboard.
                    drop(events);
                    let msgs = edit_config(&app, &mut terminal);
                    events = EventStream::new();
                    if let Some(m) = msgs {
                        let _ = tx.send(Msg::Data(m));
                    }
                    app.dirty = true;
                } else {
                    exec(c, &app, &tx, &mut run, dir);
                }
            }
            if quit {
                let code = if app.terminated { 143 } else { 0 };
                break (code, finish_run(&mut run, &mut app, &mut terminal).await?);
            }
            if app.dirty {
                terminal.draw(|f| ui::view(&app, f))?;
                app.dirty = false;
            }
            let msg = tokio::select! {
                // Through `update`, so it cancels and awaits the run like `q`.
                _ = sigterm.recv() => Msg::Term,
                Some(Ok(ev)) = events.next() => match ev {
                    TermEvent::Key(k) => Msg::Key(k),
                    TermEvent::Resize(w, h) => Msg::Resize(w, h),
                    _ => continue,
                },
                r = async {
                    match engine.as_mut() {
                        Some(r) => r.recv().await,
                        None => std::future::pending().await,
                    }
                } => match r {
                    Ok(e) => Msg::Engine(e),
                    Err(RecvError::Lagged(n)) => Msg::Lagged(n),
                    Err(RecvError::Closed) => { engine = None; continue }
                },
                Some(m) = rx.recv() => m,
                _ = tick.tick() => Msg::Tick,
            };
            // The project now exists: follow its events.
            if let Msg::Data(DataMsg::SetupSaved(Ok(s))) = &msg {
                engine = Some(s.subscribe());
            }
            if matches!(msg, Msg::RunDone(_)) {
                run = RunSlot::default();
            }
            cmds = deliver(&mut app, engine.as_mut(), msg);
        }
    };
    if let Some(n) = note {
        eprintln!("{n}");
    }
    Ok(code)
}

/// Applies `msg`. RunDone travels on its own channel, so it can overtake the run's own
/// engine events; those are already queued by then, so drain them first and the stage
/// never shows running after its run ended.
fn deliver(app: &mut App, engine: Option<&mut Receiver<Event>>, msg: Msg) -> Vec<Cmd> {
    let mut cmds = vec![];
    if let (Msg::RunDone(_), Some(rx)) = (&msg, engine) {
        loop {
            let m = match rx.try_recv() {
                Ok(e) => Msg::Engine(e),
                Err(TryRecvError::Lagged(n)) => Msg::Lagged(n),
                Err(_) => break,
            };
            cmds.extend(update(app, m));
        }
    }
    cmds.extend(update(app, msg));
    cmds
}

/// SIGTERM on unix. Windows has no SIGTERM (a console close kills the process), so there
/// it never fires.
#[cfg(unix)]
struct Sigterm(tokio::signal::unix::Signal);

#[cfg(unix)]
impl Sigterm {
    fn new() -> std::io::Result<Sigterm> {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::terminate()).map(Sigterm)
    }

    async fn recv(&mut self) {
        if self.0.recv().await.is_none() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(not(unix))]
struct Sigterm;

#[cfg(not(unix))]
impl Sigterm {
    fn new() -> std::io::Result<Sigterm> {
        Ok(Sigterm)
    }

    async fn recv(&mut self) {
        std::future::pending::<()>().await;
    }
}

/// Suspends the TUI, runs the editor on porter.toml, restores the TUI, then reloads the
/// config. None when there is no project.
fn edit_config(
    app: &App,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> Option<DataMsg> {
    let session = app.session.as_ref()?;
    term::restore();
    let edited = actions::run_editor(
        &actions::editor_command(),
        &session.project().dir().join("porter.toml"),
    );
    let resumed = term::resume().and_then(|_| terminal.clear());
    let mut msgs = actions::reload_after_edit(session, edited);
    if let Err(e) = resumed {
        msgs.push(format!("could not restore the terminal: {e}"));
    }
    Some(DataMsg::Config(msgs))
}

/// The run in flight: its cancel token and the task that waits for it.
#[derive(Default)]
pub(crate) struct RunSlot {
    pub(crate) cancel: Option<CancellationToken>,
    join: Option<JoinHandle<()>>,
}

const QUIT_WAIT: Duration = Duration::from_secs(10);

/// Cancels the run and waits for it to stop, so quitting never leaves a half-written
/// stage behind. Returns a note for stderr if it did not stop in time.
async fn finish_run<B: ratatui::backend::Backend>(
    run: &mut RunSlot,
    app: &mut App,
    terminal: &mut Terminal<B>,
) -> Result<Option<String>>
where
    B::Error: Send + Sync + 'static,
{
    let Some(join) = run.join.take() else {
        return Ok(None);
    };
    if let Some(c) = &run.cancel {
        c.cancel();
    }
    app.push_log("cancelling…".into());
    terminal.draw(|f| ui::view(app, f))?;
    Ok(match timeout(QUIT_WAIT, join).await {
        Ok(_) => None,
        Err(_) => Some(
            "porter: the running stage did not stop within 10s; exiting anyway (it can be resumed)"
                .into(),
        ),
    })
}

pub(crate) fn exec(
    cmd: Cmd,
    app: &App,
    tx: &mpsc::UnboundedSender<Msg>,
    run: &mut RunSlot,
    dir: &Path,
) {
    if let Cmd::SaveSetup(values) = cmd {
        let (tx, dir) = (tx.clone(), dir.to_path_buf());
        tokio::spawn(async move {
            let r = tokio::task::spawn_blocking(move || {
                setup::save(&dir, &values)?;
                Session::open(&dir)
                    .map(Arc::new)
                    .map_err(|e| setup::Failure::general(format!("{e:#}")))
            })
            .await
            .unwrap_or_else(|e| Err(setup::Failure::general(e.to_string())));
            let _ = tx.send(Msg::Data(DataMsg::SetupSaved(r)));
        });
        return;
    }
    let Some(session) = app.session.clone() else {
        return;
    };
    match cmd {
        Cmd::Run(plan) => match session.run(plan) {
            Ok(h) => {
                run.cancel = Some(h.cancel_token());
                let tx = tx.clone();
                run.join = Some(tokio::spawn(async move {
                    let _ = tx.send(Msg::RunDone(h.wait().await));
                }));
            }
            // Surface as a finished run so the UI leaves its running state.
            Err(e) => {
                let _ = tx.send(Msg::RunDone(RunOutcome {
                    code: 1,
                    messages: vec![format!("{e:#}")],
                    ..Default::default()
                }));
            }
        },
        Cmd::Cancel => {
            if let Some(c) = &run.cancel {
                c.cancel();
            }
        }
        Cmd::Refresh(RefreshScope::All) => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let m = match session.overview().await {
                    Ok(o) => DataMsg::Overview(Box::new(o)),
                    Err(e) => DataMsg::OverviewFailed(format!("{e:#}")),
                };
                let _ = tx.send(Msg::Data(m));
            });
        }
        Cmd::Items { q, page, seq } => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let m = match session.items(q, page).await {
                    Ok(paged) => DataMsg::Items { seq, page, paged },
                    Err(e) => DataMsg::Error(format!("{e:#}")),
                };
                let _ = tx.send(Msg::Data(m));
            });
        }
        Cmd::Issues { q, page, seq } => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let m = match session.issues(q, page).await {
                    Ok(paged) => DataMsg::IssuePage { seq, page, paged },
                    Err(e) => DataMsg::Error(format!("{e:#}")),
                };
                let _ = tx.send(Msg::Data(m));
            });
        }
        Cmd::IssueGroups { mode, seq } => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let m = match issue_groups(&session, mode).await {
                    Ok(groups) => DataMsg::IssueGroups { seq, groups },
                    Err(e) => DataMsg::Error(format!("{e:#}")),
                };
                let _ = tx.send(Msg::Data(m));
            });
        }
        Cmd::Item { id, seq } => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let m = match session.item(id).await {
                    Ok(d) => DataMsg::Item {
                        seq,
                        detail: Box::new(d),
                    },
                    Err(e) => DataMsg::Error(format!("{e:#}")),
                };
                let _ = tx.send(Msg::Data(m));
            });
        }
        Cmd::Resolve(id, r) => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let r2 = r.clone();
                let res = tokio::task::spawn_blocking(move || session.resolve_issue(id, r2)).await;
                let m = match res {
                    Ok(Ok(())) => Ok((id, r)),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(Msg::Data(DataMsg::Resolved(m)));
            });
        }
        Cmd::PreviewCounts => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let c = tokio::task::spawn_blocking(move || actions::preview_info(&session))
                    .await
                    .unwrap_or(None);
                let _ = tx.send(Msg::Data(DataMsg::Preview(c)));
            });
        }
        Cmd::RecheckApply => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let c = tokio::task::spawn_blocking(move || actions::preview_info(&session))
                    .await
                    .unwrap_or(None);
                let _ = tx.send(Msg::Data(DataMsg::ApplyRechecked(c)));
            });
        }
        Cmd::OpenReport(name) => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let r = tokio::task::spawn_blocking(move || actions::open_report(&session, &name))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                let _ = tx.send(Msg::Data(DataMsg::Reported(r)));
            });
        }
        Cmd::Copy(id) => {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = out.write_all(actions::osc52(&id).as_bytes());
            let _ = out.flush();
        }
        // Handled in the loop, which owns the terminal.
        Cmd::OpenEditor | Cmd::Quit | Cmd::SaveSetup(_) => {}
    }
}

/// Issue counts per kind with the worst severity in each. One count query per kind, then
/// severity probes of one row each, so nothing proportional to the issue count is held.
async fn issue_groups(s: &Session, mode: Mode) -> Result<Vec<Group>> {
    let one = Page {
        offset: 0,
        limit: 1,
    };
    let mut out = vec![];
    for kind in ALL_KINDS {
        let q = issue_filter(mode, Some(kind));
        let total = s.issues(q.clone(), one).await?.total;
        if total == 0 {
            continue;
        }
        let mut worst = Severity::Info;
        for sev in [Severity::Error, Severity::Warning] {
            let probe = IssueQuery {
                severity: Some(sev),
                ..q.clone()
            };
            if s.issues(probe, one).await?.total > 0 {
                worst = sev;
                break;
            }
        }
        out.push(Group { kind, total, worst });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn finish_run_cancels_and_waits_for_the_run() {
        let cancel = CancellationToken::new();
        let stopped = Arc::new(AtomicBool::new(false));
        let (c, s) = (cancel.clone(), stopped.clone());
        let mut run = RunSlot {
            cancel: Some(cancel.clone()),
            join: Some(tokio::spawn(async move {
                c.cancelled().await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                s.store(true, Ordering::SeqCst);
            })),
        };
        let mut app = App::new(None, (80, 24));
        let mut t = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let note = finish_run(&mut run, &mut app, &mut t).await.unwrap();
        assert!(note.is_none() && cancel.is_cancelled());
        assert!(
            stopped.load(Ordering::SeqCst),
            "returned before the run stopped"
        );
    }

    #[tokio::test]
    async fn run_done_is_ordered_after_the_runs_own_events() {
        use porter_engine::events::Event;
        use porter_engine::store::RunId;
        let d = tempfile::tempdir().unwrap();
        porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
        let mut app = App::new(Some(Arc::new(Session::open(d.path()).unwrap())), (120, 40));
        update(
            &mut app,
            Msg::Key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('R'),
            )),
        );
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        tx.send(Event::StageStarted {
            stage: porter_model::Stage::Load,
            run: RunId(uuid::Uuid::nil()),
        })
        .unwrap();
        // RunDone arrives on its own channel, possibly before the event is received.
        deliver(&mut app, Some(&mut rx), Msg::RunDone(Default::default()));
        assert!(
            rx.try_recv().is_err(),
            "RunDone overtook a queued run event"
        );
        assert!(app.running.is_none());
        assert!(app.panels.stages.running.is_none());
    }
}
