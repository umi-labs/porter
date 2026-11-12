# Porter terminal UI: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A lazygit-style terminal UI (`porter tui`, and bare `porter` in a TTY) over the porter engine, with a session layer shared by the CLI, the TUI and the future desktop app.

**Architecture:**
- `porter-engine::session` owns project open, config reload, one-at-a-time stage runs with cancel, event subscription and paged queries. The CLI's stage driving moves onto it.
- New crate `crates/porter-tui`: an Elm-style `App` state, pure `update(&mut App, Msg) -> Vec<Cmd>`, `view(&App, &mut Frame)`, and a runtime that turns key events and engine events into `Msg`s and executes `Cmd`s.

**Tech stack:** ratatui (latest), crossterm, tokio, insta (dev) for render snapshots.

**Spec:** `docs/superpowers/specs/2026-11-12-porter-tui-design.md`. It extends the engine spec `docs/superpowers/specs/2026-11-11-porter-engine-design.md`.

## Global Constraints

- The engine's global constraints still hold (see `.superpowers/sdd/2026-11-11-porter-engine-phase-1/constraints.md`), in particular:
  - honesty: true outcomes only, errors shown verbatim
  - no secrets typed into or stored by the TUI
  - `load` apply is never the default
- The TUI never shells out to `porter`. It uses `porter-engine` directly through `session`.
- The render loop never blocks on SQLite or the network. Store queries go through `spawn_blocking`.
- Works at 80×24, honours `NO_COLOR`, and uses the terminal's colours (no hard-coded RGB).
- The terminal is always restored: raw mode and the alternate screen are left on exit, on panic (panic hook) and on SIGTERM.
- `cargo fmt --all`; clippy at warn level; `#![deny(clippy::unwrap_used)]` outside tests.

## Review Focus

1. **Terminal restore after a panic, or after a stage error mid-run.** The user's shell must come back usable. Pinned in Task 2 (`panic_restores_terminal` via the hook unit test).
2. **Huge projects (50k items).** The Items panel must stay responsive: paging and no full reloads per event. Pinned in Task 4 (`items_page_queries_bounded`).
3. **Event floods while a stage runs** (thousands of ItemResult events). The UI must coalesce them and not redraw per event. Pinned in Task 3 (`progress_events_coalesce`).
4. **Ctrl-C semantics:**
   - first press cancels the stage (exit-3 semantics)
   - second press quits, after restoring the terminal
   - quitting while a run is active asks first

   Pinned in Task 3.
5. **Narrow and tiny terminals:** the narrow layout and the "too small" screen, with no panics on resize. Pinned in Task 5 snapshots at 80×24, 60×20 and 200×60.

---

### Task 1: Session layer

**Files:**
- Create: `crates/porter-engine/src/session.rs`
- Modify: `crates/porter-engine/src/lib.rs`, `crates/porter-cli/src/commands/stage.rs` (drive through Session)
- Test: `crates/porter-engine/tests/session.rs`

**Interfaces (produces):**
```rust
pub struct Session { /* project, config, events */ }
impl Session {
    pub fn open(dir: &Path) -> anyhow::Result<Session>;
    pub fn config(&self) -> &ProjectConfig;
    pub fn reload_config(&mut self) -> Vec<String>;               // validation messages
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event>;
    pub fn run(&self, plan: RunPlan) -> anyhow::Result<RunHandle>; // Err if a run is active
    pub fn running(&self) -> Option<RunInfo>;
    pub async fn overview(&self) -> anyhow::Result<Overview>;      // spawn_blocking inside
    pub async fn issues(&self, f: IssueQuery, page: Page) -> anyhow::Result<Paged<Issue>>;
    pub async fn items(&self, f: ItemQuery, page: Page) -> anyhow::Result<Paged<ItemRow>>;
    pub async fn item(&self, id: ItemId) -> anyhow::Result<ItemDetail>;
}
pub enum RunPlan { Stage(Stage, StageOpts), All { keep_going: bool, apply: bool } }
pub struct StageOpts { pub refresh: bool, pub apply: bool, pub schema_mode: SchemaMode }
pub struct RunHandle { pub fn cancel(&self); pub async fn wait(self) -> RunOutcome }
pub struct RunOutcome { pub per_stage: Vec<(Stage, i32 /*exit code*/, Option<String> /*error*/)>, pub code: i32 }
pub struct Page { pub offset: u64, pub limit: u64 } ; pub struct Paged<T> { pub rows: Vec<T>, pub total: u64 }
pub struct ItemRow { pub id: ItemId, pub kind: ItemKind, pub label: String, pub title: Option<String>, pub statuses: Vec<(Stage, ItemStatus)> }
```
`RunOutcome.code` uses the CLI's precedence: 1 > 3 > 2 > 0.

- [ ] **Step 1: Write the failing tests.**
  - `session_runs_one_stage_at_a_time`: a second `run` while one is active → `Err`.
  - `session_cancel_gives_code_3`: a WXR project, cancel immediately → `code == 3`.
  - `session_all_stops_at_first_failure`: a misconfigured destination → per_stage ends at schema with code 1, and verify isn't attempted.
  - `items_paged`: 1,200 items, page 500 → `rows.len() == 500` and `total == 1200`.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test session`. Expected: FAIL.
- [ ] **Step 3:** Implement. Move `run_loop`, `drive`, the adapter resolution and the exit precedence out of `porter-cli/src/commands/stage.rs` into the session, and make the CLI a thin caller. All existing CLI tests must still pass unchanged.
- [ ] **Step 4:** Run `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(engine): session layer shared by CLI and UIs`.

### Task 2: TUI crate, runtime, terminal safety, launch

**Files:**
- Create: `crates/porter-tui/{Cargo.toml,src/lib.rs,src/app.rs,src/msg.rs,src/runtime.rs,src/term.rs,src/ui/mod.rs}`
- Modify: `crates/porter-cli/src/main.rs` (add the `tui` subcommand; bare `porter` with a TTY on stdin and stdout launches the TUI)

**Interfaces (produces):**
- `pub async fn run_tui(dir: &Path) -> anyhow::Result<i32>`
- `App { session: Option<Arc<Session>>, focus: Panel, panels: Panels, modal: Option<Modal>, log: VecDeque<LogLine>, running: Option<RunInfo>, size: (u16,u16), quit: bool }`
- `enum Panel { Project, Stages, Issues, Items }`
- `enum Msg { Key(KeyEvent), Resize(u16,u16), Engine(Event), Data(DataMsg), Tick, RunDone(RunOutcome) }`
- `enum Cmd { Run(RunPlan), Cancel, Refresh(RefreshScope), OpenEditor, OpenReport(String), Copy(String), Quit }`
- `fn update(app: &mut App, msg: Msg) -> Vec<Cmd>`
- `fn view(app: &App, f: &mut Frame)`
- `term::install_panic_hook()` restores the terminal before printing the panic.

- [ ] **Step 1: Write the failing tests.**
  - `focus_cycles_with_tab`: Tab cycles Project → Stages → Issues → Items → Project.
  - `quit_when_idle_sets_quit`
  - `quit_while_running_opens_confirm`
  - `panic_restores_terminal`: the hook calls the injectable restore fn; assert it was called.
- [ ] **Step 2:** Run `cargo test -p porter-tui`. Expected: FAIL.
- [ ] **Step 3:** Implement. The runtime runs a single `tokio::select!` over the crossterm `EventStream`, the session event receiver, data results and a 100 ms tick. It redraws only when dirty. A SIGTERM handler restores the terminal and exits.
- [ ] **Step 4:** Run the tests. Expected: PASS. Manually, `cargo run -p porter-cli -- tui --project <wxr fixture project>` shows the four panels.
- [ ] **Step 5:** Commit: `feat(tui): app skeleton, runtime and terminal safety`.

### Task 3: Project and Stages panels, running stages, progress, cancel

**Files:** `crates/porter-tui/src/ui/{project.rs,stages.rs,detail.rs}`, `src/app.rs`

**Interfaces:** `StageRow { stage, state: StageState, last_run: Option<RunSummaryView>, progress: Option<(u64, Option<u64>, String)> }`, where `StageState` is one of:
- `Never`
- `Running`
- `Clean`
- `Warnings`
- `Errors`
- `Interrupted`
- `CouldNotRun(String)`

It is derived from the summary with the exit-code rules, and the glyphs follow the spec. `space` → `Cmd::Run(Stage)`; `R` → `Cmd::Run(All{keep_going:false, apply:false})`; `v` → verify; `c`/Ctrl-C while running → `Cmd::Cancel`; a second Ctrl-C → `Quit`.

- [ ] **Step 1: Write the failing tests.**
  - `stage_state_from_summary`: one case per state, including that error issues never map to Clean.
  - `progress_events_coalesce`: 5,000 Progress/ItemResult msgs in one tick → one dirty redraw, and the progress shows the latest value.
  - `ctrl_c_cancels_then_quits`
  - `space_on_stage_runs_it`
  - `run_while_running_is_refused_with_message`
- [ ] **Step 2:** Run `cargo test -p porter-tui`. Expected: FAIL.
- [ ] **Step 3:** Implement. Add insta snapshots of the Stages panel in each state at 120×40.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(tui): stages panel with live runs, progress and cancel`.

### Task 4: Issues and Items panels, Detail, filter, resolve

**Files:** `crates/porter-tui/src/ui/{issues.rs,items.rs,detail.rs,filter.rs,modal.rs}`

**Behaviour:**
- **Issues panel:** grouped by kind with counts, and severity glyphs.
- **Enter:**
  - on a kind → Detail lists those issues (paged, 200 at a time)
  - on an issue → its details JSON and a jump to its item
- **Items panel:** paged at 500 rows, kind/label/title/status; Enter → Detail shows the item (statuses per stage, its issues, a pretty-printed slice of its normalised JSON).
- **Filtering:** `/` is a fuzzy, incremental filter (client-side over the loaded page, plus a server-side `ItemQuery.text`). `f` cycles all → open → errors.
- **Resolving:** `x` on an issue opens a modal: handled manually (note required) / won't fix / reopen → `session` resolve.

- [ ] **Step 1: Write the failing tests.**
  - `items_page_queries_bounded`: scrolling past the end of a page issues exactly one query for the next page; a refresh after StageFinished re-queries only the visible page.
  - `resolve_requires_note_for_handled`
  - `filter_narrows_and_clears_with_esc`
  - Snapshots of the Issues and Items panels and of the Detail views.
- [ ] **Step 2:** Run `cargo test -p porter-tui`. Expected: FAIL.
- [ ] **Step 3:** Implement. Add `Session::resolve_issue(id, Resolution)` if Task 1 didn't.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(tui): issues and items with detail, filtering and resolve`.

### Task 5: Log, help, footer, layouts

**Files:** `crates/porter-tui/src/ui/{log.rs,help.rs,footer.rs,layout.rs}`

**Behaviour:**
- **Log:** a bounded ring buffer of 2,000 lines, toggled with `@`. Progress events update the stage bar instead of logging. `Lagged` adds "dropped N events".
- **Help:** `?` opens an overlay listing every key.
- **Footer:** shows the keys for the focused panel.
- **Layouts:** at 100 columns or wider, four panels plus Detail. Below 100, Detail replaces the left column on Enter and Esc goes back. Below 80×24, a "terminal too small (need 80×24)" screen.

- [ ] **Step 1: Write the failing tests.**
  - Snapshots at 200×60, 120×40, 80×24 and 60×20 (too small).
  - `resize_never_panics`: random sizes from 1×1 to 300×100.
  - `log_is_bounded`
  - `help_lists_every_bound_key`: generated from the keymap table, so it can't drift.
- [ ] **Step 2:** Run `cargo test -p porter-tui`. Expected: FAIL.
- [ ] **Step 3:** Implement. The keymap is a single table used by `update`, the footer and help.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(tui): log, help, footer and responsive layouts`.

### Task 6: Load, report, editor, clipboard

**Files:** `crates/porter-tui/src/{actions.rs,ui/confirm.rs}`

**Behaviour:**
- **Load:** `l` → dry-run load. `L` → a confirm modal showing the destination URL from config, the number of items to write by collection (from the last dry-run preview, or "run a dry run first" if there's none), and "this writes to a live CMS". Only `y` confirms; anything else cancels.
- **Report:** `o` → `report::write_all`, then opens the relevant file with the `open` crate.
- **Editor:** `e` → suspends the TUI (leave the alternate screen), runs `$EDITOR porter.toml` (default `vi`), restores the TUI, calls `reload_config()`, and shows validation messages in Detail.
- **Clipboard:** `y` → OSC 52 copy of the selected id.

- [ ] **Step 1: Write the failing tests.**
  - `apply_needs_explicit_y`
  - `apply_without_dry_run_says_so`
  - `editor_reload_shows_validation_errors` (with an injectable editor command)
  - `osc52_sequence_is_correct`
- [ ] **Step 2:** Run `cargo test -p porter-tui`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(tui): load with confirmation, reports, editor and clipboard`.

### Task 7: Setup flow

**Files:** `crates/porter-tui/src/setup.rs`, `crates/porter-tui/src/ui/form.rs`

**Behaviour:**
- **When:** shown when there's no `porter.toml`.
- **Fields:**
  - project name
  - source adapter (a list from `registry::source_names`)
  - connectors added through sub-forms, one per kind (crawl url + include/exclude; wxr path; wp-rest url + auth type; wp-db dump + wp_root + uploads_dir; umbraco-json paths)
  - destination adapter, url, api_key_env, config_dir
- **Secrets:** each secret-bearing connector shows the env var or keychain name it reads and whether it's currently set ✓/✗. Secrets are never typed into the form.
- **Save:** writes through the same code as `porter init` plus `toml_edit` for connectors/destination, validates, then opens the main screen.

- [ ] **Step 1: Write the failing tests.**
  - `setup_writes_valid_config` (the result loads with `ProjectConfig::load` and validates).
  - `setup_never_has_secret_fields` (the form spec contains no password/token inputs).
  - Snapshots.
- [ ] **Step 2:** Run `cargo test -p porter-tui`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(tui): setup flow replaces interactive init prompts`.

### Task 8: End-to-end and docs

**Files:** `crates/porter-tui/tests/e2e.rs`, `README.md` (TUI section), `docs/user-guide.md`

**Steps:**
- [ ] **Step 1: Write the failing test.** `e2e_analyse_wxr_fixture`: create a project on the WXR fixture, drive `update` with Msgs (`space` on analyse), let the runtime execute the Cmds against a real Session with a TestBackend, and wait for RunDone. Assert the Stages row is Clean or Warnings (not Errors) and the Items panel lists the fixture's posts.
- [ ] **Step 2:** Run it. Expected: FAIL until the wiring is complete.
- [ ] **Step 3:** Fix any wiring.
- [ ] **Step 4:** Document the keys table and the launch rules in README and the user guide.
- [ ] **Step 5:** Run `cargo test --workspace` and clippy. Expected: PASS.
- [ ] **Step 6:** Commit: `docs(tui): keys and usage; e2e test`.
