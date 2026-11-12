//! End to end: keys in, real Session run on the WXR fixture, results on screen.
use crate::app::{App, Panel, update};
use crate::msg::{Cmd, Msg};
use crate::runtime::{RunSlot, exec};
use crate::ui;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use porter_engine::project::Project;
use porter_engine::session::{Session, StageState};
use porter_model::Stage;
use ratatui::{Terminal, backend::TestBackend};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn wxr_project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    Project::init(d.path(), "t", "wordpress").unwrap();
    let wxr =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../porter-engine/tests/fixtures/wxr/basic.xml");
    let toml = d.path().join("porter.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(
        &toml,
        format!(
            "{text}\n[[connectors]]\nkind = \"wxr\"\npath = {:?}\n",
            wxr.display().to_string()
        ),
    )
    .unwrap();
    d
}

fn key(c: KeyCode) -> Msg {
    Msg::Key(KeyEvent::new(c, KeyModifiers::NONE))
}

fn screen(app: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
    t.draw(|f| ui::view(app, f)).unwrap();
    let buf = t.backend().buffer().clone();
    buf.content()
        .chunks(100)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn e2e_analyse_wxr_fixture() {
    let d = wxr_project();
    let session = Arc::new(Session::open(d.path()).unwrap());
    let mut engine = session.subscribe();
    let mut app = App::new(Some(session), (100, 30));
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut run = RunSlot::default();

    let mut cmds = app.refresh_cmds();
    cmds.extend(update(&mut app, key(KeyCode::Tab))); // Project -> Stages
    assert_eq!(app.focus, Panel::Stages);
    assert_eq!(app.panels.stages.selected(), Stage::Analyse);
    cmds.extend(update(&mut app, key(KeyCode::Char(' '))));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Run(_))), "{cmds:?}");

    let driven = tokio::time::timeout(Duration::from_secs(60), async {
        let mut done = false;
        loop {
            for c in std::mem::take(&mut cmds) {
                exec(c, &app, &tx, &mut run, d.path());
            }
            if done && app.panels.items.shown().len() >= 2 && app.panels.overview.is_some() {
                // Quiet once RunDone's refresh has landed; a message still queued is
                // handled, never dropped.
                match rx.try_recv() {
                    Ok(m) => {
                        cmds = update(&mut app, m);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            let msg = tokio::select! {
                Ok(e) = engine.recv() => Msg::Engine(e),
                Some(m) = rx.recv() => m,
            };
            if matches!(msg, Msg::RunDone(_)) {
                done = true;
            }
            cmds = update(&mut app, msg);
        }
    })
    .await;
    assert!(
        driven.is_ok(),
        "timed out; log: {:?}",
        app.log.iter().map(|l| &l.0).collect::<Vec<_>>()
    );

    let row = crate::ui::stages::rows(&app)
        .into_iter()
        .find(|r| r.stage == Stage::Analyse)
        .unwrap();
    assert!(
        matches!(row.state, StageState::Clean | StageState::Warnings),
        "{:?}",
        row.state
    );
    let titles: Vec<_> = app
        .panels
        .items
        .shown()
        .iter()
        .filter_map(|r| r.title.clone())
        .collect();
    assert!(titles.contains(&"Hello World".to_string()), "{titles:?}");
    let s = screen(&app);
    assert!(s.contains("Items 1/5") && s.contains("✓ analyse"), "{s}");
}
