use crate::app::{App, Modal};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
};

mod detail;
mod filter;
mod footer;
mod help;
mod issues;
mod items;
mod layout;
mod log;
mod modal;
mod project;
pub mod stages;

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
    let Some(l) = layout::areas(area, app.show_log, !app.detail.is_empty()) else {
        f.render_widget(Paragraph::new("terminal too small (need 80×24)"), area);
        return;
    };
    if app.session.is_none() {
        f.render_widget(
            Paragraph::new("no porter project here — setup coming in Task 7\n\nq quit"),
            area,
        );
        return;
    }
    if let Some([project, stages, issues, items]) = l.panels {
        self::project::render(app, f, project);
        self::stages::render(app, f, stages);
        self::issues::render(app, f, issues);
        self::items::render(app, f, items);
    }
    if let Some(d) = l.detail {
        self::detail::render(app, f, d);
    }
    if let Some(r) = l.log {
        self::log::render(app, f, r);
    }
    self::footer::render(app, f, l.footer);

    match &app.modal {
        Some(Modal::ConfirmQuit) => {
            let r = centered(area, 44, 5);
            f.render_widget(Clear, r);
            f.render_widget(
                Paragraph::new("A stage is running. Cancel it and quit?\n\ny yes · n no")
                    .block(panel("Quit", true)),
                r,
            );
        }
        Some(Modal::Resolve(m)) => modal::resolve(m, f, area),
        None => {}
    }
    if app.help {
        self::help::render(app, f, area);
    }
}

#[cfg(test)]
pub(crate) fn render_items_for_test(app: &App, f: &mut Frame, area: Rect) {
    items::render(app, f, area);
}

/// The rows of a list of `len` to draw in `h` lines so that `sel` stays visible.
pub(crate) fn window(sel: usize, len: usize, h: usize) -> std::ops::Range<usize> {
    // `sel` can sit past the loaded rows while the next page loads: clamp so the range is valid.
    let sel = sel.min(len.saturating_sub(1));
    let start = (sel + 1).saturating_sub(h.max(1));
    start..(start + h).min(len).max(start.min(len))
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
    use crate::app::Panel;
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

    #[test]
    fn finished_stage_never_shows_a_stale_tick() {
        // The stored overview says Extract was clean; the run that just ended had errors.
        let (mut app, _d) = app_with(sum(|_| {}), true);
        let errors = Summary {
            errors: 1,
            ..Default::default()
        };
        update(
            &mut app,
            Msg::Engine(Event::StageFinished {
                stage: Stage::Extract,
                run: RunId(uuid::Uuid::nil()),
                summary: errors.clone(),
            }),
        );
        let out = stages_panel(&app);
        assert!(!out.contains('✓') && out.contains('✗'), "{out}");
        // Still true while a stale overview answer is the last thing to have arrived.
        let stale = Overview {
            last_runs: app.panels.overview.clone().unwrap().last_runs,
            ..Default::default()
        };
        update(
            &mut app,
            Msg::Data(DataMsg::Overview(Box::new(stale.clone()))),
        );
        assert!(
            stages_panel(&app).contains('✓'),
            "overview refresh landed: stored state rules"
        );
        // RunDone does the same through its summaries.
        let (mut app, _d) = app_with(sum(|_| {}), true);
        update(
            &mut app,
            Msg::RunDone(RunOutcome {
                code: 2,
                per_stage: vec![(Stage::Extract, 2, None)],
                summaries: vec![(Stage::Extract, errors)],
                ..Default::default()
            }),
        );
        let out = stages_panel(&app);
        assert!(!out.contains('✓') && out.contains('✗'), "{out}");
    }

    #[test]
    fn live_state_survives_until_the_last_overview_lands() {
        let (mut app, _d) = app_with(sum(|_| {}), true);
        let errors = Summary {
            errors: 1,
            ..Default::default()
        };
        for _ in 0..2 {
            update(
                &mut app,
                Msg::Engine(Event::StageFinished {
                    stage: Stage::Extract,
                    run: RunId(uuid::Uuid::nil()),
                    summary: errors.clone(),
                }),
            );
        }
        let old = app.panels.overview.clone().unwrap();
        update(&mut app, Msg::Data(DataMsg::Overview(Box::new(old))));
        assert!(
            stages_panel(&app).contains('✗'),
            "one query is still in flight"
        );
    }

    // --- Issues / Items / Detail / modal at 120x40 -------------------------------

    use crate::app::{DetailView, Modal, ResolveModal};
    use crate::lists::{Group, IssueList, Window};
    use crate::msg::DataMsg;
    use porter_engine::session::{ItemDetail, ItemRow, Page, Paged};
    use porter_model::{
        Issue, IssueKind, ItemId, ItemKind, ItemStatus, Resolution, Severity, SourceRef,
    };
    use uuid::Uuid;

    fn id(n: u128) -> ItemId {
        ItemId(Uuid::from_u128(n))
    }

    fn mk_issue(n: u128, kind: IssueKind, sev: Severity, msg: &str) -> Issue {
        Issue {
            id: Uuid::from_u128(n),
            kind,
            severity: sev,
            item: Some(id(n)),
            stage: Stage::Normalise,
            message: msg.into(),
            details: serde_json::json!({"href": "/old/page", "context": {"block": "paragraph"}}),
            resolution: Resolution::Open,
        }
    }

    fn lists_app() -> (App, tempfile::TempDir) {
        let (mut app, d) = app_with(None, false);
        let seq = app.next_seq();
        app.panels.issues.seq = seq;
        update(
            &mut app,
            Msg::Data(DataMsg::IssueGroups {
                seq,
                groups: vec![
                    Group {
                        kind: IssueKind::MissingMedia,
                        total: 450,
                        worst: Severity::Error,
                    },
                    Group {
                        kind: IssueKind::UnresolvedLink,
                        total: 12,
                        worst: Severity::Warning,
                    },
                    Group {
                        kind: IssueKind::UnmappedField,
                        total: 3,
                        worst: Severity::Info,
                    },
                ],
            }),
        );
        let seq = app.next_seq();
        app.panels.items.win.reset(seq);
        let rows = vec![
            ItemRow {
                id: id(1),
                kind: ItemKind::Document,
                label: "/about".into(),
                title: Some("About us".into()),
                statuses: vec![(Stage::Extract, ItemStatus::Done)],
            },
            ItemRow {
                id: id(2),
                kind: ItemKind::Document,
                label: "/contact".into(),
                title: Some("Contact".into()),
                statuses: vec![(
                    Stage::Load,
                    ItemStatus::Failed {
                        error: "422 unprocessable".into(),
                    },
                )],
            },
            ItemRow {
                id: id(3),
                kind: ItemKind::Media,
                label: "/uploads/hero.jpg".into(),
                title: None,
                statuses: vec![(
                    Stage::Load,
                    ItemStatus::Partial {
                        reason: "no alt text".into(),
                    },
                )],
            },
        ];
        update(
            &mut app,
            Msg::Data(DataMsg::Items {
                seq,
                page: Page {
                    offset: 0,
                    limit: 500,
                },
                paged: Paged {
                    rows,
                    total: 50_000,
                },
            }),
        );
        (app, d)
    }

    #[test]
    fn issues_panel_120x40() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Issues;
        insta::assert_snapshot!(render(&app, 120, 40));
    }

    #[test]
    fn items_panel_120x40() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Items;
        insta::assert_snapshot!(render(&app, 120, 40));
    }

    #[test]
    fn issue_list_and_issue_detail_120x40() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Issues;
        let mut win = Window::new(200);
        let seq = app.next_seq();
        win.reset(seq);
        win.apply(
            seq,
            0,
            Paged {
                rows: vec![
                    mk_issue(
                        1,
                        IssueKind::MissingMedia,
                        Severity::Error,
                        "hero.jpg not found",
                    ),
                    mk_issue(
                        2,
                        IssueKind::MissingMedia,
                        Severity::Warning,
                        "logo.svg unreadable",
                    ),
                ],
                total: 450,
            },
        );
        app.detail.push(DetailView::Issues(IssueList {
            kind: IssueKind::MissingMedia,
            mode: crate::lists::Mode::All,
            win,
            sel: 0,
        }));
        insta::assert_snapshot!("issue_list_detail_120x40", render(&app, 120, 40));
        let i = mk_issue(
            1,
            IssueKind::MissingMedia,
            Severity::Error,
            "hero.jpg not found",
        );
        app.detail.push(DetailView::Issue(i));
        insta::assert_snapshot!(render(&app, 120, 40));
    }

    #[test]
    fn item_detail_120x40() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Items;
        let data = serde_json::json!({
            "title": "About us", "type": "page", "path": "/about", "parent": null,
            "fields": (0..60).map(|i| (format!("f{i}"), serde_json::json!(i))).collect::<serde_json::Map<_, _>>(),
        });
        app.detail.push(DetailView::Item(Box::new(ItemDetail {
            row: ItemRow {
                id: id(1),
                kind: ItemKind::Document,
                label: "/about".into(),
                title: Some("About us".into()),
                statuses: vec![
                    (Stage::Extract, ItemStatus::Done),
                    (
                        Stage::Load,
                        ItemStatus::Failed {
                            error: "422 unprocessable".into(),
                        },
                    ),
                ],
            },
            source: SourceRef {
                connector: "wordpress".into(),
                source_id: "42".into(),
                url: Some("/about".into()),
            },
            data,
            issues: vec![mk_issue(
                1,
                IssueKind::UnresolvedLink,
                Severity::Warning,
                "link to /gone",
            )],
        })));
        let out = render(&app, 120, 40);
        assert!(out.contains("more lines"), "long JSON is truncated");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn resolve_modal_120x40() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Issues;
        app.modal = Some(Modal::Resolve(ResolveModal {
            issue: Uuid::from_u128(1),
            title: "missing-media: hero.jpg not found".into(),
            choice: 0,
            note: "re-uploaded by hand".into(),
            error: None,
        }));
        insta::assert_snapshot!(render(&app, 120, 40));
    }

    // --- Task 5: log, help, footer, responsive layouts ---------------------------

    #[test]
    fn layout_200x60() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Items;
        insta::assert_snapshot!(render(&app, 200, 60));
    }

    #[test]
    fn layout_120x40() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Items;
        insta::assert_snapshot!(render(&app, 120, 40));
    }

    #[test]
    fn layout_80x24() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Issues;
        insta::assert_snapshot!(render(&app, 80, 24));
    }

    #[test]
    fn layout_60x20_is_too_small() {
        let (app, _d) = lists_app();
        let out = render(&app, 60, 20);
        assert!(out.contains("terminal too small (need 80×24)"));
        insta::assert_snapshot!(out);
    }

    #[test]
    fn narrow_detail_replaces_left_column_and_esc_goes_back() {
        let (mut app, _d) = lists_app();
        app.focus = Panel::Issues;
        let before = render(&app, 90, 30);
        assert!(before.contains("Stages") && !before.contains("Detail"));
        press_key(&mut app, KeyCode::Enter);
        let open = render(&app, 90, 30);
        assert!(
            open.contains("Detail") && !open.contains("Stages"),
            "{open}"
        );
        press_key(&mut app, KeyCode::Esc);
        assert_eq!(render(&app, 90, 30), before);
        assert!(
            render(&app, 100, 30).contains("Detail"),
            "wide: both at once"
        );
    }

    #[test]
    fn at_toggles_the_log() {
        let (mut app, _d) = lists_app();
        app.push_log("hello from the log".into());
        assert!(render(&app, 120, 40).contains("hello from the log"));
        press_key(&mut app, KeyCode::Char('@'));
        assert!(!render(&app, 120, 40).contains("hello from the log"));
        press_key(&mut app, KeyCode::Char('@'));
        assert!(render(&app, 120, 40).contains("hello from the log"));
    }

    #[test]
    fn help_opens_with_question_mark_and_closes_with_esc() {
        let (mut app, _d) = lists_app();
        press_key(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        // Keys other than close/scroll do nothing underneath the overlay.
        let focus = app.focus;
        press_key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, focus);
        insta::assert_snapshot!("help_120x40", render(&app, 120, 40));
        press_key(&mut app, KeyCode::Esc);
        assert!(!app.help);
    }

    #[test]
    fn help_lists_every_bound_key() {
        let (mut app, _d) = lists_app();
        app.help = true;
        let out = render(&app, 140, 100);
        for b in crate::keymap::BINDINGS {
            assert!(out.contains(b.shown), "help is missing key {}", b.shown);
            assert!(out.contains(b.what), "help is missing {}", b.what);
        }
    }

    #[test]
    fn footer_follows_focus_and_fits_80_columns() {
        let (mut app, _d) = lists_app();
        for p in [Panel::Project, Panel::Stages, Panel::Issues, Panel::Items] {
            app.focus = p;
            let last = render(&app, 80, 24).lines().last().unwrap().to_string();
            assert!(
                last.contains("? help") && last.contains("q quit"),
                "{p:?}: {last}"
            );
            assert_eq!(last.contains("space run"), p == Panel::Stages, "{p:?}");
            assert_eq!(
                last.contains("/ filter"),
                matches!(p, Panel::Issues | Panel::Items)
            );
        }
    }

    #[test]
    fn resize_never_panics() {
        let mut apps = vec![];
        let mk = |f: fn(&mut App)| {
            let (mut a, d) = lists_app();
            f(&mut a);
            (a, d)
        };
        apps.push(mk(|_| {}));
        apps.push(mk(|a| a.help = true));
        apps.push(mk(|a| a.show_log = false));
        apps.push(mk(|a| a.modal = Some(Modal::ConfirmQuit)));
        apps.push(mk(|a| {
            a.focus = Panel::Issues;
            a.modal = Some(Modal::Resolve(ResolveModal {
                issue: Uuid::nil(),
                title: "t".into(),
                choice: 0,
                note: "n".into(),
                error: Some("e".into()),
            }))
        }));
        apps.push(mk(|a| {
            a.focus = Panel::Issues;
            press_key(a, KeyCode::Enter);
            let i = mk_issue(1, IssueKind::MissingMedia, Severity::Error, "m");
            a.detail.push(DetailView::Issue(i));
        }));
        apps.push(mk(|a| {
            a.focus = Panel::Items;
            a.detail.push(DetailView::Item(Box::new(ItemDetail {
                row: a.panels.items.selected().unwrap().clone(),
                source: SourceRef {
                    connector: "c".into(),
                    source_id: "1".into(),
                    url: None,
                },
                data: serde_json::json!({"a": 1}),
                issues: vec![],
            })));
        }));
        for _ in 0..300 {
            apps.push(mk(|a| a.push_log("log".into())));
        }
        apps.truncate(7);
        let none = App::new(None, (80, 24));
        let mut x: u64 = 0x2545F4914F6CDD1D;
        let mut next = |m: u64| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            1 + (x >> 33) % m
        };
        let mut sizes = vec![
            (1, 1),
            (79, 23),
            (80, 24),
            (99, 30),
            (100, 24),
            (300, 100),
            (1, 100),
            (300, 1),
        ];
        sizes.extend((0..200).map(|_| (next(300) as u16, next(100) as u16)));
        for (w, h) in sizes {
            for (a, _) in &apps {
                render(a, w, h);
            }
            render(&none, w, h);
        }
    }

    #[test]
    fn lagged_events_are_logged_and_state_refreshed() {
        let (mut app, _d) = lists_app();
        let cmds = update(&mut app, Msg::Lagged(7));
        assert!(app.log.back().is_some_and(|l| l.0 == "dropped 7 events"));
        assert!(
            !cmds.is_empty(),
            "dropped events may have hidden a stage ending"
        );
    }

    #[test]
    fn stubs_say_not_available_yet() {
        let (mut app, _d) = lists_app();
        for c in ['l', 'L', 'o', 'e'] {
            press_key(&mut app, KeyCode::Char(c));
            assert!(
                app.log
                    .back()
                    .is_some_and(|l| l.0.ends_with("not available yet")),
                "{c}"
            );
        }
    }

    fn press_key(app: &mut App, c: KeyCode) {
        update(app, Msg::Key(KeyEvent::new(c, KeyModifiers::NONE)));
    }
}
