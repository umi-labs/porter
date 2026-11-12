use crate::app::{App, DetailView, Modal, Panel, update};
use crate::lists::Group;
use crate::msg::{Cmd, DataMsg, Msg, RefreshScope};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use porter_engine::events::{Event, Summary};
use porter_engine::session::{ItemRow, Page, Paged};
use porter_engine::store::RunId;
use porter_model::{Issue, IssueKind, ItemId, ItemKind, ItemStatus, Resolution, Severity, Stage};
use uuid::Uuid;

pub fn rows(from: u128, n: u128) -> Vec<ItemRow> {
    (from..from + n)
        .map(|i| ItemRow {
            id: ItemId(Uuid::from_u128(i)),
            kind: ItemKind::Document,
            label: format!("/page-{i}"),
            title: Some(format!("Page {i}")),
            statuses: vec![(Stage::Extract, ItemStatus::Done)],
        })
        .collect()
}

pub fn issue(n: u128, kind: IssueKind, sev: Severity) -> Issue {
    Issue {
        id: Uuid::from_u128(n),
        kind,
        severity: sev,
        item: Some(ItemId(Uuid::from_u128(n))),
        stage: Stage::Normalise,
        message: format!("problem {n}"),
        details: serde_json::json!({"href": "/gone"}),
        resolution: Resolution::Open,
    }
}

pub fn press(app: &mut App, c: KeyCode) -> Vec<Cmd> {
    update(app, Msg::Key(KeyEvent::new(c, KeyModifiers::NONE)))
}

pub fn deliver_items(app: &mut App, cmd: &Cmd, total: u64) -> Vec<Cmd> {
    let Cmd::Items { page, seq, .. } = cmd else {
        panic!("not an items cmd: {cmd:?}")
    };
    let n = (total - page.offset).min(page.limit);
    update(
        app,
        Msg::Data(DataMsg::Items {
            seq: *seq,
            page: *page,
            paged: Paged {
                rows: rows(u128::from(page.offset), u128::from(n)),
                total,
            },
        }),
    )
}

fn items_of(cmds: &[Cmd]) -> Vec<&Cmd> {
    cmds.iter()
        .filter(|c| matches!(c, Cmd::Items { .. }))
        .collect()
}

fn loaded(total: u64) -> App {
    let d = tempfile::tempdir().unwrap();
    porter_engine::project::Project::init(d.path(), "demo", "wordpress").unwrap();
    let s = porter_engine::session::Session::open(d.path()).unwrap();
    let mut app = App::new(Some(std::sync::Arc::new(s)), (120, 40));
    std::mem::forget(d);
    let cmds = app.refresh_cmds();
    let first = items_of(&cmds)[0].clone();
    deliver_items(&mut app, &first, total);
    press(&mut app, KeyCode::Char('4'));
    app
}

#[test]
fn items_page_queries_bounded() {
    let mut app = loaded(1200);
    let mut seen = vec![];
    for _ in 0..499 {
        seen.extend(press(&mut app, KeyCode::Char('j')));
    }
    assert!(seen.is_empty(), "inside the loaded page: no query");
    let at_edge = press(&mut app, KeyCode::Char('j'));
    assert_eq!(at_edge.len(), 1, "exactly one query at the page boundary");
    let Cmd::Items { page, .. } = &at_edge[0] else {
        panic!("{at_edge:?}")
    };
    assert_eq!(
        *page,
        Page {
            offset: 500,
            limit: 500
        }
    );
    for _ in 0..20 {
        assert!(
            press(&mut app, KeyCode::Char('j')).is_empty(),
            "already loading"
        );
    }
    deliver_items(&mut app, &at_edge[0], 1200);
    for _ in 0..(999 - 520) {
        assert!(press(&mut app, KeyCode::Char('j')).is_empty());
    }
    let next = press(&mut app, KeyCode::Char('j'));
    assert_eq!(next.len(), 1);
    // Deliver it, then a stage finishes: only the visible page is re-queried.
    deliver_items(&mut app, &next[0], 1200);
    let cmds = update(
        &mut app,
        Msg::Engine(Event::StageFinished {
            stage: Stage::Load,
            run: RunId(Uuid::nil()),
            summary: Summary::default(),
        }),
    );
    assert!(cmds.contains(&Cmd::Refresh(RefreshScope::All)));
    let q = items_of(&cmds);
    assert_eq!(q.len(), 1);
    let Cmd::Items { page, .. } = q[0] else {
        panic!()
    };
    assert_eq!(page.offset, 1000);
}

#[test]
fn resolve_requires_note_for_handled() {
    let mut app = App::new(None, (120, 40));
    let i = issue(1, IssueKind::MissingMedia, Severity::Error);
    app.focus = Panel::Issues;
    app.detail.push(DetailView::Issue(i.clone()));
    assert!(press(&mut app, KeyCode::Char('x')).is_empty());
    assert!(matches!(app.modal, Some(Modal::Resolve(_))));
    // Handled manually is the first choice; an empty note is refused.
    assert!(press(&mut app, KeyCode::Enter).is_empty());
    let Some(Modal::Resolve(m)) = &app.modal else {
        panic!("modal must stay open")
    };
    assert!(m.error.is_some());
    for c in "  ".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    assert!(press(&mut app, KeyCode::Enter).is_empty(), "blank note");
    for c in "fixed by hand".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let cmds = press(&mut app, KeyCode::Enter);
    assert_eq!(
        cmds,
        [Cmd::Resolve(
            i.id,
            Resolution::HandledManually {
                note: "fixed by hand".into()
            }
        )]
    );
    assert!(app.modal.is_none());
    // Won't fix and reopen need no note; Esc cancels.
    press(&mut app, KeyCode::Char('x'));
    press(&mut app, KeyCode::Down);
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        [Cmd::Resolve(i.id, Resolution::WontFix)]
    );
    press(&mut app, KeyCode::Char('x'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        [Cmd::Resolve(i.id, Resolution::Open)]
    );
    press(&mut app, KeyCode::Char('x'));
    assert!(press(&mut app, KeyCode::Esc).is_empty());
    assert!(app.modal.is_none());
}

#[test]
fn filter_narrows_and_clears_with_esc() {
    let mut app = loaded(3);
    assert_eq!(app.panels.items.shown().len(), 3);
    press(&mut app, KeyCode::Char('/'));
    for c in "pg2".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let shown = app.panels.items.shown();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].label, "/page-2");
    // Typing never ran commands such as quit.
    assert!(!app.quit);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.panels.items.shown().len(), 3);
    assert!(!app.panels.items.filter.editing);
}

#[test]
fn enter_applies_server_text_and_esc_clears_it() {
    let mut app = loaded(3);
    press(&mut app, KeyCode::Char('/'));
    for c in "about".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let cmds = press(&mut app, KeyCode::Enter);
    let Cmd::Items { q, page, .. } = &cmds[0] else {
        panic!("{cmds:?}")
    };
    assert_eq!(q.text.as_deref(), Some("about"));
    assert_eq!(page.offset, 0);
    let cmds = press(&mut app, KeyCode::Esc);
    let Cmd::Items { q, .. } = &cmds[0] else {
        panic!("{cmds:?}")
    };
    assert_eq!(q.text, None);
}

#[test]
fn f_cycles_all_open_errors() {
    let mut app = loaded(3);
    let m = |app: &App| app.panels.items.mode;
    let start = m(&app);
    press(&mut app, KeyCode::Char('f'));
    let one = m(&app);
    press(&mut app, KeyCode::Char('f'));
    let two = m(&app);
    press(&mut app, KeyCode::Char('f'));
    assert_ne!(start, one);
    assert_ne!(one, two);
    assert_eq!(m(&app), start);
}

#[test]
fn issue_groups_enter_opens_paged_list() {
    let mut app = App::new(None, (120, 40));
    let seq = app.panels.issues.seq;
    update(
        &mut app,
        Msg::Data(DataMsg::IssueGroups {
            seq,
            groups: vec![Group {
                kind: IssueKind::MissingMedia,
                total: 450,
                worst: Severity::Error,
            }],
        }),
    );
    press(&mut app, KeyCode::Char('3'));
    let cmds = press(&mut app, KeyCode::Enter);
    let [Cmd::Issues { q, page, .. }] = cmds.as_slice() else {
        panic!("{cmds:?}")
    };
    assert_eq!(q.kind, Some(IssueKind::MissingMedia));
    assert_eq!(
        *page,
        Page {
            offset: 0,
            limit: 200
        }
    );
    assert!(matches!(app.detail.last(), Some(DetailView::Issues(_))));
}

#[test]
fn pagedown_past_page_end_never_panics() {
    for h in 1..=40u16 {
        let mut app = loaded(1200);
        // PageDown from the last row: selection runs past the loaded rows while page 2 loads.
        for _ in 0..30 {
            press(&mut app, KeyCode::PageDown);
        }
        let area = ratatui::layout::Rect::new(0, 0, 40, h);
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, h)).unwrap();
        t.draw(|f| crate::ui::render_items_for_test(&app, f, area))
            .unwrap();
        // The shared window helper is valid for every selection, length and height.
        for len in 0..30usize {
            for sel in 0..(len + 25) {
                let r = crate::ui::window(sel, len, usize::from(h));
                assert!(r.start <= r.end && r.end <= len, "{sel} {len} {h} {r:?}");
            }
        }
    }
}

#[test]
fn stale_seq_page_is_dropped() {
    let mut app = loaded(1200);
    let cmds = press(&mut app, KeyCode::Char('/'));
    assert!(cmds.is_empty());
    for c in "x".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    let new = press(&mut app, KeyCode::Enter);
    let old_seq = match &new[0] {
        Cmd::Items { seq, .. } => seq - 1,
        c => panic!("{c:?}"),
    };
    update(
        &mut app,
        Msg::Data(DataMsg::Items {
            seq: old_seq,
            page: Page {
                offset: 0,
                limit: 500,
            },
            paged: Paged {
                rows: rows(0, 500),
                total: 1200,
            },
        }),
    );
    assert!(
        app.panels.items.win.rows.is_empty(),
        "answer to the replaced query is ignored"
    );
}

#[test]
fn selection_clamped_after_shrunk_page() {
    let mut app = loaded(1200);
    for _ in 0..100 {
        press(&mut app, KeyCode::Char('j'));
    }
    let cmds = app.refresh_cmds();
    let c = items_of(&cmds)[0].clone();
    deliver_items(&mut app, &c, 10);
    assert_eq!(app.panels.items.sel, 9);
    assert!(app.panels.items.selected().is_some());
}

#[test]
fn filtered_empty_result_says_scope() {
    let mut app = loaded(1200);
    press(&mut app, KeyCode::Char('f'));
    press(&mut app, KeyCode::Char('f'));
    let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    t.draw(|f| crate::ui::view(&app, f)).unwrap();
    let out = t.backend().to_string();
    assert!(out.contains("0 in loaded 500 of 1200"), "{out}");
    assert!(out.contains("no matches in loaded 500 of 1200"));
}
