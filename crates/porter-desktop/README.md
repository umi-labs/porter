# porter-desktop

The native desktop app for Porter, built on Zed's [GPUI](https://gpui.rs) with
[gpui-component](https://github.com/longbridge/gpui-component) for widgets.

It is **not** in the default workspace build, so `cargo build` for the CLI never compiles GPUI.

```sh
cargo build -p porter-desktop
cargo run -p porter-desktop
cargo test -p porter-desktop
```

## Installing

```sh
brew install --cask umi-labs/tap/porter-desktop
```

Each `v*` release gets `Porter_<version>_aarch64.dmg` and `Porter_<version>_x86_64.dmg` from
`.github/workflows/desktop-release.yml`, which also updates the cask in `umi-labs/homebrew-tap`.

## Building the app locally

```sh
cargo install cargo-packager --locked
cargo build --release -p porter-desktop && cargo packager -p porter-desktop --release
open target/packager/Porter.app        # the .dmg is next to it
```

Config is `[package.metadata.packager]` in `Cargo.toml` (bundle id `uk.co.umidigital.porter`,
minimum macOS 11.0). There is no icon yet. The app is **ad-hoc signed and not notarised**: Umi has
no Apple Developer ID yet. The cask clears the quarantine flag on install; for a DMG downloaded by
hand, clear it yourself:

```sh
xattr -dr com.apple.quarantine /Applications/Porter.app
```

Quitting with Cmd-Q while a stage runs cancels it first. GPUI gives quit handlers only 100 ms
and cannot ask a question, so a writing load is cancelled without the confirmation the window's
close button shows; it stops at the next safe point and can be resumed.
Closing a window waits up to 10 s for a running stage to stop; past that the window closes
anyway and the stage stops at its next safe point.

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
5. Confirm the default build is still GPUI-free: `cargo tree -p porter-cli -e normal -i gpui`
   must report no match.

## Smoke checklist

- `cargo run -p porter-desktop` shows the launcher with recent projects.
- *Open folder…* on a porter project opens a project window and adds it to the recent list.
- *Open folder…* on a folder that is not a project shows the error verbatim in a dialog.
- The project window shows the sidebar (eight sections), the top
  bar (project name, source → destination, *Run all*, *Load (dry run)*, *Apply load…*) and the
  Overview: six stage cards, coverage, open issues by kind.
- *Run* on a stage flips its card to running, shows progress, and ends in the true state (a
  run with error issues is red, never green). *Cancel* stops it; it ends interrupted.
- *Runs* shows the live log (newest first, the last 2,000 lines) and the history per stage;
  a dry-run load is labelled "(dry run)".
- *Apply load…* checks three gates: a fresh dry run (no porter.toml section load reads changed,
  and no earlier stage re-ran since), no open errors, and warnings acknowledged. A stale or
  missing dry run offers *Run dry run now*. It lists what will be written and what will not, and
  *Apply* stays disabled until the destination host is typed (paste disabled); Cancel has focus
  and Enter never applies.
- *Media* lists the files with thumbnails and which items use them; *URLs* lists the routes
  and filters them (unresolved, chains, not found); *Issues* lists open issues and bulk
  actions say what actually happened; *Destination* shows the mode and opens the output folder.
- Closing a project window during a run waits for it to stop (up to 10 s) rather than losing it.
- Opening the same project twice (recent list, then *Open folder…*) brings the existing window
  to the front instead of opening a second one.

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
| Media, URLs, Issues, Destination sections load and behave | not yet run (needs a display) |
| Window close during a run waits for it to stop (up to 10 s) | not yet run (needs a display) |
| Opening the same project twice activates the existing window | not yet run (needs a display) |
| Cmd-Q and window close during a run cancel it | not yet run (needs a display) |
| `open target/packager/Porter.app` launches (after the `xattr` step) | launches and quits cleanly (2026-11-19, cargo-packager build, arm64) |
