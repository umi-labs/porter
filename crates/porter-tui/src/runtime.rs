use crate::app::{App, update};
use crate::msg::{Cmd, DataMsg, Msg, RefreshScope};
use crate::{term, ui};
use anyhow::Result;
use crossterm::event::{Event as TermEvent, EventStream};
use futures::StreamExt;
use porter_engine::session::Session;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast::error::RecvError, mpsc};
use tokio::time::interval;
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
    let _guard = term::Guard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let size = crossterm::terminal::size().unwrap_or((80, 24));

    let mut app = App::new(session.clone(), size);
    let mut engine = session.as_ref().map(|s| s.subscribe());
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(100));
    let mut cancel: Option<CancellationToken> = None;
    let mut cmds = vec![Cmd::Refresh(RefreshScope::All)];

    loop {
        for c in std::mem::take(&mut cmds) {
            if matches!(c, Cmd::Quit) {
                return Ok(0);
            }
            exec(c, &app, &tx, &mut cancel);
        }
        if app.dirty {
            terminal.draw(|f| ui::view(&app, f))?;
            app.dirty = false;
        }
        let msg = tokio::select! {
            _ = sigterm.recv() => return Ok(143),
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
            cancel = None;
        }
        cmds = update(&mut app, msg);
    }
}

fn exec(
    cmd: Cmd,
    app: &App,
    tx: &mpsc::UnboundedSender<Msg>,
    cancel: &mut Option<CancellationToken>,
) {
    let Some(session) = app.session.clone() else {
        return;
    };
    match cmd {
        Cmd::Run(plan) => match session.run(plan) {
            Ok(h) => {
                *cancel = Some(h.cancel_token());
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(Msg::RunDone(h.wait().await));
                });
            }
            Err(e) => {
                let _ = tx.send(Msg::Data(DataMsg::Error(format!("{e:#}"))));
            }
        },
        Cmd::Cancel => {
            if let Some(c) = cancel {
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
