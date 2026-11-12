use crate::app::{App, update};
use crate::msg::{Cmd, DataMsg, Msg, RefreshScope};
use crate::{term, ui};
use anyhow::Result;
use crossterm::event::{Event as TermEvent, EventStream};
use futures::StreamExt;
use porter_engine::session::{RunOutcome, Session};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast::error::RecvError, mpsc};
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
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    // The guard lives only in this block: the terminal is restored before any note prints.
    let (code, note) = {
        let _guard = term::Guard::enter()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
        let size = crossterm::terminal::size().unwrap_or((80, 24));

        let mut app = App::new(session.clone(), size);
        let mut engine = session.as_ref().map(|s| s.subscribe());
        let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
        let mut events = EventStream::new();
        let mut tick = interval(Duration::from_millis(100));
        let mut run = RunSlot::default();
        let mut cmds = vec![Cmd::Refresh(RefreshScope::All)];

        loop {
            let mut quit = false;
            for c in std::mem::take(&mut cmds) {
                if matches!(c, Cmd::Quit) {
                    quit = true;
                } else {
                    exec(c, &app, &tx, &mut run);
                }
            }
            if quit {
                break (0, finish_run(&mut run, &mut app, &mut terminal).await?);
            }
            if app.dirty {
                terminal.draw(|f| ui::view(&app, f))?;
                app.dirty = false;
            }
            let msg = tokio::select! {
                _ = sigterm.recv() => {
                    if let Some(c) = &run.cancel { c.cancel() }
                    break (143, None)
                }
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
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => { engine = None; continue }
                },
                Some(m) = rx.recv() => m,
                _ = tick.tick() => Msg::Tick,
            };
            if matches!(msg, Msg::RunDone(_)) {
                run = RunSlot::default();
            }
            cmds = update(&mut app, msg);
        }
    };
    if let Some(n) = note {
        eprintln!("{n}");
    }
    Ok(code)
}

/// The run in flight: its cancel token and the task that waits for it.
#[derive(Default)]
struct RunSlot {
    cancel: Option<CancellationToken>,
    join: Option<JoinHandle<()>>,
}

const QUIT_WAIT: Duration = Duration::from_secs(10);

/// Cancels the run and waits for it to stop, so quitting never leaves a half-written
/// stage behind. Returns a note for stderr if it did not stop in time.
async fn finish_run(
    run: &mut RunSlot,
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> Result<Option<String>> {
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

fn exec(cmd: Cmd, app: &App, tx: &mpsc::UnboundedSender<Msg>, run: &mut RunSlot) {
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
                    Err(e) => DataMsg::Error(format!("{e:#}")),
                };
                let _ = tx.send(Msg::Data(m));
            });
        }
        // Wired up with the panels in later tasks.
        Cmd::OpenEditor | Cmd::OpenReport(_) | Cmd::Copy(_) | Cmd::Quit => {}
    }
}
