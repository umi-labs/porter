# Porter terminal UI design

Status: draft written overnight by Claude under Isaac's instruction to keep building through the terminal UI, desktop app and hub. Every decision below is Claude's call, flagged for Isaac's review. Nothing here was discussed in detail with him beyond "take note from lazygit and lazydocker".

Depends on: `docs/superpowers/specs/2026-11-11-porter-engine-design.md` (the engine, the project store and the event stream).

## Why

Porter 1.x drove everything through one-question-at-a-time prompts (`dialoguer`). A migration is long-running and stateful: stages run for minutes, issues pile up, and you jump between "what failed", "why", "fix the mapping", "re-run that stage". lazygit and lazydocker handle the same shape of work: a few panels of lists on the left, a detail view on the right, every action one key away, and a live log of what the tool is doing. Porter's terminal UI copies that model.

## Goals

- **One screen that shows the whole migration:** project, stages and their state, issues, items, and live progress while a stage runs.
- **Every common action is one key:** run a stage, run everything, dry-run or apply the load, resolve an issue, open the report, edit the config.
- **The engine as a library, not a shell-out.** The TUI calls `porter-engine` directly and subscribes to its `EventSink`. It shows exactly what the CLI's `--json` would print, so the two can't disagree.
- **Works over SSH, in tmux, at 80×24,** with `NO_COLOR`, and with the terminal's own colours.
- **Replaces the interactive prompts:** `porter init --interactive` and the first-run setup happen inside the TUI.

## Non-goals

- Editing the mapping inside the TUI. `e` opens `porter.toml` in `$EDITOR`, and the TUI reloads and re-validates on return. A structured mapping editor belongs to the desktop app.
- Charts, rendering rich text or previewing pages. The TUI links to the HTML reports and the desktop app instead.
- Remote projects. The hub has its own design; the TUI works on a local project folder.

## How it's launched

- `porter tui [--project <dir>]`.
- `porter` with no subcommand, when stdin and stdout are a TTY, also opens the TUI. Without a TTY it prints help, as now. *(Claude's call)*
- If there's no `porter.toml` in the project folder, the TUI opens the **setup flow** instead (below).

## Layout (lazygit-style)

```
┌ Project ───────────────────┐┌ Detail ─────────────────────────────────────────┐
│ landmark   wordpress→payload││                                                  │
│ connectors: wp-db, crawl    ││  (depends on the focused panel and selection)    │
├ Stages ────────────────────┤│                                                  │
│ ✓ analyse      2m ago   0 ⚠ ││                                                  │
│ ✓ extract      2m ago  26 ⚠ ││                                                  │
│ ● normalise    41% ███▌     ││                                                  │
│ · schema                    ││                                                  │
│ · load        (dry run)     ││                                                  │
│ · verify                    ││                                                  │
├ Issues  3✗ 38⚠ 12ℹ ────────┤│                                                  │
│ ✗ schema-mismatch      3    ││                                                  │
│ ⚠ missing-media       26    ││                                                  │
│ ⚠ unsupported-block   12    ││                                                  │
├ Items ─────────────────────┤│                                                  │
│ page  Home            done  ││                                                  │
│ page  About           part. ││                                                  │
└────────────────────────────┘└──────────────────────────────────────────────────┘
┌ Log ───────────────────────────────────────────────────────────────────────────┐
│ 12:03:11 normalise started (run 01J…)                                          │
│ 12:03:14 ⚠ missing-media: wp-content/uploads/2019/04/foo-300x200.jpg            │
└────────────────────────────────────────────────────────────────────────────────┘
 space run · R run all · l load (dry) · L load apply · / filter · e edit config · ? help · q quit
```

- **Left column: four panels.** Project, Stages, Issues and Items. Each is a scrollable list. Tab and Shift-Tab, or 1–4, move focus. The focused panel gets the accent border.
- **Right: Detail.** What it shows depends on the selection:
  - **Stage:** last run summary (counts by status, issue counts, duration, porter version), and a live progress bar and label while running.
  - **Issue kind:** the list of issues of that kind with message, item and resolution. Enter on one opens its details JSON and the item.
  - **Item:** source identity, statuses per stage, its issues, and a pretty-printed slice of its normalised JSON (title, type, path, parent, fields summary).
  - **Project:** connectors with their coverage from the analysis, destination and config validation messages.
- **Bottom: Log.** The live event stream rendered as lines (progress collapses into the stage's bar rather than spamming the log). Toggle with `@` (lazygit's command-log key). *(Claude's call)*
- **Footer:** the keys that apply to the focused panel, like lazygit.
- **Narrow terminals:** below 100 columns, Detail replaces the left column when you press Enter, and Esc goes back (lazydocker's behaviour). Below 80×24, the TUI shows "terminal too small" rather than a broken layout.

## Keys

| Key | Where | Action |
|---|---|---|
| `j`/`k`, `↓`/`↑` | lists | move |
| `Tab`/`Shift-Tab`, `1`–`4` | anywhere | focus panel |
| `Enter` | lists | open in Detail / drill in |
| `Esc` | anywhere | back / close modal |
| `space` | Stages | run the selected stage |
| `R` | anywhere | run all stages (`porter run`), stopping at the first failing stage |
| `l` | anywhere | load, dry run |
| `L` | anywhere | load, apply. Shows a confirmation modal naming the destination URL and item counts; typing `y` confirms |
| `v` | anywhere | verify |
| `c` / `Ctrl-C` | while running | cancel the running stage (resumable, exit-3 semantics); a second `Ctrl-C` quits |
| `/` | lists | filter (fuzzy, incremental) |
| `f` | Issues/Items | cycle filter: all → open → errors only |
| `x` | an issue | resolve: modal with *handled manually* (requires a note) / *won't fix* / *reopen* |
| `o` | anywhere | open the relevant HTML report (`porter report` first if stale) in the browser |
| `e` | anywhere | open `porter.toml` in `$EDITOR`; on return, reload and show validation errors |
| `y` | Items/Issues | copy item id / issue id to the clipboard (OSC 52, so it works over SSH) |
| `@` | anywhere | toggle the log |
| `?` | anywhere | help overlay listing every key |
| `q` | anywhere | quit; if a stage is running, asks first |

All keys are configurable later. Not in v1. *(Claude's call)*

## Setup flow (replaces `dialoguer`)

When there's no `porter.toml`, the TUI opens a single-screen form instead of the panels:
- project name
- source adapter (list from `registry::source_names`)
- connectors to add, each with its own small form: crawl URL, WXR path, REST URL and auth type, DB dump path and `wp_root`, Umbraco files
- destination (Payload URL, API key env var name, `config_dir`)

Secrets are never typed into the form. The form shows which environment variable or keychain entry to set, and checks whether it's present. Saving writes `porter.toml` through the same code path as `porter init` and opens the main screen. *(Claude's call)*

## Architecture

- New crate `crates/porter-tui` (library + `porter tui` subcommand wired from `porter-cli`), using `ratatui` with the `crossterm` backend.
- **App state machine:**
  - `App { project, panels, focus, modal, running: Option<RunningStage>, log: VecDeque<LogLine>, ... }`
  - pure `update(&mut App, Msg) -> Vec<Cmd>` and `view(&App, &mut Frame)`, in the Elm style that ratatui apps use
  - `Msg` comes from key events, engine `Event`s and timers
  - `Cmd` is run stage, cancel, refresh, open editor, open browser, and so on
- **Engine integration:**
  - Stages run on a tokio task calling the same functions the CLI uses. One shared `run_stage` helper is extracted from `porter-cli` into `porter-engine`, so the CLI and TUI drive stages identically.
  - Events come from `EventSink::subscribe`; `Lagged` shows a "dropped N events" log line.
  - Lists re-query the store after each `StageFinished`, and at most every 500 ms while a stage runs (`Store::overview`, `issues`, and a paged `items` query added to the store if needed).
- Only one stage runs at a time per project, the same as the CLI. A second `space` while running shows "a stage is already running".
- **Store access** stays synchronous (std Mutex). The TUI calls it from `spawn_blocking` so the render loop never stalls on SQLite.
- **Paging:** projects can have tens of thousands of items, so the Items panel pages 500 at a time.

## Honesty in the UI

- A stage's row shows its real exit meaning:
  - ✓ finished clean
  - ⚠ finished with warnings
  - ✗ finished with error issues or failed items
  - ⏸ interrupted (resumable)
  - ‼ could not run (with the error)
- Never a green tick on a run that opened error issues.
- The load-apply confirmation states the target URL and that it writes to a live CMS. The default action is the dry run.
- Errors from the engine are shown verbatim in the Detail pane, not swallowed into a status bar.

## Testing

- **State machine:** unit tests feed `Msg`s and assert on state and `Cmd`s, for example: `space` on Stages emits `Cmd::RunStage(Analyse)`, `L` opens the confirm modal, `y` emits apply.
- **Rendering:**
  - snapshot tests with ratatui's `TestBackend` and `insta` for each panel and modal at 120×40 and 80×24
  - the "too small" screen
- **End-to-end:** run analyse on the WXR fixture through the TUI's command path with a fake terminal, and assert that the Stages panel ends in ✓ and the Items panel lists the posts.

## Calls made without Isaac (to check)

- Bare `porter` in a TTY opens the TUI.
- Four left panels (Project, Stages, Issues, Items) plus a Detail pane and a toggleable Log.
- Key choices, most of them lazygit-flavoured:
  - `space` runs a stage
  - `R` runs all
  - `l` / `L` are the dry-run and apply load
  - `x` resolves an issue
  - `@` toggles the log
- The mapping is edited in `$EDITOR`, not in the TUI.
- The setup form replaces `porter init --interactive` prompts. Secrets go through env/keychain only.
- A shared stage-driving helper moves from `porter-cli` into `porter-engine` so the CLI, TUI and desktop app behave the same.
