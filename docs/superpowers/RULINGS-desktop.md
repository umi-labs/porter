# Desktop app: rulings and deferred backlog

These are taken from the build ledger on 2026-11-12. They are the decisions Claude made while Isaac was offline, each with what it costs if it's wrong, plus everything that was deferred.

The final whole-branch review found nothing Critical. Its two Important findings, and the cheap minor ones, were fixed in 4ea5475 and 95f1ff5 and re-reviewed clean.

## Needs a human with a display

None of the window behaviour has been run, because this environment has no display. The smoke checklist in `crates/porter-desktop/README.md` covers it:
- the launcher and recent projects
- run all (dry) on the WXR fixture
- content, media, URLs, issues and destination
- mapping edit → issues update
- the apply sheet gates
- closing a window during a run
- Cmd-Q
- opening the same project twice
- `open target/Porter.app`

## Decisions for Isaac

- **serde_json `preserve_order` is now workspace-wide (4ea5475).** GPUI was quietly turning it on whenever the desktop app was built, so JSON key order differed between builds. All of porter now writes JSON keys in insertion order, not sorted. Values are unchanged. The TUI snapshots were re-accepted.
- **`porter init` now creates porter.toml as 0600 (owner-only).** This is a user-visible CLI change.
- **The launcher's "New project…" dialog and the GPUI TestAppContext tests are not built.** Both are deferred to v1.1. `porter init` and the TUI setup cover project creation, and window wiring is on the manual checklist.
- **Cmd-Q** gets only about 100 ms from GPUI 0.2.2: it cancels the run and exits. Closing a window confirms first and waits up to 10 s. Both are documented in the README.

## Rulings
- | Tasks | Produces → consumes | Finding | Ruling |
- | T1 | GPUI pin | plan says git revs | Ruling: prefer exact crates.io pins of a compatible gpui (0.2.x) + gpui-component (0.9.x) pair; git revs only if incompatible; verify the runtime-shaders feature name at build — cost: none |
- | T3 ↔ TUI | stage_state | already in porter-engine::session (TUI T3) | Ruling: reuse it; no new extraction — cost: none |
- | T4 | Session::children | new paged query | consistent |
- | T6 | mapping::edit in porter-engine | core module, no platform names | consistent |
- | T8 | packaging | dist macOS app vs cargo-bundle | Ruling: cargo-bundle-free — produce the .app via a small script (Info.plist + binary) if dist can't, unsigned — cost: manual |
- | env | no display/TTY in this environment | GPUI windows can't be shown here | Ruling: rely on view-model unit tests + TestAppContext where it works headless; manual smoke checklist for Isaac |
- | per-task self-consistency | | none found | — |
- Task 1: Ruling: gpui =0.2.2 (runtime_shaders) + gpui-component =0.5.1 (newest on Zed's gpui; 0.6+ uses a fork); linux CI builds default members only; desktop-macos CI job — cost: gpui-component lags upstream features
- Ruling: desktop Tasks 2+3 batched (bridge + project window shell + overview/runs) — tightly coupled, saves a GPUI build cycle on a nearly full disk — cost: larger single review
- Found by desktop T2: TUI race — RunDone can arrive before the run's own StageStarted → TUI stage can stay '● running' after the run ended. Ruling: small follow-up dispatch to porter-tui after desktop T2+3 review (honesty issue)
- Task 2-3: Ruling (carry to D T8): Cmd-Q bypasses should-close — add on_app_quit hook that applies the same close_step
- Task 4: Ruling: accepted store migration v5 (parent/type partial indexes; older binaries can't open v5 dbs — fine pre-release); orphans listed at the top level (reachable); tree collapse on refresh deferred (ponytail)
- Task 5: Ruling (carry to T6): off-site absolute redirect `to` must not match a local document path (compare host via site_of; off-site → resolved-external, not local); thumbs open with O_NONBLOCK + is_file() on the opened fd (TOCTOU)
- Task 6: Ruling: accepted — save blocked only by validation problems the edit introduces (pre-existing problems don't block mapping edits); text boxes refill after save (unapplied typing lost)
- Task 6: Ruling (carry to T8): other porter.toml writers (porter-tui setup.rs persist_noclobber, engine project init) get the same fsync-before-persist; open-time check failures shown as general issue (accepted)
- Task 7: Ruling (carry to T8): "cancelled — nothing written" may be false for Generate/Spec (partial files) → "cancelled — output may be incomplete"
- Task 8: Ruling: accepted — gpui 0.2.2 quit handlers get ~100 ms and can't delay/prompt, so Cmd-Q cancels and waits ≤100 ms (window close still confirms/waits); documented. bundle.sh instead of dist (no .app target in dist 0.30.3)
- Task 8: Ruling: Important (stray .snap.new files committed; the two TUI snapshot tests fail under `cargo test --workspace`) folded into the desktop final-review fix wave — same root cause investigation; cost: none (fixed before merge)

## Deferred minors

Some of these were fixed in the final wave: the v4→v5 migration test, the close_step doc comment, the "media" magic string (now an engine constant), bundle.sh CARGO_TARGET_DIR, parent-cycle roots, and the recent-list tmp race (it now uses write_atomic).

- Task 1: minor (deferred): recent::load sync on UI thread (tiny file); stale recents never pruned; corrupt-file log line untested; canonical/symlink dedup untested; fixed tmp name race across two app instances
- Task 2-3: minor (deferred): runs history_row glyph for dry runs unchecked (text labelled); close_step doc comment displaced; window wiring untested
- Task 4: minor (deferred): type count vs roots-only expansion (label "top-level"); no v4→v5 migration test; Labels failure leaves links "resolving…"; parent cycles drop both docs from roots; nested toggle stop_propagation unverified visually
- Task 5: minor (deferred): invalidate() drops scroll on refresh; 'media' connector magic string; chain filter ignores issue resolution; media 'new URL' is filename
- Task 5: minor (deferred): landing_paths uses url column not $.url; query-string redirect targets don't match path docs (intended)
- Task 6: minor (deferred): transforms() can drop a non-serialising transform silently (unreachable today); drop_field removes transforms; write_atomic replaces a symlinked porter.toml with a regular file
- Task 7: minor (deferred): visible() refilters per paint (ponytail); Page limit u64::MAX
- Task 8: minor (deferred): bundle.sh ignores CARGO_TARGET_DIR; porter.toml created 0600 (consistent with owner-only config intent)
- Final: a self-parented doc lists itself as its own child in the content tree (cosmetic); Labels failure leaves links "resolving…"; write_atomic replaces a symlinked porter.toml with a regular file; one-window guard is per app instance only (two app instances or CLI+desktop can still run concurrently on one project)
