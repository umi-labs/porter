//! Output discipline: with `--json` stdout carries only NDJSON; everything human
//! (event lines, logs, errors) goes to stderr.

use porter_engine::events::Event;
use serde::Serialize;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

pub fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => tracing::Level::WARN,
        1 => tracing::Level::INFO,
        2 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Wire string of a unit enum, e.g. `Stage::Load` -> `load`.
pub fn wire<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Object(m)) => m.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

/// One JSON line on stdout.
pub fn print_json(v: &impl Serialize) {
    use std::io::Write;
    if let Ok(line) = serde_json::to_string(v) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
    }
}

fn render(e: &Event) -> String {
    match e {
        Event::StageStarted { stage, .. } => format!("{}: started", wire(stage)),
        Event::Progress {
            stage,
            done,
            total,
            label,
        } => match total {
            Some(t) => format!("{}: {done}/{t} {label}", wire(stage)),
            None => format!("{}: {done} {label}", wire(stage)),
        },
        Event::ItemResult {
            stage,
            item,
            kind,
            status,
        } => format!(
            "{}: {} {item} {}",
            wire(stage),
            wire(kind),
            serde_json::to_string(status).unwrap_or_default()
        ),
        Event::IssueOpened { issue } => format!(
            "{}: issue [{}] {}: {}",
            wire(&issue.stage),
            wire(&issue.severity),
            wire(&issue.kind),
            issue.message
        ),
        Event::StageFinished { stage, summary, .. } => format!(
            "{}: finished (done {}, partial {}, skipped {}, failed {}, errors {})",
            wire(stage),
            summary.done,
            summary.partial,
            summary.skipped,
            summary.failed,
            summary.errors
        ),
    }
}

/// Drains an event receiver: NDJSON to stdout with `--json`, else one line per
/// event on stderr. Drop every sender, then call `finish`, so nothing is lost.
pub struct Printer {
    task: JoinHandle<()>,
}

impl Printer {
    pub fn start(json: bool, mut rx: Receiver<Event>) -> Printer {
        let task = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(e) if json => print_json(&e),
                    Ok(e) => eprintln!("{}", render(&e)),
                    Err(RecvError::Lagged(n)) => eprintln!("warning: dropped {n} events"),
                    Err(RecvError::Closed) => break,
                }
            }
        });
        Printer { task }
    }

    /// Waits for the printer to drain the channel (it ends once the senders are dropped).
    pub async fn finish(self) {
        let _ = self.task.await;
    }
}
