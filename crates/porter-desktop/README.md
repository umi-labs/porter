# porter-desktop

The native desktop app for Porter, built on Zed's [GPUI](https://gpui.rs) with
[gpui-component](https://github.com/longbridge/gpui-component) for widgets.

It is **not** in the default workspace build, so `cargo build` for the CLI never compiles GPUI.

```sh
cargo build -p porter-desktop
cargo run -p porter-desktop
cargo test -p porter-desktop
```

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
