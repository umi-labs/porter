# Porter desktop app: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A native GPUI desktop app over the porter engine, with these screens:
- launcher
- project window with Overview, Content, Media, URLs, Issues, Mapping editor, Destination, Runs

**Architecture:**
- New crate `crates/porter-desktop` on `gpui` and `gpui-component`, both pinned to exact git revisions. It is excluded from the workspace's `default-members`.
- It uses `porter-engine::session` (built in the TUI plan's Task 1; that must land first).
- View models are plain structs, unit-tested without a window. Views are thin GPUI renderers.

**Tech stack:**
- `gpui` with the `runtime_shaders` feature (no Metal toolchain on this machine)
- `gpui-component`
- tokio (engine runtime)
- `image` (thumbnails)
- `toml_edit` (mapping editor)

**Spec:** `docs/superpowers/specs/2026-11-12-porter-desktop-design.md`

## Global Constraints

- The engine constraints still hold (honesty, no secrets, apply never the default).
- The UI thread never blocks on SQLite, the network or image decoding.
- Mapping edits go only through `toml_edit`. User comments and formatting survive, writes are atomic, and the config is re-validated after every save.
- `cargo build` (default members) must not compile GPUI. `cargo build -p porter-desktop` builds the app.
- `cargo fmt --all`; clippy at warn level.

## Review Focus

1. **Mapping-editor round-trip:** editing one field must change only that line of `porter.toml`. Pinned in Task 6 (`edit_preserves_other_lines`).
2. **Event floods** while a stage runs must not freeze the UI: coalesced like the TUI. Pinned in Task 3.
3. **50k-document projects in the content tree:** lazy children, paged. Pinned in Task 4.
4. **Missing or broken media files in the thumbnail grid:** placeholders, never a crash. Pinned in Task 5.
5. **GPUI revision drift:** the build pins exact revisions; the README documents the upgrade procedure. Pinned in Task 1.

---

### Task 1: Crate, pinned GPUI, launcher window

**Files:**
- Create: `crates/porter-desktop/{Cargo.toml,src/main.rs,src/app.rs,src/launcher.rs,src/recent.rs,README.md}`
- Modify: root `Cargo.toml` (`default-members` excludes porter-desktop), `.github/workflows/ci.yml` (a macOS job building porter-desktop)

**Interfaces:**
- `recent::load() -> Vec<RecentProject>` and `recent::push(path)`, stored in the platform config dir as JSON, max 20, de-duplicated.
- The launcher opens a project window via `Session::open`; an error is shown verbatim in a dialog.

**Steps:**
- [ ] **Step 1:** Failing tests: `recent_dedupes_and_caps`, `recent_survives_corrupt_file` (a corrupt file → empty list plus a log line, no panic).
- [ ] **Step 2:** Run `cargo test -p porter-desktop`. Expected: FAIL.
- [ ] **Step 3:** Implement. Pin `gpui` and `gpui-component` to exact revisions known to build together. Enable `runtime_shaders`. Document the pin and how to upgrade it in README.md.
- [ ] **Step 4:**
  - `cargo build -p porter-desktop` and the tests pass.
  - `cargo build` (default members) does not compile gpui (check with `cargo tree -e normal -i gpui` from the default members).
  - Launch it manually and see the launcher.
- [ ] **Step 5:** Commit: `feat(desktop): crate with pinned GPUI and launcher window`.

### Task 2: Engine bridge and project window shell

**Files:** `crates/porter-desktop/src/{bridge.rs,project_window.rs,sidebar.rs}`

**Interfaces:**
- `Bridge::new(session: Arc<Session>) -> (Bridge, Receiver<UiEvent>)`
- `UiEvent { Engine(Event), Data(DataResult), RunDone(RunOutcome) }`
- A GPUI task polls the receiver at most every 50 ms and coalesces Progress events.
- Sidebar sections as listed in the spec. The top bar has Run all, Load (dry run) and Apply load…

**Steps:**
- [ ] **Step 1:** Failing tests (view model): `coalesces_progress`, `sidebar_selection_switches_view`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(desktop): engine bridge and project window shell`.

### Task 3: Overview and Runs

**Files:** `crates/porter-desktop/src/views/{overview.rs,runs.rs}`, `src/vm/{pipeline.rs,runs.rs}`

**Behaviour:**
- **Pipeline strip:** the six stages, with the same `StageState` derivation as the TUI. Extract it into `porter-engine::session` as `stage_state(&Overview, Stage)` so both UIs share it.
- **Overview:** Run and Cancel per stage; coverage table; issues by kind.
- **Runs:** history per stage, plus the live log of the current run (a bounded list of 2,000).
- **Apply load:** a confirm sheet that requires typing the destination host.

**Steps:**
- [ ] **Step 1:** Failing tests: `pipeline_states` (including that error issues are never green), `apply_sheet_requires_host`, `log_bounded`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(desktop): overview pipeline and runs`.

### Task 4: Content browser

**Files:** `crates/porter-desktop/src/views/content.rs`, `src/vm/content.rs`

**Behaviour:**
- **Tree:** by type, then the parent hierarchy, with lazy children loaded per node through a new `Session::children(parent: Option<ItemId>, doc_type, page)` paged query.
- **Detail panel:**
  - old URL and new path
  - statuses
  - fields table
  - simple rich-text rendering, with links showing their resolution (resolved → title; unresolved → red)
  - layout outline
  - the document's issues

**Steps:**
- [ ] **Step 1:** Failing tests: `tree_is_lazy` (expanding a node issues one paged query), `unresolved_links_marked`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement. Add `Session::children` with a test in porter-engine.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(desktop): content browser`.

### Task 5: Media and URLs

**Files:** `crates/porter-desktop/src/views/{media.rs,urls.rs}`, `src/thumbs.rs`

**Behaviour:**
- **Media:** a virtualised thumbnail grid. `thumbs::Cache` decodes off-thread with `image` (max 256 px), caches by hash with an LRU of 512, and shows a placeholder plus the reason for missing or undecodable files.
- **Media detail:** variants, usages, issues.
- **URLs:** a virtualised table of old URL, status, target and new URL, with filters (unresolved, chains, 404).

**Steps:**
- [ ] **Step 1:** Failing tests: `thumb_missing_file_placeholder`, `thumb_corrupt_image_placeholder`, `thumb_cache_bounded`, `urls_filter_unresolved`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(desktop): media grid and URL table`.

### Task 6: Mapping editor

**Files:** `crates/porter-desktop/src/views/mapping.rs`, `src/vm/mapping.rs`, `crates/porter-engine/src/mapping/edit.rs`

**Interfaces:** `mapping::edit::MappingDoc::open(path) -> MappingDoc`. It has `set_type_to(src_type, Option<&str>)`, `set_path(...)`, `set_field(src_type, field, FieldMapEdit)`, `drop_field(...)`, `set_block(...)` and `save() -> Vec<String>` (validation messages). It wraps `toml_edit`, saves atomically, and the core module names no platform.

**Behaviour:**
- **Pickers:** the destination collection and field pickers come from `read_payload_config` when `config_dir` is set; otherwise free text with a note.
- **Live checking:** a background `schema check` runs after each save, and its issues are shown inline per row.

**Steps:**
- [ ] **Step 1:** Failing tests:
  - `edit_preserves_other_lines`: a fixture `porter.toml` with comments; one edit changes exactly one line.
  - `drop_requires_reason_optional_but_kept`
  - `save_is_atomic`
  - `validation_messages_returned`
- [ ] **Step 2:** Run `cargo test -p porter-engine mapping::edit` and the desktop view-model tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(desktop): structured mapping editor`.

### Task 7: Issues and Destination

**Files:** `crates/porter-desktop/src/views/{issues.rs,destination.rs}`

**Behaviour:**
- **Issues:** a filterable table, bulk resolve with a note, and a jump to the item.
- **Destination:** schema check results by collection, plus buttons for Generate, Prompt and Spec that open the output folder.

**Steps:**
- [ ] **Step 1:** Failing tests: `bulk_resolve_requires_note_for_handled`, `destination_groups_by_collection`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(desktop): issues and destination views`.

### Task 8: Packaging and smoke

**Files:** `crates/porter-desktop/README.md` (build, run, smoke checklist), `dist-workspace.toml` or a `cargo bundle` config for an unsigned `.app`

**Steps:**
- [ ] **Step 1:** Build the `.app`.
- [ ] **Step 2:** Run the smoke checklist against the WXR fixture project: open, run all (dry), browse content, edit the mapping, see the issues update. Record the results in the README.
- [ ] **Step 3:** Commit: `chore(desktop): unsigned app bundle and smoke checklist`.
