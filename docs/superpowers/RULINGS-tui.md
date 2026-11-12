# Terminal UI: rulings and deferred backlog

Collected from the build ledger on 2026-11-12. Decisions Claude made while Isaac was offline, with what each costs if wrong, plus everything deferred.

## Rulings

- | Tasks | Produces → consumes | Finding | Ruling |
- | T3 ↔ T1 | stage_state derivation | TUI plan defines StageState in TUI; desktop plan wants it in session | Ruling: T3 puts stage_state(&Overview, Stage) -> StageState in porter-engine::session so desktop reuses it — cost: none |
- | T4 ↔ T1 | Session::resolve_issue | T4 says add if T1 didn't | Ruling: T1 adds it — cost: none |
- | T6 | dry-run preview counts | needs reading raw/load-preview | Ruling: Session gains load_preview_counts() -> Option<BTreeMap<String,u64>> in T6 |
- Carried from engine phase: parked m6 (eager read before regular-file check, wordpress/media.rs:229) — Ruling: fix it inside T1's dispatch with a test (one-line safety fix) — cost: T1 diff touches one unrelated engine line (separate commit)
- Task 1: Ruling: accepted RunOutcome extra fields, RunHandle::cancel_token(); reload_config(&mut self) to become &self with an internal RwLock<ProjectConfig> in T6 (TUI holds Arc<Session>); in-memory issues paging ok until 100k (ponytail noted)
- Task 2: Ruling: crate-level #![allow(dead_code)] tolerated until T8, which must remove it — cost: dead code may hide until then; no TTY in this environment, so manual TUI runs are Isaac's (noted for morning)
- Task 2: Ruling (carry to T3): confirm-quit during a run must cancel AND await the run handle (bounded, e.g. 10 s) before exiting so the stage records interrupted state
- Task 3: Ruling (carry to T4): hold the finished stage's live state until the overview refresh lands (no one-frame stale ✓ after an errored run)
- Task 5: Ruling (carry to T6): add test filter_typing_wins_over_detail_keys (open issue list, `/`, type j → filter "j", sel unchanged); fix help column overlap for long descriptions
- Task 6: Ruling: dry-run load clears raw/load-preview first (counts = last dry run exactly); load_preview_counts returns the preview time; modal shows age and refuses apply when the preview is older than the latest normalise run or porter.toml mtime ('dry run is stale — press l'); wording 'would send'
- Task 6: Ruling (carry to T7): preview_stale fails CLOSED on store errors/unparseable timestamps; re-evaluate staleness at `y`; add store-backed normalise-newer test
- Task 7: Ruling: accepted validate-on-scratch-copy then atomic write (safer than spec order); omitted wp-rest keyring + wp-db table_prefix/site_url fields (edit via e) — cost: users must hand-edit those
- Note (desktop prep): gpui 0.2.2 and gpui-component 0.9.1 are on crates.io — Ruling for desktop T1: prefer exact crates.io pins (=x.y.z) of a mutually compatible pair over git revs; if gpui-component needs a git gpui, pin both to git revs; runtime_shaders feature name to be verified at build time
- Final TUI review: With fixes — 10 findings (final-findings.md); Ruling #5: dry_run flag stored on run row, shown in TUI and CLI

## Deferred minors

- Task 1: minor (deferred): resolve_issue sync on store mutex (TUI must call via spawn_blocking); item() loads all issues in memory; items() one status query per row; FIFO test not cfg(unix); no guard-after-panic test
- Task 2: minor (deferred): no bare-porter non-TTY regression test; EventStream error silently disables input; 143 not re-raised
- Task 3: minor (deferred): SIGTERM doesn't await run; NO_COLOR empty-value convention; RunPlan::All running flag delay
- Task 4: minor (deferred): filter_key awkward assignment; resolve modal j/k-into-note undocumented
- Task 4: minor (deferred): lists_tests leaks a tempdir per test (mem::forget)
- Task 5: minor (deferred): duplicate j/k rows in help; DetailIssue Enter shadows panel Enter
- Task 6: minor (deferred): resume failure path continues drawing; keypress during editor may drop; open/base64 pinned in crate not workspace
- Task 7: minor (deferred): ApplyRechecked has no modal generation id (benign); include/exclude comma-split
- Task 7: minor (deferred): form invalid-name line insertion may shift scroll by one; engine rejects untrimmed env names (fails closed); "created meanwhile" wording when file pre-existed. Checked: render() writes only to a scratch tempdir.
- Task 8: minor (deferred): e2e exit condition may beat the post-RunDone overview refresh (low flake risk); "Items 1/5" hard-codes fixture count
- Final TUI review fix wave: 10/10 addressed (commits 2de25a8..826caa6); minor (deferred): confirm height estimate vs word-wrap; CLI '(dry run)' column misalign; e editor sh -c fails on Windows; windows-check only type-checks

## Needs a human in a real terminal

- Editor suspend/resume (vim, code -w), Ctrl-C inside the editor, keys typed while editing
- OSC 52 copy inside tmux; resize while running; 80x24 with a real project
- The apply prompt with many collections; pasting into the setup form; o on Linux
- An injected run panic, kill -TERM mid-run, double Ctrl-C from the filter box
- Follow-up after final review: RunDone ordering race (found by desktop bridge tests) fixed in d34d0df, reviewed approved.
