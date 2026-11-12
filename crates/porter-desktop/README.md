# porter-desktop

The native desktop app for Porter, built on Zed's [GPUI](https://gpui.rs) with
[gpui-component](https://github.com/longbridge/gpui-component) for widgets.

It is **not** in the default workspace build, so `cargo build` for the CLI never compiles GPUI.

```sh
cargo build -p porter-desktop
cargo run -p porter-desktop
cargo test -p porter-desktop
```

## Unsigned app bundle

```sh
crates/porter-desktop/scripts/bundle.sh          # release build -> target/Porter.app
PROFILE=debug crates/porter-desktop/scripts/bundle.sh   # faster, larger
open target/Porter.app
```

The script builds `-p porter-desktop` and assembles `Contents/MacOS/porter-desktop` plus an
`Info.plist` (bundle id `uk.co.umidigital.porter`, version from Cargo, minimum macOS 11.0).
There is no icon yet. The bundle is **unsigned and not notarized**; for local use, clear the
quarantine flag if it was copied from another machine or downloaded:

```sh
xattr -dr com.apple.quarantine target/Porter.app
```

(`dist`, the release tool this repo uses for the CLI, only makes shell, PowerShell, npm,
Homebrew and MSI installers; it has no macOS `.app` target, hence the script.)

Quitting with Cmd-Q while a stage runs cancels it first. GPUI gives quit handlers only 100 ms
and cannot ask a question, so a writing load is cancelled without the confirmation the window's
close button shows; it stops at the next safe point and can be resumed.

## Build notes

- GPUI is built with its `runtime_shaders` feature: Metal shaders are compiled when the app
  starts, so only the Xcode Command Line Tools are needed (no `metal` compiler). With full Xcode
  installed you can drop the feature to precompile shaders at build time.
- Recent projects live in `<config dir>/porter/recent.json` (`~/Library/Application Support` on
  macOS); a corrupt file is ignored with a line on stderr.

## Pinned GPUI

| crate            | pin       |
| ---------------- | --------- |
| `gpui`           | `=0.2.2`  |
| `gpui-component` | `=0.5.1`  |

Both are exact crates.io pins in `Cargo.toml`. GPUI is pre-1.0 and changes its API between
releases, so a bump is a deliberate change. gpui-component 0.6 and later depend on
`gpui-pre` (a fork) instead of `gpui`; 0.5.1 is the newest release that builds on
Zed's published `gpui`.

To upgrade:

1. Pick a gpui-component release and read the `gpui` version it requires:
   `curl -s https://crates.io/api/v1/crates/gpui-component/<ver>/dependencies | jq '.dependencies[] | select(.crate_id|startswith("gpui"))'`.
2. Set both `=` pins in `crates/porter-desktop/Cargo.toml` to that pair (if it needs a git
   GPUI, pin both to exact `rev = "..."` instead).
3. Check `runtime_shaders` still exists: `cargo info gpui@<ver> -v | grep runtime_shaders`.
4. `cargo build -p porter-desktop && cargo test -p porter-desktop`, fix API breaks, then launch
   it and run the smoke checklist below.
5. Confirm the default build is still GPUI-free: `cargo tree -i gpui` (default members)
   must report no match.

## Smoke checklist

- `cargo run -p porter-desktop` shows the launcher with recent projects.
- *Open folder…* on a porter project opens a project window and adds it to the recent list.
- *Open folder…* on a folder that is not a project shows the error verbatim in a dialog.
- The project window shows the sidebar (eight sections; those not built yet say so), the top
  bar (project name, source → destination, *Run all*, *Load (dry run)*, *Apply load…*) and the
  Overview: six stage cards, coverage, open issues by kind.
- *Run* on a stage flips its card to running, shows progress, and ends in the true state (a
  run with error issues is red, never green). *Cancel* stops it; it ends interrupted.
- *Runs* shows the live log (newest first, the last 2,000 lines) and the history per stage;
  a dry-run load is labelled "(dry run)".
- *Apply load…* without a dry run, or after editing `porter.toml` or re-running normalise,
  refuses. After a fresh *Load (dry run)* it lists the counts, and *Apply* stays disabled until
  the destination host is typed; Enter never applies.

### Smoke results

Built `target/Porter.app` (release) and checked its structure and `Info.plist` (`plutil -lint`
OK). The build machine has no display, so **nothing below has been run**; every item is
*not yet run (needs a display)*. Run them against the WXR fixture project
(`crates/porter-engine/tests/fixtures/wxr/basic.xml`) before shipping.

| item | result |
| ---- | ------ |
| Launcher shows recent projects | not yet run (needs a display) |
| Open folder on a porter project opens a window and adds it to recent | not yet run (needs a display) |
| Open folder on a non-project shows the error verbatim | not yet run (needs a display) |
| Project window: sidebar, top bar, Overview | not yet run (needs a display) |
| Run all (dry) on the WXR fixture ends in true states, never green with errors; Cancel interrupts | not yet run (needs a display) |
| Browse content (list, detail) | not yet run (needs a display) |
| Edit the mapping, save; issues update | not yet run (needs a display) |
| Runs: live log and history, dry run labelled | not yet run (needs a display) |
| Apply load…: refuses without a fresh dry run; Enter never applies | not yet run (needs a display) |
| Cmd-Q and window close during a run cancel it | not yet run (needs a display) |
| `open target/Porter.app` launches (after the `xattr` step) | not yet run (needs a display) |
