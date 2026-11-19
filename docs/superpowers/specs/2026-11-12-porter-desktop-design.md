# Porter desktop app design

Status: draft written overnight by Claude under Isaac's instruction to keep building through the hub. Every decision below is Claude's call, flagged for review. What Isaac decided: it must be a native Rust app, and he asked for Zed's UI framework, GPUI.

Depends on: the engine design and the terminal UI design (`2026-11-12-porter-tui-design.md`). Both front ends share one session layer.

## Why

The terminal UI suits running and watching stages. Some migration work is visual or structured, and a terminal handles it badly:
- reading the analysis of an old site
- browsing thousands of pages as a tree
- seeing media
- editing the mapping table by table instead of hand-writing TOML

It is also how people who don't live in a terminal (PMs, other devs) look at a migration. The desktop app covers those.

## Goals

- A native app (`Porter.app` on macOS first; Linux works; Windows untested) built on **GPUI**, with **gpui-component** for widgets (tables, inputs, trees, tabs).
- Open any local porter project, see everything the engine knows, run stages, and edit the mapping with a structured editor.
- Same engine, same behaviour as the CLI and TUI, through a shared session layer.

## Non-goals (v1)

- Hub features: sign-in, push, pull and shared links. The hub design covers them. The desktop app gets a "Hub" item later.
- Rendering migrated pages the way the destination site would. A simple rich-text and layout preview is enough.
- Windows packaging.

## Shared session layer

A new module, `porter-engine::session`, is used by the TUI and the desktop app. *(Claude's call: it replaces the TUI design's "shared run helper")*
- `Session::open(dir) -> Session`, which holds `Project`, `ProjectConfig` and a subscribed `EventSink`.
- `session.run(stage, opts) -> RunHandle` with `cancel()` and a completion future. It runs one stage at a time.
- Query helpers:
  - `overview()`
  - `issues(filter, page)`
  - `items(kind, filter, page)`
  - `item(id)` with all statuses and issues
  - `coverage()`
  - `routes(page)`
  - `media(page)`
- `reload_config()` returns validation messages.

The CLI's stage driving moves here too, so all three front ends share one implementation.

## Windows and navigation

- **Launcher window:** recent projects (stored in the platform config dir), *Open folder…* and *New project…* (the same setup form as the TUI, as a dialog).
- **Project window:** a left sidebar with these sections:
  1. **Overview.** Pipeline strip with the six stages, each showing state, last run and issue counts, plus Run and Cancel buttons. Below it, coverage from the analysis (connectors, what each found, gaps) and a summary of issues by kind.
  2. **Content.** A tree of documents grouped by type, then by parent hierarchy, filterable. Selecting a document shows:
     - the old URL and the new path
     - status per stage
     - fields as a table
     - rich text rendered simply (headings, paragraphs, lists, links shown with where they resolve)
     - the layout as an outline tree of blocks with their `original` builder module
     - the document's issues
  3. **Media.** A thumbnail grid from the content-addressed store, showing alt text, URL variants, usages ("used in 4 documents") and missing-media issues.
  4. **URLs.** A table of old URL → status → resolves to (document, media, redirect, nothing) → new URL. Filters: unresolved, redirect chains, not found. This is where "where did this page go?" is answered.
  5. **Issues.** A table with filters (stage, kind, severity, resolution), bulk resolve with a note, and a jump to the item.
  6. **Mapping.** A structured editor (below).
  7. **Destination.** Schema check results grouped by collection: the expected field, its kind and the issue. Buttons for *Generate schema*, *Write agent prompt* and *Write spec*, which open the output folder.
  8. **Runs.** Run history per stage and the live log of the current run.
- **Top bar:** project name, source → destination, *Run all*, *Load (dry run)*, and *Apply load…*. Apply opens a confirmation sheet naming the destination URL and the counts.

## Mapping editor

- Edits `[mapping]` in `porter.toml` through `toml_edit`, so Isaac's comments and formatting survive. It saves atomically and re-validates on every save.
- **Types table:** one row per source type with its document count. Columns are *to* (a destination collection picked from the parsed destination schema when `config_dir` is set), *path pattern*, *key field/type*, *drop + reason* and *status rules*.
- **Fields table** for the selected type: neutral field path, sample values, fill rate, then *to* (a destination field picker filtered to compatible kinds), *transforms* (an ordered list editor) and *drop + reason*.
- **Blocks table:** observed layout block kinds and builder modules with their counts, then destination block, field map and raw-block fallback.
- **Live feedback:** every edit re-runs `schema check` in the background (it is offline and fast) and shows issues inline next to the row that causes them.

## Architecture

- **New crate `crates/porter-desktop`** (binary `porter-desktop`) on GPUI and gpui-component, both pinned to exact git revisions. GPUI is pre-1.0 and its API changes, so upgrades are deliberate.
- **Build:**
  - GPUI normally needs Xcode's Metal shader compiler. This Mac has only the Command Line Tools, so the crate builds GPUI with its **`runtime_shaders`** feature: shaders are compiled at runtime and no Metal toolchain is needed. *(Claude's call; Isaac can switch once full Xcode is installed)*
  - The crate is **excluded from the default workspace build** (`default-members`), so `cargo build` for the CLI never pulls GPUI. CI builds it in its own macOS job.
- **Engine bridge:** GPUI's executor drives the UI. Engine work runs on a tokio runtime owned by the app. Events cross into GPUI through a channel polled by a GPUI task, which notifies the relevant views. Store queries run off the UI thread.
- **View models** sit next to each view, as plain structs with no GPUI types where possible, so they can be unit-tested without a window.
- **Thumbnails:** media files are decoded off-thread with the `image` crate into GPUI images and cached by hash. Non-images get type icons.
- **Packaging:** `cargo bundle` or dist's macOS app support. An unsigned local build is fine for v1; signing and notarisation come later.

## Honesty in the UI

The rules are the same as the TUI:
- stage states always show the true outcome
- engine errors are shown verbatim
- apply is never the default action
- unresolved links and missing media are visible in the content view where they occur, not only in the issues list

## Testing

- **View-model unit tests:** filters, tree building from parents, mapping edits producing the expected TOML text with comments preserved.
- **GPUI's `TestAppContext`:** tests for the pipeline strip state changes on events, and for the mapping editor's save path.
- **Manual smoke checklist** in the crate README: open the WXR fixture project, run all, browse content, edit the mapping, see the issues update.

## Calls made without Isaac (to check)

- **Window structure:** a launcher plus a project window with eight sidebar sections.
- **Mapping editor:** it is the desktop app's headline feature.
- **Session layer:** `porter-engine::session` is shared by the CLI, TUI and desktop.
- **GPUI build:** `runtime_shaders` instead of requiring full Xcode, and the desktop crate is out of the default workspace build.
- **Scope:** macOS first, unsigned builds in v1, no hub features in v1.
