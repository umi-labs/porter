# Porter 2.0 engine, phase 1: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild porter as a headless, honest, re-runnable migration engine that takes WordPress (and Umbraco) through analyse → extract → normalise → schema → load → verify into Payload, and passes the Landmark acceptance run. Ships as porter 2.0.

**Architecture:** A Cargo workspace of three crates:
- `porter-model`: the neutral model types; no I/O.
- `porter-engine`:
  - the project store: SQLite plus content-addressed files
  - stages, events and connectors
  - the source adapters (WordPress, Umbraco) and the destination adapter (Payload)
  - mapping, verify and reports
- `porter-cli`: a thin `porter` binary.

The old 1.x code moves to `legacy/` (outside the workspace) as reference and is deleted in the last task.

**Tech Stack:** Rust 2024 edition (rustc 1.97), tokio, reqwest 0.12 (rustls, multipart, stream), rusqlite (bundled), serde/serde_json, toml, clap 4, uuid (v5, v7), sha2, quick-xml, scraper, swc (current release line), tokio-util `CancellationToken`, self_update, keyring. Docker (MariaDB + `wordpress:cli`) for the DB-dump connector.

**Spec:** `docs/superpowers/specs/2026-11-11-porter-engine-design.md`. Read it before any task. Where this plan is silent, the spec decides.

**Reference implementation:** `~/Desktop/work/clients/landmark/migration/` (TypeScript, read-only). Tasks say which file to port. Port the *rules*, not client data: test fixtures are rewritten as synthetic content, and no Landmark text, names, emails or URLs go into this repo.

**Deferred to later phase plans** (the spec includes them; this plan doesn't build them):
- the SSH/SFTP server-access connector (use local paths to rsynced files for now)
- the Elementor and WPBakery converters (detected and reported as `converter-missing`)
- the generic public-site-only source
- Sanity, Webflow, Squarespace and Gatsby
- the Sanity and WordPress destinations
- the terminal UI, desktop app and hub

## Global Constraints

- Nothing in `porter-model` or the engine core (`store`, `stage`, `events`, `mapping`, `verify`, `report`) names a platform. Platform knowledge lives only in `sources/<name>/` and `destinations/<name>/`.
- Every item leaves every stage it enters with exactly one `ItemStatus`: `Done`, `Partial { reason }`, `Skipped { reason }` or `Failed { error }`. A code path that drops an item without a status is a bug.
- Issue kinds are exactly these (kebab-case on the wire):
  - `unresolved-link`
  - `missing-media`
  - `unmapped-field`
  - `unsupported-block`
  - `unresolved-relation`
  - `parent-missing`
  - `redirect-chain`
  - `redirect-loop`
  - `write-failed`
  - `access-gap`
  - `converter-missing`
  - `schema-mismatch`
- Exit codes:
  - `0`: finished, and this run opened no error-severity issues
  - `1`: could not run
  - `2`: finished, with error issues or failed items
  - `3`: interrupted, resumable
- With `--json`, stdout carries only NDJSON events. All human text and logs go to stderr.
- Never prompt unless stdin is a TTY **and** `--interactive` is set.
- Credentials never go in `porter.toml`. They come from env vars named in config, or from a keyring entry named in config. WordPress env names stay the ones 1.x introduced: `PORTER_WP_AUTH_TYPE`, `PORTER_WP_BEARER_TOKEN`, `PORTER_WP_USERNAME`, `PORTER_WP_PASSWORD`, `PORTER_WP_API_KEY`, `PORTER_WP_API_KEY_HEADER`. Payload: `PORTER_PAYLOAD_API_KEY`.
- Item IDs are UUIDv5 over `"{kind}|{connector}|{source_id}"` in the namespace `uuid!("6f1c2b0e-4d8a-4c1e-9b7a-0b6f2e9d5a11")`. Other store rows (issues, runs) are UUIDv7. Every table has `updated_at`, and every mutable table has `deleted_at`.
- `load` is a dry run unless `--apply` is given.
- `porter clean` deletes only `raw/`, `media/`, `reports/` and `project.db` inside the project folder.
- `cargo build` must succeed with fresh dependency resolution (`cargo update` then build), not only with the lockfile.
- Clippy runs in CI at warn level. New crates add `#![deny(clippy::unwrap_used)]` outside tests.
- Version `2.0.0`. No 1.x flags (`--source`, `--target`, `--collection`) survive.

## Review Focus

1. **URL identity.** The same page shows up as `http://`/`https://`, with or without `www.`, with or without a trailing slash, percent-encoded or not, in different case, or with tracking query strings. Route matching, redirects and link resolution must treat these as one URL, while keeping a meaningful query string (`?p=123`) distinct. Pinned in Task 4 (`normalise_url` tests) and Task 16.
2. **Interrupted or failing load.** Ctrl-C, or a Payload 429/5xx halfway through, must exit 3 (or 2), leave a consistent store, and on re-run update rather than duplicate. Pinned in Task 21 (`load_resumes_without_duplicates`, `load_retries_then_fails_item`).
3. **Media collisions.** Different files whose names differ only in case, the same file under several size-variant URLs, and two different files with the same name in different upload folders. Pinned in Task 5 (`same_bytes_dedupe`, `case_collision_kept_apart`) and Task 13.
4. **Broken hierarchy.** A missing parent, a parent cycle, or a child extracted before its parent must produce `parent-missing` issues and load deterministically. Pinned in Task 16 (`parent_cycle_is_issue`) and Task 21 (`children_after_parents`).
5. **Malformed and large source files.** WXR with multi-line items, CDATA containing `]]>` split across sections, invalid UTF-8 bytes, and files over 500 MB must stream without loading whole files into memory and must report bad items instead of aborting. Pinned in Task 10 (`wxr_streams_multiline_and_cdata`, `wxr_bad_item_is_failed_not_fatal`).

---

## File structure

```
Cargo.toml                         # [workspace] members = ["crates/*"]
crates/porter-model/src/
  lib.rs  ids.rs  status.rs  issue.rs  document.rs  richtext.rs  layout.rs
  media.rs  route.rs  site.rs
crates/porter-engine/src/
  lib.rs
  config.rs            # porter.toml types + validation
  project.rs           # Project: open/init, paths, clean
  store/mod.rs store/schema.sql store/items.rs store/issues.rs store/runs.rs store/media.rs
  events.rs            # Event, EventSink, Summary, exit_code
  stage.rs             # StageCtx, run_stage(), traits Connector / Source / Destination
  url.rs               # normalise_url, UrlKey
  http.rs              # shared reqwest client: retries, backoff, politeness
  report/mod.rs report/analysis.rs report/extract.rs report/verify.rs
  mapping/mod.rs mapping/transforms.rs mapping/paths.rs
  verify.rs
  connectors/crawl.rs
  sources/wordpress/{mod.rs, rest.rs, wxr.rs, db.rs, export.php, analyse.rs, normalise.rs,
                     shortcodes.rs, texturize.rs, autop.rs, html.rs, divi.rs, gutenberg.rs,
                     acf.rs, seo.rs, forms.rs, redirects.rs}
  sources/umbraco/mod.rs
  destinations/payload/{mod.rs, client.rs, lexical.rs, blocks.rs, schema_check.rs,
                        schema_generate.rs, schema_prompt.rs, load.rs, ts/ (ported swc parser)}
crates/porter-engine/tests/fixtures/...   # synthetic only
crates/porter-cli/src/main.rs  crates/porter-cli/src/commands/*.rs
crates/porter-cli/tests/cli.rs
tests/payload-app/                 # minimal Payload 3 + db-sqlite app for golden tests
legacy/                            # old 1.x src, deleted in Task 24
```

---

## Milestone A: core

### Task 1: Workspace, legacy move, CI

**Files:**
- Modify: `Cargo.toml` (becomes a workspace)
- Create: `crates/{porter-model,porter-engine,porter-cli}/Cargo.toml` and `src/lib.rs` / `src/main.rs`
- Move: `src/` → `legacy/src/`, `tests/` → `legacy/tests/`. Exclude both from the workspace.
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: crate names `porter_model`, `porter_engine`, and a binary `porter`.

- [ ] **Step 1:** Create the workspace with `resolver = "3"`, workspace-level `[workspace.package] version = "2.0.0", edition = "2024"`, and shared `[workspace.dependencies]` for the stack above. Keep `[profile.dist]`. Update `dist-workspace.toml` to point at `crates/porter-cli`.
- [ ] **Step 2:** `porter-cli` `main.rs` prints `porter 2.0.0` for `--version`.
- [ ] **Step 3:** CI on `push` and `pull_request` (branches `main` and `prod`) runs:
  - `cargo build --workspace --locked`
  - `cargo test --workspace --locked`
  - `cargo clippy --workspace --all-targets` (no `-D warnings`)
  - a `fresh-resolve` job: `cargo update && cargo build --workspace`

  Remove the udeps and duplication steps.
- [ ] **Step 4:** Run `cargo build --workspace && cargo run -p porter-cli -- --version`. Expected: `porter 2.0.0`.
- [ ] **Step 5:** Commit: `chore: workspace for porter 2.0; 1.x code moved to legacy/`.

### Task 2: Model: ids, status, issues, documents, site data

**Files:**
- Create: `crates/porter-model/src/{ids.rs,status.rs,issue.rs,document.rs,media.rs,route.rs,site.rs}`
- Test: inline `#[cfg(test)]` in each file

**Interfaces:**
- Produces:
  - `ItemKind`, an enum: `Document | Media | Route | Redirect | Menu | Taxonomy | Term | Form | Submission | Settings | Author`
  - `SourceRef { connector: String, source_id: String, url: Option<String> }`
  - `ItemId(Uuid)`, with `ItemId::new(kind: ItemKind, src: &SourceRef) -> ItemId`
  - `Stage`, an enum: `Analyse | Extract | Normalise | Schema | Load | Verify`
  - `ItemStatus`, an enum: `Done | Partial { reason: String } | Skipped { reason: String } | Failed { error: String }`
  - `Severity`, an enum: `Error | Warning | Info`
  - `IssueKind` (the 12 kinds from the constraints)
  - `Resolution`, an enum: `Open | FixedByRerun | HandledManually { note: String } | WontFix`
  - `Issue { id: Uuid, kind: IssueKind, severity: Severity, item: Option<ItemId>, stage: Stage, message: String, details: serde_json::Value, resolution: Resolution }`
  - `Document`, with the fields listed in the spec ("The neutral model" → Document):
    - `fields: IndexMap<String, FieldValue>`
    - `original: serde_json::Value`
    - `translations: Vec<ItemId>`
  - `DocStatus`, an enum: `Published | Draft | Private | Scheduled`
  - `FieldValue`, an enum: `Null | Text(String) | Number(f64) | Bool(bool) | Date(chrono::DateTime<Utc>) | RichText(RichText) | Layout(Vec<Block>) | Media(ItemId) | Doc(ItemId) | List(Vec<FieldValue>) | Object(IndexMap<String, FieldValue>)`
  - `Seo { title, description, canonical, noindex: bool, og_image: Option<ItemId>, og_title, og_description }` (optional fields as `Option<String>`)
  - `Media { id, hash: String, variants: Vec<String>, filename, mime, width: Option<u32>, height: Option<u32>, alt, caption, title, credit, folder: Option<String>, usages: Vec<Usage { item: ItemId, field: String }> }`
  - `Route { url: String, target: RouteTarget, http_status: Option<u16>, title: Option<String> }`, where `RouteTarget` is `Document(ItemId) | Media(ItemId) | Redirect(ItemId) | None`
  - `Redirect { from: String, to: String, code: u16, origin: RedirectOrigin }`, where `RedirectOrigin` is `Plugin | Crawl | Field | Manual`
  - in `site.rs`:
    - `Menu { name, items: Vec<MenuItem { label, target: MenuTarget, children: Vec<MenuItem> }> }`, where `MenuTarget` is `Doc(ItemId) | Url(String)`
    - `Taxonomy { name, hierarchical: bool }` and `Term { taxonomy, name, slug, parent: Option<ItemId> }`
    - `Form { title, fields: Vec<FormField>, notifications: Vec<FormNotification>, confirmation: RichText, retired: bool }`, using `FormField { name, label, kind: String, required: bool, options: Vec<(String,String)>, conditions: serde_json::Value }`
    - `Submission { form: ItemId, created: DateTime<Utc>, values: IndexMap<String,String> }`
    - `Settings { site_name, front_page: Option<ItemId>, permalink_structure: Option<String>, seo_defaults: Seo }`
    - `Author { name, email: Option<String>, slug }`
  - All of these derive `Serialize, Deserialize, Clone, Debug, PartialEq`, with serde `rename_all = "kebab-case"` on enums.

- [ ] **Step 1: Write the failing tests**
  - `item_id_is_stable`: two calls with equal inputs give an equal `ItemId`, and a different `source_id` gives a different one.
  - `issue_kind_wire_names`: `serde_json::to_string(&IssueKind::UnresolvedLink) == "\"unresolved-link\""`, and the same for all 12 kinds.
  - `document_roundtrip`: a `Document` with one of every `FieldValue` variant survives a JSON round trip unchanged.
- [ ] **Step 2:** Run `cargo test -p porter-model`. Expected: FAIL (types missing).
- [ ] **Step 3:** Implement the types above.
- [ ] **Step 4:** Run `cargo test -p porter-model`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(model): ids, statuses, issues, documents, media, routes, site data`.

### Task 3: Model: rich text and layout trees

**Files:**
- Create: `crates/porter-model/src/{richtext.rs,layout.rs}`

**Interfaces:**
- Produces:
  - `RichText { blocks: Vec<RtBlock> }`
  - `RtBlock`, an enum:
    - `Paragraph { inlines: Vec<Inline>, align: Align }`
    - `Heading { level: u8, inlines, align }`
    - `List { ordered: bool, start: u32, items: Vec<Vec<RtBlock>> }`
    - `Quote { blocks: Vec<RtBlock> }`
    - `Code { lang: Option<String>, text: String }`
    - `Rule`
    - `Table { rows: Vec<Vec<Vec<RtBlock>>>, header: bool }`
    - `Media { media: ItemId, caption: Option<RichText>, align: Align, width: Option<u32> }`
    - `Embed { provider: Option<String>, url: String }`
    - `Raw { html: String }`
  - `Align`, an enum: `None | Left | Center | Right | Justify`
  - `Inline`, an enum:
    - `Text { text: String, marks: Marks }`
    - `Link { target: LinkTarget, new_tab: bool, inlines: Vec<Inline> }`
    - `Break`
  - `Marks` is a bitflags-style struct of `bool`s: `bold, italic, underline, strike, code, sub, sup`
  - `LinkTarget`, an enum: `Doc(ItemId) | Media(ItemId) | Url(String)`
  - `Block { kind: BlockKind, props: IndexMap<String, FieldValue>, children: Vec<Block>, original: Option<Original> }`
  - `BlockKind`, an enum: `Section | Row | Column { width: Option<f32> } | RichText | Image | Gallery | Button | Video | Embed | Map | FormRef | Tabs | Accordion | Slider | Spacer | Divider | Raw`
  - `Original { builder: String, module: String, classes: Vec<String>, settings: serde_json::Value }`
  - `RichText::plain_text(&self) -> String`
  - `Block::walk(&self, f: &mut impl FnMut(&Block))`

- [ ] **Step 1: Write the failing tests**
  - `plain_text_flattens`: a heading plus a paragraph with a link gives `"Title\nHello world"`.
  - `walk_visits_depth_first`: section > row > [column > image, column > rich-text] gives the visit order `[Section, Row, Column, Image, Column, RichText]`.
  - `layout_roundtrip`: a JSON round trip, including `original`.
- [ ] **Step 2:** Run `cargo test -p porter-model`. Expected: FAIL.
- [ ] **Step 3:** Implement the types.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(model): neutral rich text and layout trees`.

### Task 4: Project, config, store, URL keys

**Files:**
- Create:
  - `crates/porter-engine/src/{project.rs,config.rs,url.rs}`
  - `store/{mod.rs,schema.sql,items.rs,issues.rs,runs.rs}`
- Test: `crates/porter-engine/tests/store.rs` and inline tests in `url.rs`

**Interfaces:**
- Consumes: Task 2 types.
- Produces:
  - `ProjectConfig`, deserialised from `porter.toml`. Its sections:
    - `project { name }`
    - `source { adapter: String }`
    - `connectors: Vec<ConnectorConfig>`: tagged by `kind`, one of `"crawl" | "wp-rest" | "wxr" | "wp-db" | "umbraco-json"`, each variant with its own fields as in Tasks 9-12 and 18
    - `destination { adapter: String, url: String, api_key_env: String, auth_collection: String = "users", config_dir: Option<PathBuf> }`
    - `mapping: MappingConfig` (Task 15)
    - `redirects: Vec<ManualRedirect { from, to, code: u16 = 301 }>`
    - `verify { site_url: Option<String> }`
  - `ProjectConfig::load(path) -> Result<ProjectConfig>`
  - `ProjectConfig::validate(&self) -> Vec<String>` (empty means valid; each string names the exact missing or invalid key)
  - `Project::init(dir: &Path, name: &str, adapter: &str) -> Result<Project>` writes a minimal commented `porter.toml` and an empty store, and errors if `porter.toml` already exists.
  - `Project::open(dir: &Path) -> Result<Project>`
  - `Project::clean(&self) -> Result<()>`
  - `Project::{raw_dir(connector: &str), media_dir(), reports_dir()} -> PathBuf`
  - `Store` (inside `Project`, `project.store()`), with these methods:
    - `put_item<T: Serialize>(&self, kind: ItemKind, id: ItemId, src: &SourceRef, data: &T) -> Result<()>` upserts by `id`.
    - `get_item<T: DeserializeOwned>(&self, id: ItemId) -> Result<Option<T>>`
    - `items<T: DeserializeOwned>(&self, kind: ItemKind) -> Result<Vec<(ItemId, SourceRef, T)>>`
    - `set_status(&self, run: RunId, stage: Stage, id: ItemId, status: &ItemStatus) -> Result<()>`
    - `status(&self, stage: Stage, id: ItemId) -> Result<Option<ItemStatus>>`
    - `open_issue(&self, run: RunId, issue: Issue) -> Result<()>` dedupes by `(kind, item, stage, message)`; reopening an existing one updates its `run`.
    - `issues(&self, filter: &IssueFilter { stage, kind, severity, resolution: Option<..> }) -> Result<Vec<Issue>>`
    - `resolve_issue(&self, id: Uuid, r: Resolution) -> Result<()>`
    - `begin_run(&self, stage: Stage) -> Result<RunId>`
    - `end_run(&self, run: RunId, summary: &Summary) -> Result<()>`: when a run ends, every `Open` issue of that stage that this run did not reopen becomes `FixedByRerun`. `HandledManually` and `WontFix` are never changed.
  - `RunId(Uuid)`
  - `url::normalise_url(raw: &str, base: Option<&Url>) -> Option<UrlKey>`, where `UrlKey(String)` is a lowercase host without `www.`, no scheme, a path with exactly one trailing slash unless the last segment has a file extension, percent-decoding normalised, and query params sorted with `utm_*`, `fbclid`, `gclid` and `_ga` dropped.

- [ ] **Step 1: Write the failing tests**
  - `init_then_open`: init a project in a tempdir, then open it. `porter.toml` and `project.db` exist, and a second `init` errors.
  - `put_get_item_upserts`: put the same id twice with different data. `items()` returns 1 row with the second data.
  - `rerun_resolves_missing_issues`:
    - run 1 opens issues A and B
    - B is marked `HandledManually`
    - run 2 reopens nothing
    - after `end_run`, A is `FixedByRerun` and B is still `HandledManually`
  - `issue_dedupes`: opening the same issue twice in one run gives 1 row.
  - `clean_only_owned_paths`: a project dir holding `notes.txt`, `porter.toml`, `raw/` and `media/`. After `clean()`, only `notes.txt` and `porter.toml` remain.
  - `config_validate_names_keys`: a config with no `destination.url` validates to a message containing `"destination.url"`.
  - `normalise_url` cases (Review Focus 1). These all give equal keys:
    - `http://www.Example.com/About`
    - `https://example.com/about/`
    - `https://example.com/about?utm_source=x`
    - `https://example.com/%61bout/`

    These stay distinct:
    - `https://example.com/?p=12`
    - `https://example.com/?p=13`
    - `https://example.com/file.pdf` (keeps no trailing slash)
- [ ] **Step 2:** Run `cargo test -p porter-engine --test store` and `cargo test -p porter-engine url`. Expected: FAIL.
- [ ] **Step 3:** Implement the code. Use one SQLite connection behind a `Mutex`, with WAL mode. Version `schema.sql` with `PRAGMA user_version`. Item tables hold `(id, kind, connector, source_id, url, data JSON, updated_at, deleted_at)`.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(engine): project, config, SQLite store, issues lifecycle, URL keys`.

### Task 5: Media store

**Files:**
- Create: `crates/porter-engine/src/store/media.rs`
- Test: `crates/porter-engine/tests/media_store.rs`

**Interfaces:**
- Produces:
  - `Store::put_media_bytes(&self, bytes: &[u8], url: &str, suggested_name: &str) -> Result<MediaHandle { id: ItemId, hash: String, path: PathBuf, is_new: bool }>`
    - stores the bytes at `media/<h0h1>/<h2h3>/<sha256>`
    - records `url` as a variant
    - the media item's id is derived from the hash (`SourceRef { connector: "media", source_id: hash }`)
  - `Store::add_media_variant(&self, id: ItemId, url: &str) -> Result<()>`
  - `Store::media_by_url(&self, url: &str) -> Result<Option<ItemId>>`, matched through `normalise_url`
  - `Store::media_path(&self, id: ItemId) -> Result<PathBuf>`

- [ ] **Step 1: Write the failing tests**
  - `same_bytes_dedupe`: the same bytes under two URLs (`/a-300x200.jpg` and `/a.jpg`) give one `ItemId`, `is_new` false the second time, and both URLs resolve.
  - `case_collision_kept_apart`: different bytes under `/Photo.jpg` and `/photo.jpg` give two ids, both readable on a case-insensitive filesystem. Paths are hashes, so this holds by construction; assert both files exist and differ.
  - `variant_lookup_normalises`: `http://www.x.com/a.jpg` finds media stored as `https://x.com/a.jpg`.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test media_store`. Expected: FAIL.
- [ ] **Step 3:** Implement. Write to a temp file in `media/`, then rename, so an interrupted write never leaves a partial file under a hash name.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(engine): content-addressed media store`.

### Task 6: Events, stage runner, adapter traits

**Files:**
- Create: `crates/porter-engine/src/{events.rs,stage.rs,http.rs}`
- Test: `crates/porter-engine/tests/stage.rs`

**Interfaces:**
- Produces:
  - `Event`, an enum serialised with `{"event": "<kebab>", ...}` tagging and a top-level `"v": 1`. Variants:
    - `StageStarted { stage, run: RunId }`
    - `Progress { stage, done: u64, total: Option<u64>, label: String }`
    - `ItemResult { stage, item: ItemId, kind: ItemKind, status: ItemStatus }`
    - `IssueOpened { issue: Issue }`
    - `StageFinished { stage, run, summary: Summary }`
  - `EventSink(tokio::sync::broadcast::Sender<Event>)`, with `emit(&self, e: Event)`
  - `Summary { done, partial, skipped, failed: u64, errors, warnings, infos: u64, interrupted: bool }`
  - `exit_code(s: &Summary) -> i32`, which implements the four codes in the constraints
  - `StageCtx { project: Arc<Project>, config: Arc<ProjectConfig>, events: EventSink, cancel: CancellationToken, run: RunId, stage: Stage }`. Its helpers:
    - `ctx.item_status(id, kind, status)` writes the status and emits `ItemResult`
    - `ctx.issue(kind, severity, item, message, details)` opens the issue and emits `IssueOpened`
    - `ctx.progress(done, total, label)`
  - `run_stage<F, Fut>(project, config, events, cancel, stage, f: F) -> Result<Summary>`, where `F: FnOnce(StageCtx) -> Fut`, `Fut: Future<Output = anyhow::Result<()>>`. It calls `begin_run`, then `f`, tallies the summary from statuses and issues written in this run, sets `interrupted` if `cancel.is_cancelled()`, then calls `end_run` and emits `StageFinished`. If `f` returns `Err`, the error propagates and the CLI exits 1.
  - Traits:
    ```rust
    #[async_trait] pub trait Connector: Send + Sync {
        fn name(&self) -> &str;
        fn capabilities(&self) -> Capabilities;
        async fn analyse(&self, ctx: &StageCtx) -> anyhow::Result<()>;
        async fn extract(&self, ctx: &StageCtx) -> anyhow::Result<()>;
    }
    pub trait Source: Send + Sync {
        fn connectors(&self, cfg: &ProjectConfig) -> anyhow::Result<Vec<Box<dyn Connector>>>;
        fn normalise(&self, ctx: &StageCtx) -> anyhow::Result<()>;
    }
    #[async_trait] pub trait Destination: Send + Sync {
        fn capabilities(&self) -> Capabilities;
        async fn schema(&self, mode: SchemaMode, ctx: &StageCtx) -> anyhow::Result<()>;
        async fn load(&self, ctx: &StageCtx, apply: bool) -> anyhow::Result<()>;
        async fn count(&self, ctx: &StageCtx, dest_type: &str) -> anyhow::Result<u64>;
        async fn fetch_loaded(&self, ctx: &StageCtx, id: ItemId) -> anyhow::Result<Option<serde_json::Value>>;
    }
    ```
  - `Capabilities { reads: BTreeSet<Feature>, writes: BTreeSet<Feature> }`, where `Feature` is `Layouts | RichText | Media | Forms | Submissions | Locales | Drafts | Menus | Redirects | Taxonomies | Seo`
  - `SchemaMode`, an enum: `Check | Generate | Prompt | Spec`
  - `registry::source(name: &str) -> Option<Box<dyn Source>>` and `registry::destination(name, cfg) -> Option<Box<dyn Destination>>`
  - `http::client(user_agent: &str) -> reqwest::Client`
  - `http::send_with_retry(req: RequestBuilder, max: u32) -> Result<Response>`: retries 429 and 5xx with exponential backoff (500 ms base, capped at 30 s, honouring `Retry-After`), up to `max` attempts. This is ported from `legacy/src/sources/wordpress/api_connector.rs`.

- [ ] **Step 1: Write the failing tests**
  - `exit_codes`: all-done with no errors gives 0; a failed item gives 2; an error issue gives 2; warnings only give 0; interrupted gives 3.
  - `stage_tallies_and_emits`: a stage fn marks 2 items `Done`, 1 `Failed`, and opens 1 warning. The summary is `done=2 failed=1 warnings=1`, and the receiver sees `StageStarted`, 3 `ItemResult`, 1 `IssueOpened`, and `StageFinished`, in that order.
  - `event_json_shape`: `serde_json::to_value(Event::Progress{..})` has `"event":"progress"` and `"v":1`.
  - `retry_honours_retry_after`: against a local `axum` or `wiremock` server that answers 429 with `Retry-After: 1` and then 200, `send_with_retry` succeeds after at least 1 s.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test stage`. Expected: FAIL.
- [ ] **Step 3:** Implement. (Use `wiremock` as a dev-dependency for all HTTP tests in this plan.)
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(engine): events, stage runner, adapter traits, retrying HTTP`.

### Task 7: CLI contract

**Files:**
- Create: `crates/porter-cli/src/main.rs` and `crates/porter-cli/src/commands/{init,status,issues,clean,stage,report,export}.rs`
- Test: `crates/porter-cli/tests/cli.rs` (uses `assert_cmd`)

**Interfaces:**
- Consumes: Tasks 4 and 6.
- Produces the commands:
  - `porter init <name> --adapter <src>`
  - `porter status`
  - `porter issues [--stage S] [--kind K] [--severity S] [--open] [--resolve <id> --as handled|wont-fix --note <text>]`
  - `porter clean`
  - `porter analyse|extract|normalise [--refresh]`
  - `porter schema check|generate|prompt|spec`
  - `porter load [--apply]`
  - `porter verify`
  - `porter run [--continue] [--apply]`
  - `porter report`
  - `porter export --json <file>`

  Global flags: `--project <dir>` (default `.`), `--json`, `--interactive`, and `-v` to raise the stderr log level.

  Stage commands resolve the adapters through `registry` and call `run_stage`. On SIGINT the CLI cancels the token, waits for the stage to return, and exits 3. `run` executes analyse → extract → normalise → schema check → load → verify, and stops after any stage whose exit code is non-zero unless `--continue` is set.

- [ ] **Step 1: Write the failing tests**
  - `json_stdout_is_pure_ndjson`: `porter --json status` in an initialised project prints stdout lines that all parse as JSON, the last one with `"event":"status"`.
  - `missing_config_exits_1_naming_key`: `porter analyse` with an empty `[source]` exits 1, and stderr contains `source.adapter`.
  - `no_prompt_without_tty`: `porter init x --adapter wordpress --interactive` with piped stdin completes without waiting (a 5 s timeout) and exits 0.
  - `clean_keeps_user_files`: the same as the store test, through the CLI.
  - `old_flags_rejected`: `porter --source umbraco` exits 2 (clap usage error) and stderr mentions `2.0`. Add a clap `after_help` note: "porter 2.0 replaced --source/--target; see `porter --help` and UPGRADING.md".
- [ ] **Step 2:** Run `cargo test -p porter-cli`. Expected: FAIL.
- [ ] **Step 3:** Implement. Human output (without `--json`) is a one-line-per-event renderer on stderr, plus a final summary table on stdout.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(cli): porter 2.0 command contract, NDJSON output, exit codes`.

### Task 8: HTML reports

**Files:**
- Create: `crates/porter-engine/src/report/{mod.rs,analysis.rs,extract.rs,verify.rs}`
- Test: inline tests

**Interfaces:**
- Produces:
  - `report::write_all(project: &Project) -> Result<Vec<PathBuf>>`, which writes `reports/{analysis,extract,verify}.html` for each stage that has a finished run
  - `report::render_analysis(store: &Store) -> Result<String>`; the extract and verify renderers follow the same pattern

  Each file is fully self-contained: inline CSS, no external requests, and it works offline as a single shareable file. Content:
  - **Analysis:** the platform and detected plugins/builders; content types with counts by status; the URL tree (collapsible `<details>`); media count and total bytes; redirects; forms; menus; languages; a **coverage** table (connector → what it contributed → access gaps); and issues grouped by kind.
  - **Extract:** per-connector fetch counts, media fetched/failed, and run history.
  - **Verify:** count checks per type, link and relation checks, the URL check table, and every failure listed.

  Every report header shows the run time, porter version and project name.

- [ ] **Step 1: Write the failing tests**
  - `analysis_report_self_contained`: render from a store seeded with 3 docs, 1 issue and 1 coverage gap. The HTML contains no `src="http` and no `href="http` (apart from content links in tables), contains the issue message, and contains `Coverage`.
  - `report_escapes_content`: a doc titled `<script>alert(1)</script>` appears escaped.
- [ ] **Step 2:** Run `cargo test -p porter-engine report`. Expected: FAIL.
- [ ] **Step 3:** Implement with `maud`. Any templating that compiles in and has no runtime assets is acceptable.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(engine): self-contained HTML reports`.

---

## Milestone B: WordPress and Umbraco sources

### Task 9: Public-site crawl connector

**Files:**
- Create: `crates/porter-engine/src/connectors/crawl.rs`
- Test: `crates/porter-engine/tests/crawl.rs` (with a wiremock site fixture)

**Interfaces:**
- Consumes: `http`, `url`, `StageCtx`, the media store.
- Produces:
  - `ConnectorConfig::Crawl { url: String, concurrency: usize = 4, delay_ms: u64 = 250, include: Vec<String>, exclude: Vec<String>, max_pages: Option<usize> }`
  - `CrawlConnector::new(cfg) -> Self`, which implements `Connector`
    - `analyse` seeds from `robots.txt` sitemaps and `/sitemap.xml` (sitemap indexes included), then follows same-host links. For every URL it stores a `Route` item with `http_status`, the redirect hops (each one becomes a `Redirect { origin: Crawl }`) and the `title`. It stores the page head (meta description, OG, canonical, hreflang, generator) in `raw/crawl/pages/<urlkey-hash>.json`. It writes `raw/crawl/detect.json`.
    - `extract` also saves the full HTML to `raw/crawl/html/<urlkey-hash>.html` and downloads every `img[src|srcset]`, `source[srcset]`, `a[href]` pointing at a media extension, and CSS `url()` in inline styles into the media store.
  - `detect_platform(html: &str, headers: &HeaderMap) -> Detected { platform: Option<String>, builders: Vec<String>, plugins: Vec<String> }`, with signatures for:
    - WordPress: `meta[name=generator]`, `/wp-content/`, `/wp-json/` link
    - Divi: `et_pb_` classes or `Divi` in the theme path
    - Elementor: `elementor-` classes
    - Gutenberg: `wp-block-` classes
    - WPBakery: `vc_row`
    - Yoast: a `<!-- This site is optimized with the Yoast` comment
    - Rank Math
    - WPML: `wpml-ls`
    - Gravity Forms: `gform_wrapper`
    - Ninja Forms: `nf-form-cont`
    - Smart Slider: `n2-section-smartslider`
    - Squarespace, Webflow (`data-wf-site`), Sanity image CDN, Gatsby (`___gatsby`)

- [ ] **Step 1: Write the failing tests**
  - `crawl_follows_sitemap_and_links`: the fixture site has a sitemap with 2 URLs, one page linking to a third, and one 301 → 200. After `analyse` there are 4 routes, 1 crawl redirect, and every route has a status.
  - `crawl_respects_exclude`: `exclude = ["/private/*"]` means no route under `/private/`.
  - `detect_wordpress_divi_yoast`: fixture HTML gives `platform == Some("wordpress")`, with builders containing `"divi"` and plugins containing `"yoast"`.
  - `extract_downloads_srcset_variants_once`: `img src=a-300x200.jpg srcset="a-300x200.jpg 300w, a.jpg 1200w"` with identical bytes gives 1 media item with 2 variants. With different bytes it gives 2 items.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test crawl`. Expected: FAIL.
- [ ] **Step 3:** Implement using `scraper` for HTML and `quick-xml` for sitemaps. Concurrency comes from a `Semaphore`, with `delay_ms` per worker between requests. Check `cancel` before every request.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(engine): public-site crawl connector with platform detection`.

### Task 10: WXR connector (streaming)

**Files:**
- Create: `crates/porter-engine/src/sources/wordpress/wxr.rs`
- Test: `crates/porter-engine/tests/wxr.rs`, using `tests/fixtures/wxr/{basic.xml,tricky.xml}`. `basic.xml` is a corrected copy of `test_data/wordpress_sample.xml`; `tricky.xml` has a multi-line item, a split `]]>`, an invalid UTF-8 byte and an attachment.

**Interfaces:**
- Produces:
  - `ConnectorConfig::Wxr { path: PathBuf }`
  - `WxrConnector`, implementing `Connector`
    - `analyse` counts items by `wp:post_type`, plus authors, terms and attachments.
    - `extract` writes one JSON file per item to `raw/wxr/items/<post_id>.json`, plus `authors.json`, `terms.json` and `site.json` (title, link, base_site_url). Attachments are fetched into the media store from `wp:attachment_url`.
  - `WxrItem { post_id: u64, post_type, status, title, link, slug, parent: u64, menu_order: i64, date_gmt, modified_gmt, author, content, excerpt, categories: Vec<(String /*domain*/, String /*nicename*/, String /*label*/)>, meta: Vec<(String,String)>, attachment_url: Option<String> }`
  - `parse_wxr<R: BufRead>(r: R, on_item: impl FnMut(Result<WxrItem, WxrItemError>)) -> Result<WxrHeader>`. This is a streaming `quick-xml` reader that never holds more than one item in memory. A malformed item yields `Err` for that item; parsing continues with the next one.

- [ ] **Step 1: Write the failing tests**
  - `wxr_streams_multiline_and_cdata`: `tricky.xml` gives the expected count, and the post whose content contains `]]>` round-trips that content exactly.
  - `wxr_bad_item_is_failed_not_fatal`: the item with an invalid byte gets `ItemStatus::Failed`, the other items are `Done`, and the stage exit code is 2.
  - `wxr_post_type_unwrapped`: `post_type == "post"` (not `<![CDATA[post]]>`).
  - `wxr_memory_bounded`: generate a 200k-item WXR on the fly into a temp file and parse it. This passes if it completes. `#[ignore]` by default; CI runs it in a nightly job.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test wxr`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(wordpress): streaming WXR connector`.

### Task 11: WordPress REST connector

**Files:**
- Create: `crates/porter-engine/src/sources/wordpress/rest.rs`
- Port from: `legacy/src/sources/wordpress/api_connector.rs` (pagination and auth) and `legacy/src/util/auth.rs` (env overrides)
- Test: `crates/porter-engine/tests/wp_rest.rs` (wiremock)

**Interfaces:**
- Produces:
  - `ConnectorConfig::WpRest { url: String /* site root */, auth: Option<String> /* "basic"|"bearer"|"apikey" */, keyring: Option<String> }`. Credentials come from the `PORTER_WP_*` env vars, or from the keyring entry.
  - `WpRestConnector`, implementing `Connector`
    - `analyse` reads `/wp-json` (`namespaces` and `routes`), and `/wp-json/wp/v2/types` and `/taxonomies`. It records counts from the `X-WP-Total` header per type.
    - `extract` pages every type with `context=edit` when authenticated (`view` otherwise), `per_page=100`, `status=any` when authenticated. It also fetches `/wp/v2/media`, `/menus` and `/menu-items` (WordPress 5.9+, auth required), `/users`, terms per taxonomy, `/settings` (auth), Yoast's `/yoast/v1/get_head?url=` when the `yoast/v1` namespace exists, and Redirection's `/redirection/v1/redirect` when that namespace exists. Each record is written to `raw/wp-rest/<route>/<id>.json`.
  - Every unauthenticated limitation opens an `access-gap` issue naming what's missing. Examples: "drafts and private posts not visible without credentials", "menus need credentials".

- [ ] **Step 1: Write the failing tests**
  - `rest_paginates_by_total_pages`: 3 pages via `X-WP-TotalPages` give 250 records.
  - `rest_unauthenticated_reports_gaps`: with no credentials, `access-gap` issues mention `drafts` and `menus`.
  - `rest_env_overrides_keyring`: with `PORTER_WP_BEARER_TOKEN` set, the request carries `Authorization: Bearer <token>`.
  - `rest_optional_namespaces`: when `/wp-json` lists `redirection/v1`, the redirects are fetched; when it doesn't, there is no request.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test wp_rest`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(wordpress): REST connector with access-gap reporting`.

### Task 12: WordPress DB-dump connector

**Files:**
- Create: `crates/porter-engine/src/sources/wordpress/db.rs` and `sources/wordpress/export.php` (embedded with `include_str!`)
- Reference: Landmark's `extract/export.php`, `extract/pull.sh` and `extract/submissions.sh`
- Test: `crates/porter-engine/tests/wp_db.rs`, gated by `PORTER_DOCKER_TESTS=1`, with a tiny synthetic dump in `tests/fixtures/wp-db/` (a fresh WordPress install with 3 posts, 1 page with a child, 1 ACF field group, 1 menu, the Redirection table with 2 rows and a Yoast meta row)

**Interfaces:**
- Produces:
  - `ConnectorConfig::WpDb { dump: PathBuf, wp_root: Option<PathBuf> /* core+themes+plugins */, table_prefix: String = "wp_", site_url: Option<String> }`
  - `WpDbConnector`, implementing `Connector`. It:
    1. starts `mariadb:11` as a `porter-<project>-db` container on a private Docker network
    2. imports the dump (gz or plain)
    3. if `wp_root` is present, runs `docker run --rm --network ... -v wp_root:/var/www/html -v <tmp>/export.php:/export.php wordpress:cli wp eval-file /export.php --path=/var/www/html`, with output to `raw/wp-db/export.json`
    4. stops and removes the container, even on error or cancel (a `Drop` guard)
  - `export.php` is **generic**. It discovers everything at runtime and hardcodes no post types, URLs or emails. It exports:
    - all post types (`get_post_types`) including private, draft and future posts
    - each post's permalink (`get_permalink`, or `get_sample_permalink` for non-public posts)
    - post meta
    - ACF values when ACF is active: `get_fields($id, false)` with field keys mapped to names via `acf_get_field`
    - ACF field groups (`acf_get_field_groups` and `acf_get_fields`)
    - taxonomies and terms
    - menus (`wp_get_nav_menus` and `wp_get_nav_menu_items`) with locations
    - options: `blogname`, `page_on_front`, `show_on_front`, `permalink_structure`, `wpseo_titles`
    - attachments with `_wp_attachment_metadata` (sizes and `original_image`) and `_wp_attached_file`
    - Gravity Forms (`GFAPI::get_forms`) and Ninja Forms (`Ninja_Forms()->form()->get_forms()`) definitions when active
    - WPML/Polylang translation groups when active
    - the active plugins list

    Submissions and Redirection rows are read with direct SQL in Rust, from `gf_entry`/`gf_entry_meta`, Ninja Forms' `nf3_*`/postmeta, and `redirection_items`. They go to `raw/wp-db/{submissions,redirection}.json`.
  - Without `wp_root`: direct SQL fallback for posts, postmeta, terms, options and attachments, plus `access-gap` issues for "ACF field names unavailable (keys only)", "permalinks reconstructed from permalink_structure", and "form definitions unavailable".
  - Without Docker on PATH: the stage errors (exit 1) with "Docker is required for the wp-db connector".

- [ ] **Step 1: Write the failing tests** (Docker-gated)
  - `wpdb_exports_everything_with_wp_root`: `export.json` has 3 posts and 2 pages, the child page has `parent` set, the ACF value comes under its field *name*, there is 1 menu with its items, and the private post has a permalink.
  - `wpdb_fallback_reports_gaps`: without `wp_root`, the 3 `access-gap` issues are present.
  - `wpdb_container_removed_on_error`: a corrupt dump makes the stage error, and `docker ps -a` shows no `porter-*-db` container.
  - `export_php_is_generic` (not gated): `export.php` contains no `http` URL and no `@` email literal.
- [ ] **Step 2:** Run `PORTER_DOCKER_TESTS=1 cargo test -p porter-engine --test wp_db`. Expected: FAIL.
- [ ] **Step 3:** Implement. Shell out to the `docker` CLI; no Docker SDK crate.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(wordpress): DB-dump connector with generic in-WordPress exporter`.

### Task 13: WordPress analyse: merge and coverage

**Files:**
- Create: `crates/porter-engine/src/sources/wordpress/{mod.rs,analyse.rs}`
- Test: `crates/porter-engine/tests/wp_analyse.rs`

**Interfaces:**
- Consumes: the connectors from Tasks 9-12.
- Produces:
  - `WordPressSource`, implementing `Source`. `connectors()` builds the connectors from config in this priority order: wp-db > wp-rest > wxr > crawl.
  - `analyse::merge(store: &Store) -> Result<MergeReport { only_in_source: Vec<String /*url*/>, only_live: Vec<String>, matched: usize }>`
    - Content is matched to routes by `UrlKey`, and by post ID where both sides have one (the crawl reads `postid-N` and `page-id-N` body classes).
    - It writes `raw/analysis/coverage.json`: per connector, what it contributed (types, counts) and its gaps.
  - Opens `converter-missing` (warning) for each detected builder or plugin with no converter in this phase: Elementor, WPBakery and Smart Slider are detected but not converted, so content using them becomes raw blocks.
  - Opens `access-gap` (info) for each kind of data the configured connectors can't reach.

- [ ] **Step 1: Write the failing tests**
  - `merge_reports_unreachable_and_orphans`: in the store, 3 published pages from wxr and 2 crawl routes, where one route has no content and one page has no route. `only_in_source.len() == 1` and `only_live.len() == 1`.
  - `merge_by_body_class_id`: a crawl route whose body class is `page-id-7` matches WXR post 7 even when its URL differs (an old slug).
  - `elementor_detected_raises_converter_missing`
- [ ] **Step 2:** Run `cargo test -p porter-engine --test wp_analyse`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(wordpress): analyse merges connectors and reports coverage`.

### Task 14: WordPress content converters

**Files:**
- Create in `crates/porter-engine/src/sources/wordpress/`: `shortcodes.rs`, `texturize.rs`, `autop.rs`, `html.rs`, `divi.rs`, `gutenberg.rs`
- Port from Landmark:
  - `lib/shortcodes.ts` (the parser rules, but with a configurable `known` tag pattern)
  - `lib/texturize.ts` and its test cases
  - the `wpHtml` and `normaliseHref` rules from `lib/html-to-lexical.ts`
  - `lib/divi-to-blocks.ts` (module mapping, global-layout expansion, `%91/%93/%22` decoding, hidden-module skip)
  - the `__tests__/divi.test.ts` cases, rewritten with synthetic content
- Test: inline tests, plus `tests/fixtures/wp-content/*.txt`

**Interfaces:**
- Produces:
  - `ScNode { tag: String, attrs: IndexMap<String,String>, children: Vec<ScNode>, text: String }`
  - `parse_shortcodes(src: &str, known: &Regex) -> Vec<ScNode>`
  - `texturize(text: &str) -> String`
  - `autop(html: &str) -> String`, a port of WordPress's `wpautop`
  - `HtmlCtx<'a> { store: &'a Store, site_hosts: Vec<String>, issues: &'a mut Vec<PendingIssue> }`, where `PendingIssue { kind: IssueKind, severity: Severity, message: String, details: Value }` collects issues for the caller to attach to the item
  - `html_to_richtext(html: &str, ctx: &mut HtmlCtx) -> RichText`
    - Internal links (hosts in `site_hosts`, or relative) resolve to `LinkTarget::Doc` through `Store` routes when a route exists; otherwise `LinkTarget::Url` plus an `unresolved-link` warning.
    - `img` resolves to `RtBlock::Media` through `media_by_url`; otherwise `missing-media`.
    - Tables, iframes, scripts, forms and unknown elements become `RtBlock::Raw`.
  - `divi_to_layout(content: &str, globals: &HashMap<u64, String>, ctx: &mut HtmlCtx) -> Vec<Block>`
    - Divi sections, rows and columns become `Section`/`Row`/`Column { width }` (from `type="1_2"` and similar).
    - Modules map as follows:

      | Divi module | Neutral block |
      |---|---|
      | `et_pb_text` | `RichText` |
      | `et_pb_image` | `Image` |
      | `et_pb_gallery` | `Gallery` |
      | `et_pb_button` | `Button` |
      | `et_pb_video` | `Video` |
      | `et_pb_code` | `Raw` |
      | `et_pb_tabs` | `Tabs` |
      | `et_pb_accordion` / `et_pb_toggle` | `Accordion` |
      | `et_pb_slider` / `et_pb_fullwidth_slider` | `Slider` |
      | `et_pb_map` | `Map` |
      | `et_pb_divider` | `Divider` |
      | `gravityform` / `ninja_form` | `FormRef` |
      | anything else | `Raw` plus an `unsupported-block` issue |

    - Every block gets `original` filled in.
  - `gutenberg_to_layout(content: &str, ctx: &mut HtmlCtx) -> Vec<Block>` parses `<!-- wp:name {json} -->` comments:
    - `core/paragraph`, `heading`, `list`, `quote` and `table` become `RichText`
    - `core/image` becomes `Image`
    - `core/gallery` becomes `Gallery`
    - `core/columns`/`column` become `Row`/`Column`
    - `core/group` becomes `Section`
    - `core/buttons` becomes `Button`
    - `core/embed` becomes `Embed`
    - `core/html` becomes `Raw`
    - `core/separator` becomes `Divider`
    - `core/spacer` becomes `Spacer`
    - `core/block` (a reusable block) is expanded from the store
    - anything else becomes `Raw` plus `unsupported-block`
  - `classic_to_richtext(content: &str, ctx) -> RichText` = `html_to_richtext(&autop(&texturize(content)), ctx)`

- [ ] **Step 1: Write the failing tests**
  - `shortcodes_nested_and_unknown_text`: `[et_pb_section][et_pb_text]Hi [key][/et_pb_text][/et_pb_section]` gives a section > text tree, and `[key]` stays as text.
  - `shortcode_attr_quoting`: the double-, single- and un-quoted forms all parse.
  - `texturize_cases`: every case from Landmark's `lib/__tests__/texturize.test.ts`, as inputs and expected outputs.
  - `autop_paragraphs_and_br`: `"a\n\nb\nc"` gives `"<p>a</p>\n<p>b<br />\nc</p>\n"`.
  - `html_links_resolve_or_flag`: a link to a known route becomes `Doc`, an unknown internal link gives `unresolved-link`, and an external link becomes `Url` with no issue.
  - `divi_columns_and_widths`: `type="1_3"` and `"2_3"` give `Column { width: Some(0.333..) }` and `Some(0.666..)`.
  - `divi_global_module_expanded`: `global_module="12"` is replaced by layout 12's content.
  - `divi_hidden_module_skipped`: `disabled_on="on|on|on"` gives no block.
  - `divi_unknown_module_raw_with_issue`
  - `gutenberg_columns_image`: a two-column layout with an image in one column gives `Row` > 2 × `Column`, with an `Image` whose media resolves.
  - `gutenberg_unknown_block_raw`
- [ ] **Step 2:** Run `cargo test -p porter-engine wordpress::`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(wordpress): shortcode, texturize, autop, HTML, Divi and Gutenberg converters`.

### Task 15: Mapping config, transforms, path patterns

**Files:**
- Create: `crates/porter-engine/src/mapping/{mod.rs,transforms.rs,paths.rs}`
- Test: inline tests

**Interfaces:**
- Produces:
  - `MappingConfig`, with these parts:
    - `types: IndexMap<String /*source type*/, TypeMap>`
    - `TypeMap { to: Option<String> /* None = drop */, drop_reason: Option<String>, path: Option<String>, key_field: String = "legacyId", key_type: KeyType = Text, status: IndexMap<String,String> /* e.g. private→draft */, fields: IndexMap<String /*neutral path*/, FieldMap> }`
    - `FieldMap { to: String /* dest path */, transform: Vec<Transform> }`
    - `blocks: Vec<BlockMap { r#match: BlockMatch { kind: Option<String>, module: Option<String> }, to: String /* dest block slug */, fields: IndexMap<String,String> }>`
    - `locales: LocaleMode`, with variants `Skip` and `Map { default: String, others: Vec<String> }`
    - `forms: FormsMap { collection: Option<String>, retire: Vec<String> }`
    - `media: MediaMap { collection: String = "media", folders: bool = true }`
    - `redirects: RedirectsMap { collection: String = "redirects" }`
  - `KeyType`, an enum: `Text | Number`
  - `Transform`, a tagged enum (TOML inline tables): `Trim | Lower | Upper | Slugify | DateParse { format } | DateFormat { format } | Split { sep } | Join { sep } | Number | UrlRewrite { from, to } | Regex { pattern, replace } | Default { value } | Lookup { table: IndexMap<String,String> }`
  - `apply(value: &FieldValue, transforms: &[Transform]) -> Result<FieldValue, String>`
  - `paths::render(pattern: &str, doc: &Document, parent_path: Option<&str>) -> Result<String, String>`, which substitutes `{slug}`, `{id}`, `{type}`, `{yyyy}`, `{mm}` and `{parent.path}`, and normalises double slashes
  - `MappingConfig::seed_from(store: &Store) -> MappingConfig`: one `TypeMap` per source type, mapped to the same name with a default path and every neutral field mapped to itself. `porter init` and `porter analyse` write this into `porter.toml` when `[mapping]` is empty, as a commented block the user edits.

- [ ] **Step 1: Write the failing tests**
  - `transforms_chain`: `[Trim, Lower, Slugify]` turns `"  Hello World "` into `"hello-world"`.
  - `lookup_missing_errors`: a lookup with no entry gives `Err` naming the value.
  - `nested_path_pattern`: `"{parent.path}/{slug}/"` with parent `/rooms/` and slug `suite` gives `/rooms/suite/`.
  - `mapping_toml_roundtrip`: a TOML sample with a type drop, a field transform and a block map parses and round-trips.
- [ ] **Step 2:** Run `cargo test -p porter-engine mapping`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(engine): mapping config, transforms, URL path patterns`.

### Task 16: WordPress normalise

**Files:**
- Create in `crates/porter-engine/src/sources/wordpress/`: `normalise.rs`, `acf.rs`, `seo.rs`, `forms.rs`, `redirects.rs`
- Port from Landmark:
  - `lib/acf.ts`: walk the field-group definition, not the destination config, to type values
  - the Yoast substitution in `import-content.ts`
  - the generic parts of `lib/forms.ts` (Ninja and Gravity mapping, conditions, notifications), with no hardcoded emails
  - the chain collapsing in `import-redirects.ts`
  - `lib/media-map.ts`'s URL-variant rules (`-WxH`, `-scaled`, case, other hosts), feeding `add_media_variant`
- Test: `crates/porter-engine/tests/wp_normalise.rs`, using a store seeded from the `tests/fixtures/wxr/basic.xml` extract plus a synthetic `export.json`

**Interfaces:**
- Produces `WordPressSource::normalise`, which reads `raw/` from all connectors, preferring wp-db > wp-rest > wxr > crawl per field, and writes:
  - a `Document` per post or page. It sets:
    - `type` = `post_type`
    - `status`: publish → `Published`, draft or pending → `Draft`, private → `Private`, future → `Scheduled`
    - `parent`
    - `slug`
    - `path`, from the permalink
    - `fields["content"]`: `Layout` when it is Divi or Gutenberg (detected by `[et_pb_` or `<!-- wp:`), otherwise `RichText` via `classic_to_richtext`
    - `fields["excerpt"]`
    - `fields["featured_image"]` (`Media`)
    - ACF fields under `fields["acf.<name>"]`, typed: repeater → `List(Object)`, flexible content → `List(Object)` with a `"layout"` key, gallery → `List(Media)`, relationship/post_object → `Doc`/`List(Doc)`, image/file → `Media`, wysiwyg → `RichText`, true_false → `Bool`, date_picker → `Date`
    - `seo`: Yoast (`_yoast_wpseo_*`) or Rank Math (`rank_math_*`), with `%%title%%`, `%%sep%%`, `%%sitename%%`, `%%excerpt%%` and `%%page%%` substituted, and unknown variables removed with an info issue
    - `locale` and `translations` from WPML/Polylang groups
  - `Media`: alt, caption, title, and `folder` from the `_wp_attached_file` directory. Variants are added for every size in the attachment metadata and for `original_image`.
  - `Taxonomy` and `Term` items, and term assignments as `fields["terms.<taxonomy>"] = List(Doc)`
  - `Menu` items with `MenuTarget::Doc` when the menu item's object resolves, otherwise `Url`
  - `Form` and `Submission` items
  - `Settings`, with `front_page` taken from `page_on_front`
  - `Redirect` items from Redirection rows (`origin: Plugin`), crawl hops (`Crawl`) and `[[redirects]]` in config (`Manual`), deduped by `from` key. Chains are collapsed to the final target (`redirect-chain` info issue) and loops dropped (`redirect-loop` warning). Rows with a query string or regex that can't be represented get `Skipped { reason }`.
  - `Route` items, linking each old URL to its document, media or redirect

  Every item gets a `Normalise` status, and issues come from the converters.

- [ ] **Step 1: Write the failing tests**
  - `post_statuses_mapped`: publish, draft, private and future map to the four `DocStatus` values.
  - `divi_page_becomes_layout` and `classic_post_becomes_richtext`
  - `acf_repeater_typed`: an ACF repeater of `{image, caption}` gives `List(Object{image: Media, caption: Text})`.
  - `yoast_vars_substituted`: `"%%title%% %%sep%% %%sitename%%"` gives `"About – Site"` (the separator comes from `wpseo_titles`).
  - `redirect_chain_collapsed`: a → b → c gives a → c, with a `redirect-chain` issue.
  - `redirect_loop_dropped`: a → b → a gives no redirects and a `redirect-loop` warning.
  - `parent_cycle_is_issue` (Review Focus 4): pages 1 and 2 that are each other's parent both get `parent-missing` issues, and `parent` is cleared on both.
  - `media_variants_registered`: an attachment with sizes `300x200` and `-scaled` makes all variant URLs resolve to one media item.
  - `every_item_has_status`: after normalise, every raw item has a `Normalise` status.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test wp_normalise`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(wordpress): normalise documents, ACF, SEO, menus, forms, redirects, routes`.

### Task 17: Umbraco source

**Files:**
- Create: `crates/porter-engine/src/sources/umbraco/mod.rs`
- Port from: `legacy/src/sources/umbraco.rs` (JSON reading, `_variants` locale handling, coordinate helpers)
- Test: `crates/porter-engine/tests/umbraco.rs` with a synthetic `tests/fixtures/umbraco/sample.json`. Also port `legacy/tests/umbraco_to_payload_test.rs` as `umbraco_local_fixture`, which skips when `./test-data/hotels.json` is absent, as it does today.

**Interfaces:**
- Produces:
  - `ConnectorConfig::UmbracoJson { paths: Vec<PathBuf> }`
  - `UmbracoSource`, implementing `Source`:
    - its connector copies the files into `raw/umbraco/`
    - analyse counts content types (from `contentTypeAlias`)
    - normalise makes one `Document` per node:
      - `type` = `contentTypeAlias`
      - each `_variants` entry becomes a translation `Document` linked through `translations`
      - property values are typed (RTE HTML goes to `RichText` via the WordPress `html_to_richtext`, which is moved to `crate::html` so both sources share it)
      - media pickers become `Media` (the media is fetched from URLs at extract)
      - the URL becomes `path`

- [ ] **Step 1: Write the failing tests**
  - `umbraco_variants_become_translations`: 1 node with en/de variants gives 2 documents that link to each other.
  - `umbraco_rte_is_richtext`
  - `umbraco_local_fixture` (skips when absent)
- [ ] **Step 2:** Run `cargo test -p porter-engine --test umbraco`. Expected: FAIL.
- [ ] **Step 3:** Implement. Move `html.rs` to `crates/porter-engine/src/html.rs` and re-export it from the WordPress module.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(umbraco): Umbraco JSON source on the neutral model`.

---

## Milestone C: Payload destination, verify, release

### Task 18: Payload test app and REST client

**Files:**
- Create:
  - `tests/payload-app/`: a minimal Payload 3 app using `@payloadcms/db-sqlite`, with collections `pages` (nested-docs, drafts, `legacyId` text, a `layout` blocks field with `richText`, `image` and `raw` blocks, and an SEO group), `posts`, `media` (upload, folders enabled), `users` (with API keys), plus the form-builder and redirects plugins. It has a `pnpm dev` script on port 3999 and a `seed-user` script that creates an API-key user and prints the key.
  - `crates/porter-engine/src/destinations/payload/{mod.rs,client.rs}`
- Test: `crates/porter-engine/tests/payload_client.rs` (wiremock), plus `tests/payload_live.rs` gated by `PORTER_PAYLOAD_TEST_URL` and `PORTER_PAYLOAD_API_KEY`
- CI: a `payload-golden` job that installs pnpm, runs the app, seeds the user, and runs the gated tests

**Interfaces:**
- Produces:
  - `PayloadClient::new(base: &str, api_key: &str, auth_collection: &str) -> Self`. It sends the header `Authorization: <auth_collection> API-Key <key>`.
  - `find_by(&self, collection, field, value: &Value) -> Result<Option<Value>>`: `GET /api/<c>?where[<field>][equals]=<v>&limit=1&depth=0&draft=true`
  - `create(&self, collection, data: &Value, draft: bool) -> Result<Value>`
  - `update(&self, collection, id: &Value, data: &Value, draft: bool) -> Result<Value>`
  - `upsert(&self, collection, key_field, key: &Value, data: &Value, draft: bool) -> Result<(Value /*doc*/, bool /*created*/)>`
  - `upload(&self, collection, file: &Path, filename: &str, mime: &str, data: &Value) -> Result<Value>`: multipart with a `file` part and a `_payload` JSON part
  - `update_global(&self, slug, data) -> Result<Value>`
  - `count(&self, collection) -> Result<u64>`: `GET /api/<c>?limit=0`, reading `totalDocs`
  - All calls go through `http::send_with_retry(.., 5)`.
  - Pitfalls from Landmark, each handled in `create`/`update`:
    - **Drafts:** when `draft == false`, send `_status: "published"` and no `?draft=true`. When true, send `?draft=true` *and* `_status: "draft"`, so the main row isn't left stale.
    - **Empty arrays:** remove `null` array fields from the payload before sending.
    - **Link URLs:** percent-encode spaces in link URLs (handled in Task 19's Lexical writer).

- [ ] **Step 1: Write the failing tests**
  - `auth_header_format`
  - `upsert_creates_then_updates`: wiremock returns `docs: []`, so the client POSTs; it then returns a doc, so the client PATCHes the id.
  - `draft_flag_and_status`: `draft=false` sends `_status: "published"` with no draft query.
  - `nulls_stripped`
  - `live_upload_and_upsert` (gated): upload a PNG, upsert a page twice, and `count("pages") == 1`.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test payload_client`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS. Run the gated test locally against `tests/payload-app`.
- [ ] **Step 5:** Commit: `feat(payload): test app and REST client with upsert and upload`.

### Task 19: Payload Lexical and blocks writers

**Files:**
- Create: `crates/porter-engine/src/destinations/payload/{lexical.rs,blocks.rs}`
- Reference: Landmark's `lib/html-to-lexical.ts` for the node shapes
- Test: inline tests, plus golden JSON in `tests/fixtures/lexical/*.json`

**Interfaces:**
- Produces:
  - `to_lexical(rt: &RichText, ids: &IdMap) -> (serde_json::Value, Vec<PendingIssue>)`, where `IdMap` is a lookup trait over `ItemId → (collection, dest id)`, filled at load. The node shapes:
    - root: `{type:"root",format:"",indent:0,version:1,direction:"ltr",children}`
    - paragraph: `{type:"paragraph",format:<align>,indent:0,version:1,direction:"ltr",textFormat:0,textStyle:"",children}`
    - heading: `{type:"heading",tag:"h<n>",...}`
    - list: `{type:"list",listType:"bullet"|"number",tag:"ul"|"ol",start}`, with `listitem` children (`value` numbered from 1) and nested lists inside list items
    - quote, horizontalrule, linebreak
    - text: `{type:"text",text,format:<bitmask>,detail:0,mode:"normal",style:"",version:1}`, with the bitmask bold=1, italic=2, strike=4, underline=8, code=16, sub=32, sup=64
    - link: `{type:"link",fields:{linkType:"internal",doc:{relationTo,value},newTab}}` for a `Doc` target, or `{linkType:"custom",url}` for a `Url` target, with spaces encoded as `%20`
    - upload: `{type:"upload",relationTo:"media",value:<id>,fields:{}}`
    - `Raw`, `Embed`, `Table` and `Code`: a `block` node `{type:"block",fields:{blockType:<mapping raw block slug>,html}}` when the mapping configures a raw block; otherwise the HTML is dropped to text, with an `unsupported-block` warning
    - an unresolved `Doc`/`Media` gives `unresolved-relation`, and the link degrades to a custom URL using the old URL
  - `to_blocks(layout: &[Block], mapping: &[BlockMap], ids: &IdMap) -> (Vec<Value>, Vec<PendingIssue>)`:
    - containers are flattened: Section, Row and Column are unwrapped unless a `BlockMap` matches them
    - each content block goes to the first matching `BlockMap` as `{blockType: to, ...mapped props}`
    - a block with no match gives `unsupported-block` (error) and is emitted as the configured raw block when one exists

- [ ] **Step 1: Write the failing tests**
  - `lexical_golden`: a fixture `RichText` with every node type converts to exactly `tests/fixtures/lexical/all-nodes.json`.
  - `lexical_marks_bitmask`: bold+italic gives `format: 3`.
  - `lexical_internal_link_resolves` and `lexical_unresolved_link_degrades_with_issue`
  - `blocks_flatten_containers_and_map`: a section > row > column > image with a map `{kind=image → "mediaBlock", fields {media → "media"}}` gives `[{"blockType":"mediaBlock","media":<id>}]`.
  - `blocks_unmatched_is_error_issue`
- [ ] **Step 2:** Run `cargo test -p porter-engine payload::lexical payload::blocks`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(payload): Lexical and blocks writers from the neutral model`.

### Task 20: Payload schema check, generate, prompt, spec

**Files:**
- Move and upgrade: `legacy/src/parser/` → `crates/porter-engine/src/destinations/payload/ts/`, upgraded to the current swc crates (`swc_ecma_parser`, `swc_ecma_ast`, `swc_common` at the latest mutually compatible versions)
- Create: `destinations/payload/{schema_check.rs,schema_generate.rs,schema_prompt.rs}`
- Test: `crates/porter-engine/tests/payload_schema.rs`, run against `tests/payload-app/src/collections/*.ts` and the existing `test-data/*.ts`

**Interfaces:**
- Produces:
  - `ts::read_payload_config(dir: &Path) -> Result<DestSchema>`, where `DestSchema { collections: IndexMap<String, CollectionSchema { fields: Vec<DestField>, upload: bool, drafts: bool }>, globals: IndexMap<String, CollectionSchema>, blocks: IndexMap<String, Vec<DestField>> }` and `DestField { name, kind: String, required: bool, many: bool, relation_to: Vec<String>, fields: Vec<DestField> /* group/array/blocks */ }`. It follows imports and path aliases, which the ported parser already does.
  - `schema_check::check(model_types: &TypeSummary, mapping: &MappingConfig, dest: &DestSchema) -> Vec<PendingIssue>` raises `schema-mismatch` (error) for:
    - a mapped destination collection that is missing
    - a mapped destination field path that is missing, or of an incompatible kind (the table: Text → text/textarea/email/select/code; RichText → richText; Layout → blocks; Media → upload; Doc → relationship; List → array/hasMany; Date → date; Bool → checkbox; Number → number)
    - a missing upsert `key_field`, or one whose kind doesn't match `key_type`
    - a required destination field with no mapped source
    - a `BlockMap.to` slug that isn't defined

    It raises `unmapped-field` (warning) for a neutral field with no mapping and no `drop`.
  - `TypeSummary`: per source type, each field path with its observed `FieldValue` variants and fill rate. It is computed from the store by `TypeSummary::from_store(&Store)`.
  - `schema_generate::generate(summary, mapping, out_dir: &Path) -> Result<Vec<PathBuf>>` writes one `<Collection>.ts` per mapped type, including `legacyId` (indexed, hidden, `admin.readOnly`), plus `blocks/<Slug>.ts` per `BlockMap`. The output is valid TypeScript that `read_payload_config` parses back.
  - `schema_prompt::prompt(summary, mapping, dest_dir) -> String` writes `reports/schema-prompt.md`: the goal, the required collections and fields as a table with sample values (3 real samples per field, truncated to 80 chars), the block list, the key-field requirement, and the closing instruction "verify by running `porter schema check --project <dir>` until it exits 0".
  - `schema_prompt::spec(...)` writes `reports/schema-spec.{md,html}`: the same content, for a human.
  - The Payload `Destination::schema` dispatches the four `SchemaMode`s. `check` reads `destination.config_dir`.

- [ ] **Step 1: Write the failing tests**
  - `reads_test_app_schema`: `pages` has `legacyId`, a `layout` blocks field with `richText`, `image` and `raw`, and `drafts == true`.
  - `check_flags_missing_field_and_kind`: mapping a `fields["acf.hero"]` (`RichText`) to `pages.hero` where `hero` is `text` gives a `schema-mismatch` that names both kinds.
  - `check_flags_missing_key_field`
  - `generate_roundtrips`: generate into a tempdir, then `read_payload_config` gives the same field names and kinds.
  - `prompt_contains_check_command_and_samples`
- [ ] **Step 2:** Run `cargo test -p porter-engine --test payload_schema`. Expected: FAIL.
- [ ] **Step 3:** Implement. The swc upgrade is part of this step. The old parser's tests (`legacy/src/parser/tests.rs`) move with it and must pass.
- [ ] **Step 4:** Run the tests. Expected: PASS, including the moved parser tests.
- [ ] **Step 5:** Commit: `feat(payload): schema check, generate, agent prompt and human spec`.

### Task 21: Payload load

**Files:**
- Create: `crates/porter-engine/src/destinations/payload/load.rs`
- Test: `crates/porter-engine/tests/payload_load.rs` (wiremock for ordering and failures), plus a gated live test

**Interfaces:**
- Consumes: Tasks 15, 18, 19 and 20.
- Produces:
  - `PayloadDestination::load(ctx, apply)` (the struct implementing `Destination`, defined in `destinations/payload/mod.rs` in Task 18). The order:
    1. **Media folders**, when `mapping.media.folders` is set and the media collection has folders. Upserted into `payload-folders` by path.
    2. **Media.** Upload to `mapping.media.collection`, keyed by `legacyId` = the media `ItemId` string, with alt/caption/title and the folder.
    3. **Taxonomies and terms.**
    4. **Documents, pass 1.** Create or upsert each with scalar fields only (references and rich text omitted), parents first. Sort by path depth, then by a topological order on `parent`. Any remaining cycle was already cleared in normalise.
    5. **Documents, pass 2.** Update each with references, rich text (`to_lexical`), layouts (`to_blocks`) and `parent`, now that the `IdMap` is complete.
    6. **Menus**, into the global or collection named in the mapping.
    7. **Forms**, then **submissions** (when `forms.collection` is set).
    8. **Redirects**, into `mapping.redirects.collection`, with targets rendered through `paths::render`.
    9. **Settings**, such as the front page, when mapped.
  - The `IdMap` is persisted in a store table `dest_ids(item_id, collection, dest_id)`, so re-runs and pass 2 after an interruption both work.
  - With `apply == false`, nothing is sent. Each item gets `Skipped { reason: "dry run" }`, the would-be payloads go to `raw/load-preview/<collection>/<item>.json`, and every mapping and schema problem is still raised.
  - Per-item failures (after retries) become `Failed { error }` plus a `write-failed` (error) issue, and the load continues. `cancel` is checked between items.
  - Items mapped with `to: None` get `Skipped { reason: drop_reason }`.
  - `count()` and `fetch_loaded()` for verify.

- [ ] **Step 1: Write the failing tests**
  - `children_after_parents`: with wiremock recording request order, page /a/b/ is created after /a/, and pass-2 PATCHes come after all pass-1 POSTs.
  - `load_retries_then_fails_item`: a 503 for one page 5 times gives `Failed`, a `write-failed` issue, the other pages `Done`, and exit code 2.
  - `load_resumes_without_duplicates` (Review Focus 2): cancel after 3 of 6 documents; exit code 3; re-run: `find_by` returns the 3 existing docs, so they're updated (PATCH) and the other 3 created (POST). There are 6 `dest_ids` rows in total, and no POST for an already-loaded item.
  - `dry_run_sends_nothing`: zero non-GET requests, and preview files exist.
  - `dropped_type_skipped_with_reason`
  - `live_load_roundtrip` (gated): a fixture project of 5 docs, 2 media, 1 menu and 2 redirects loads into the test app. Counts match, and a second run creates nothing new.
- [ ] **Step 2:** Run `cargo test -p porter-engine --test payload_load`. Expected: FAIL.
- [ ] **Step 3:** Implement. Media uploads run with concurrency 4, falling back to 1 for the remainder after any filename-collision error (the Landmark lesson). Documents load sequentially.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(payload): two-pass, resumable, idempotent load`.

### Task 22: Verify

**Files:**
- Create: `crates/porter-engine/src/verify.rs`
- Test: `crates/porter-engine/tests/verify.rs`

**Interfaces:**
- Consumes: the `Destination` trait (`count`, `fetch_loaded`), `dest_ids`, `paths::render`, `http`.
- Produces `verify::run(ctx: &StageCtx, dest: &dyn Destination) -> Result<()>`. It checks:
  - **Counts:** for each mapped type, the number of `Done` items equals `count(dest_type)`. A mismatch opens a `schema-mismatch` error, with expected and actual counts in the details.
  - **Structure:** for each loaded document, through `fetch_loaded`:
    - its parent matches `dest_ids[parent]`, otherwise `parent-missing`
    - every `Doc` reference and internal link in the loaded Lexical or blocks points at `dest_ids[target]`, otherwise `unresolved-relation`
    - every upload node and media field points at an existing media doc, otherwise `missing-media`
  - **URLs:** when `verify.site_url` is set, every old route (`Route` items with an old status of 200) is requested at `site_url + path`, following at most 1 redirect. It passes on 200, or on a single 301/302/307/308 hop to a 200. A failure opens `unresolved-link` (error) with the chain. This runs at concurrency 8.
    - Without `site_url`: an in-model check (each route maps to a loaded doc via its path pattern, or to a redirect) plus one `access-gap` info issue, "live URL check skipped: set verify.site_url".
  - Item statuses for `Verify`: `Done`, or `Failed` with the first failing check.

- [ ] **Step 1: Write the failing tests** (wiremock for both the destination and the site)
  - `verify_count_mismatch`
  - `verify_wrong_parent_detected`: the loaded doc's parent id differs from `dest_ids`.
  - `verify_url_one_hop_ok_two_hops_fail`
  - `verify_without_site_url_is_model_check_plus_gap`
- [ ] **Step 2:** Run `cargo test -p porter-engine --test verify`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(engine): verify counts, structure, and old URLs`.

### Task 23: Landmark acceptance

**Files:**
- Create: `acceptance/landmark/README.md`, plus `acceptance/landmark/porter.toml.example`: the mapping onto Landmark's app collections and blocks, written as config only, with no client content beyond collection and field names.
- Create: `acceptance/run.sh`
- Modify: `.gitignore`, to add `/acceptance/*/project/` and `/acceptance/*/snapshot/`

**Interfaces:**
- `acceptance/run.sh landmark`:
  - expects `LANDMARK_DIR` (default `~/Desktop/work/clients/landmark`)
  - inits a project in `acceptance/landmark/project/` from the example config, with connectors:
    - `wp-db`, with the dump at `$LANDMARK_DIR/wp-source/prodlandmark.sql.gz` and `wp_root` at `$LANDMARK_DIR/wp-source/wp`
    - `crawl`, from the recorded snapshot
  - points the destination at Landmark's app, running locally on a **fresh** database the user creates (the README gives the steps; porter never touches Landmark's real database)
  - runs `porter run --apply --json`, then prints the acceptance table

**Steps:**
- [ ] **Step 1: Capture the snapshot.** Run `porter extract` with only the `crawl` connector against the live old site, then copy `project/raw/crawl` and `project/media` to `acceptance/landmark/snapshot/`. This happens once, before Landmark's launch replaces the site. The run script uses the snapshot through `ConnectorConfig::Crawl { url: "file://..." }`. Add `file://` support to the crawler: it reads saved pages, keyed by `UrlKey`, and needs a test, `crawl_from_snapshot_dir`.
- [ ] **Step 2:** Write the mapping in `porter.toml.example`, using the Landmark importer's `TYPES` table in `import-content.ts` as the source of truth for type and field mapping:
  - `post` → `pages`
  - `events` → `meeting-rooms`
  - the members and payment types dropped, with reasons
  - private → draft
  - the hand-written redirects from `import-redirects.ts` as `[[redirects]]`
  - `key_type = "number"` (Landmark's `legacyId` is numeric)
- [ ] **Step 3:** Run `acceptance/run.sh landmark`. The acceptance table must show:
  - per-type document counts equal to Landmark's `import-content.ts` OK lines, read from `$LANDMARK_DIR/.migration-logs/` or a re-run of its dry run
  - media = 2,865
  - redirects = 248
  - verify URL check ≥ 417/418, against Landmark's dev server as `verify.site_url`
  - media folders = 132
  - submissions imported and equal to Landmark's count
  - issues: kinds present for unmapped fields, unresolved media and Smart Slider (`converter-missing`)
- [ ] **Step 4:** For each mismatch: fix the engine (with a synthetic regression test in the owning task's test file) or fix the mapping, then re-run. Record the final table in `acceptance/landmark/RESULT.md` (counts only, no content).
- [ ] **Step 5:** Commit: `test: Landmark acceptance harness and results`.

### Task 24: Release 2.0

**Files:**
- Delete: `legacy/`, `test_data/` (moved into fixtures earlier), `rust-toolchaindemo.toml`, `.idea/` (and add it to `.gitignore`), `src/plugin.rs`-era docs that describe removed features (`docs/plugin-system-design.md`, `docs/implementation-phases.md`, `docs/improvement-analysis.md`, `docs/tasks.toml`, `docs/plan.md`)
- Create: `UPGRADING.md`
- Modify: `README.md`, `docs/user-guide.md`, `docs/api-reference.md`, `crates/porter-cli/src/commands/upgrade.rs`, `scripts/*` (the paths to the CLI crate)

**Interfaces:**
- `porter upgrade`: when the binary path is under a Homebrew prefix, it keeps the existing brew flow (ported from `legacy/src/main.rs:900-1043`). Otherwise it uses `self_update` with the `umi-labs/porter` GitHub releases, filtered to the target triple.

- [ ] **Step 1: Write the failing test.** `upgrade_detects_brew_prefix`: `install_kind(Path::new("/opt/homebrew/Cellar/porter/2.0.0/bin/porter")) == InstallKind::Brew`, and `/usr/local/bin/porter` outside the Cellar gives `InstallKind::Binary`.
- [ ] **Step 2:** Implement `install_kind(exe: &Path) -> InstallKind` and the upgrade command. Run `cargo test -p porter-cli`. Expected: PASS.
- [ ] **Step 3:** Write the docs. `README.md` covers the five stages, the connectors, `porter.toml` with a full annotated example, the `--json` events schema (`v: 1`), the exit codes, and the env vars. `UPGRADING.md` covers the 1.x → 2.0 changes: flags removed, config format new, seed-file output replaced by the Payload REST load, and Umbraco config now under `[[connectors]] kind = "umbraco-json"`.
- [ ] **Step 4:** Delete `legacy/` and run the full checks: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets`, plus `cargo update && cargo build --workspace` in a scratch clone. Expected: all pass, with clippy warnings allowed.
- [ ] **Step 5:** Commit: `release: porter 2.0.0`. Tagging and publishing go through `scripts/release.sh` and are run by a human.
