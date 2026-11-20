# Sanity source, tidy-ups and flow graph (engine + CLI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Porter reads a Sanity dataset (documents, drafts, references, Portable Text, page builders, images, form submissions) and loads it into a `foundry create` Payload app, with optional, accounted-for tidy-up rules and an engine-built flow graph, all from the CLI.

**Architecture:** A new `sources/sanity/` adapter follows the Umbraco pattern. Connectors write raw rows plus `raw/sanity/data.ndjson`, assets go into the existing content-addressed media store, and normalise writes neutral `Document`/`Media`/`Form`/`Submission`/`Route`/`Redirect` items under connector `sanity`. Tidy rules live in `porter.toml` as `[[tidy]]` and are compiled into `MappingConfig` before schema and load read it. A new `flow` module builds one JSON graph from the store, the mapping and the rules.

**Tech Stack:** Rust 2024 workspace (`porter-model`, `porter-engine`, `porter-cli`, `porter-tui`), reqwest, rusqlite, toml_edit, wiremock (tests), flate2 + tar (export archives), sha1.

**Spec:** `docs/superpowers/specs/2026-11-20-sanity-source-and-tidy-flow-design.md` (read it, including "Builder check: decisions"). The engine conventions are in `docs/superpowers/specs/2026-11-11-porter-engine-design.md`.

## Global Constraints

- The honesty rule: explicit per-item statuses (`ItemStatus::{Done,Partial,Skipped,Failed}`), typed issues through `ctx.issue`, exit codes 0/1/2/3, `--json` NDJSON events `{"v":1,"event":...}`.
- Issue messages must be deterministic. Dedupe is keyed by `(kind, item, stage, message)`.
- Credentials come only from environment variables. The default token env var is `PORTER_SANITY_TOKEN`; `token_env` is validated with `config::is_env_var_name`.
- The canonical connector name is `sanity`. Raw rows use `sanity-api` or `sanity-export`. Files go under `ctx.project.raw_dir("sanity")`.
- Media item ids stay content-addressed (`connector "media"`, source_id = sha256). Sanity CDN URLs are stored **without** query strings.
- No real Kirker data or personal data is ever committed. Fixtures are hand-made.
- Form submissions never appear in flow examples, reports or share output; they show counts only.
- `#![deny(clippy::unwrap_used)]` holds outside tests. `cargo clippy --all-targets` must stay clean.
- Porter never edits the client app's `payload.config.ts`.
- Asset download: bounded concurrency (4), retries through `http::send_with_retry(req, 3)`, per-file cap `DEFAULT_MEDIA_CAP`, SHA-1 verified against `sha1hash`.
- The tidy similarity threshold starts at 0.7 (Jaccard similarity over field-name sets).

## Review Focus

1. **A dataset whose drafts outnumber or replace its published documents.** Draft-only documents load as drafts. A draft of a published document loads only the published version and opens one `unpublished-changes` warning. Neither case may produce two Payload docs. *(Task 6: `draft_and_published_merge_into_one_document`)*
2. **A re-run after an interrupted 24 GB asset fetch.** Files already in `media/` with the matching SHA-1 aren't fetched again, and an interrupted extract marks the rest `Partial`, not `Failed`. *(Task 4: `second_extract_skips_downloaded_assets`, `cancel_marks_remaining_partial`)*
3. **A reference to a document that was deleted in Sanity.** A strong reference raises an `UnresolvedRelation` error; a `_weak` one raises a warning. Load sends null, never a broken id. *(Task 6: `dangling_strong_and_weak_references`)*
4. **A page-builder block type nobody mapped.** Load fails that item with `UnsupportedBlock` rather than dropping the block. Seeding maps every block `_type` found, so a fresh project has none of these. *(Task 9: `seed_maps_every_block_type`)*
5. **Enabling a tidy-up that merges two fields of different shapes, or targets an existing collection missing a field.** This is an error issue that blocks load, and the flow reports it. Nothing is silently coerced. *(Task 11: `shape_conflict_blocks_load`, `existing_target_missing_field_is_unaccounted`)*

---

## File structure

```
crates/porter-model/src/media.rs                       # + focal_x, focal_y
crates/porter-engine/src/config.rs                     # + ConnectorConfig::{SanityApi, SanityExport}, ProjectConfig.tidy
crates/porter-engine/src/registry.rs                   # + "sanity"
crates/porter-engine/src/sources/mod.rs                # + pub mod sanity
crates/porter-engine/src/sources/sanity/mod.rs         # SanitySource, connectors, consts
crates/porter-engine/src/sources/sanity/export.rs      # API stream / tar.gz / folder -> data.ndjson + raw rows
crates/porter-engine/src/sources/sanity/shape.rs       # value classifier
crates/porter-engine/src/sources/sanity/portable_text.rs
crates/porter-engine/src/sources/sanity/assets.rs      # asset refs -> CDN URLs, resumable fetch
crates/porter-engine/src/sources/sanity/analyse.rs     # analysis.json, coverage, submissions/singletons detection
crates/porter-engine/src/sources/sanity/normalise.rs
crates/porter-engine/src/mapping/paths.rs              # dereferencing tokens, lookups
crates/porter-engine/src/mapping/mod.rs                # + lookups, seeding for blocks/forms
crates/porter-engine/src/destinations/payload/schema_generate.rs  # group/array, path field, --into
crates/porter-engine/src/destinations/payload/load/{documents,media}.rs  # path field, focal point
crates/porter-engine/src/tidy/{mod,compile,account,suggest}.rs
crates/porter-engine/src/flow.rs
crates/porter-cli/src/commands/{tidy,flow}.rs          # + main.rs wiring
crates/porter-tui/src/setup.rs                         # Sanity connector sections
crates/porter-engine/tests/sanity.rs                   # end-to-end on the fixture
crates/porter-engine/tests/fixtures/sanity/{data.ndjson,export.tar.gz}
docs/sanity.md, README.md, UPGRADING.md
```

---

### Task 1: Config, registry and `porter init --adapter sanity`

**Files:**
- Modify: `crates/porter-engine/src/config.rs` (ConnectorConfig at 49-104, validate at 186-253)
- Modify: `crates/porter-engine/src/registry.rs`, `crates/porter-engine/src/sources/mod.rs`
- Modify: `crates/porter-engine/src/sources/wordpress/mod.rs:33-43` (`rank()` must bail on Sanity kinds, like UmbracoJson)
- Modify: `crates/porter-engine/src/project.rs:93-128` (template comment block for sanity)
- Modify: `crates/porter-cli/tests/cli.rs:85,370`; `README.md:142-164`; `UPGRADING.md:19`
- Modify: `crates/porter-tui/src/setup.rs` (SECTIONS 76-85, FIELDS 87-220, `secrets()` 380-406, `connector_table()` 433-462, test at 716-776)
- Create: `crates/porter-engine/src/sources/sanity/mod.rs`

**Interfaces:**
- Produces:
  ```rust
  // config.rs
  ConnectorConfig::SanityApi { project_id: String, dataset: String,
      #[serde(default = "default_sanity_token_env")] token_env: String,   // "PORTER_SANITY_TOKEN"
      #[serde(default = "default_sanity_api_version")] api_version: String, // "v2021-06-07"
      #[serde(default)] assets: SanityAssets }
  ConnectorConfig::SanityExport { path: PathBuf, #[serde(default)] assets: SanityAssets }
  #[serde(rename_all = "kebab-case")] pub enum SanityAssets { #[default] Referenced, All }
  // sources/sanity/mod.rs
  pub struct SanitySource;           // Source impl; canonical() == "sanity"
  pub(crate) const CANON: &str = "sanity";
  ```
  `SanitySource::connectors` accepts any number of `SanityApi`/`SanityExport` connectors (exactly one of them is required) plus optional `Crawl` connectors, and bails on anything else with `only "sanity-api", "sanity-export" and "crawl" connectors apply to a sanity source`. `normalise` is a stub returning `Ok(())` until Task 6.

- [ ] **Step 1: Write the failing tests**
  - `config.rs` tests:
    - `sanity_api_parses_with_defaults`: toml `kind="sanity-api"`, `project_id="abc"`, `dataset="production"` gives `token_env=="PORTER_SANITY_TOKEN"`, `api_version=="v2021-06-07"`, `assets==Referenced`.
    - `sanity_token_env_must_be_env_var_name`: `token_env="not a name"` makes `validate()` return an error mentioning `token_env`.
    - `sanity_unknown_key_rejected`.
  - `registry.rs`:
    - `canonical_connector_is_the_adapter_name` passes, with `"sanity"` included.
  - `porter-cli/tests/cli.rs`:
    - Update both adapter-list assertions to `(available: wordpress, umbraco, sanity)`.
    - `init_sanity_project`: `porter init kirker --adapter sanity` exits 0, and `porter.toml` contains `adapter = "sanity"` and a commented `kind = "sanity-api"` block.
  - TUI:
    - `every_connector_kind_round_trips_alone` gets cases for `sanity-api` and `sanity-export`.
- [ ] **Step 2: Run** `cargo test -p porter-engine config:: registry:: && cargo test -p porter-cli --test cli init_ && cargo test -p porter-tui setup::`. Expected: FAIL (unknown variant / adapter).
- [ ] **Step 3: Implement** the variants, the defaults, the validation, the `SanitySource` skeleton, the registry entries, the `rank()` arm, the init template text, the TUI sections (fields: project_id, dataset, token_env, assets / path, assets), `PORTER_SANITY_TOKEN` in `secrets()`, and the docs lines.
- [ ] **Step 4: Run** the same commands plus `cargo clippy --all-targets`. Expected: PASS, no warnings.
- [ ] **Step 5: Commit** `feat(sanity): config, registry and init for the sanity source`.

### Task 2: Reading an export (API stream, tar.gz, folder) into raw rows

**Files:**
- Create: `crates/porter-engine/src/sources/sanity/export.rs`
- Create: `crates/porter-engine/tests/fixtures/sanity/data.ndjson`, the tar.gz is built from it at test time by a helper in `tests/sanity.rs` (flate2 + tar); no binary fixture is committed
- Modify: `crates/porter-engine/Cargo.toml` (add `flate2`, `tar`, `sha1` if absent)
- Test: `crates/porter-engine/tests/sanity.rs`

**Fixture contents** (hand-made, Kirker-shaped, about 30 lines; the shapes are listed in the spec's Testing section):
- `regionPage` ×1, `countryPage` ×2, `cityPage` ×3 (near-identical fields; city → country → region references)
- `hotelBreak` ×2: an image with hotspot and crop, a Portable Text `description` with a `link` markDef and an inline image, and a `flexibleContent` builder array (`hero`, `cardCarousel`, `customCardCarousel`, `accordion`)
- one draft-only doc `drafts.hb-new`, and one `drafts.hb-1` beside a published `hb-1`
- one strong dangling reference and one `_weak` dangling reference
- a geopoint, a `seoGroup`
- singletons `siteSettings` and `homepage`
- `enquiryForm` ×3 with nested `holidayDetails` objects (synthetic names and emails like `person1@example.com`)
- `sanity.imageAsset` ×3 (one unreferenced) and `sanity.fileAsset` ×1, with `url`, `sha1hash`, `size`, `mimeType`, `metadata.dimensions`

**Interfaces:**
- Consumes: Task 1 config.
- Produces:
  ```rust
  pub(crate) struct Export { pub docs: Vec<serde_json::Value>, pub assets: HashMap<String, AssetDoc> } // key: asset _id
  pub(crate) struct AssetDoc { pub id: String, pub url: String, pub sha1: String, pub size: u64,
      pub mime: String, pub width: Option<u32>, pub height: Option<u32>, pub filename: Option<String>, pub alt: Option<String> }
  pub(crate) async fn fetch_api(cfg: &SanityApiCfg, base: &str, token: &str, out: &Path, cancel: &CancellationToken) -> Result<()>
  pub(crate) fn read_archive(path: &Path, out: &Path) -> Result<()>   // tar.gz or folder -> out/data.ndjson
  pub(crate) fn load(out_dir: &Path) -> Result<Export>               // parses raw/sanity/data.ndjson
  ```
  `base` defaults to `https://{project_id}.api.sanity.io`, overridable for tests via the env var `PORTER_SANITY_API_BASE`, which is test-only and documented as such. The URL is `{base}/{api_version}/data/export/{dataset}`, with `Authorization: Bearer <token>`.
  - Connector `analyse` fetches or reads the export into `raw/sanity/data.ndjson` and writes one `RawItem { label: _type, title, status: Some("draft"|"published"), url: None, raw_path: None }` per non-asset document (source id = `_id`), plus an analyse status `Done` per row.
  - `extract` re-reads only with `--refresh`, then runs Task 4's asset fetch.

- [ ] **Step 1: Write failing tests** in `tests/sanity.rs` (using wiremock):
  - `api_export_sends_bearer_and_writes_raw_rows`: the mock asserts the `Authorization: Bearer t0ken` header and serves the fixture. After `analyse`, the raw row count equals the fixture's non-asset doc count, and `raw/sanity/data.ndjson` exists.
  - `api_export_401_is_a_clear_error`: the mock returns 401. The stage errors with a message containing `PORTER_SANITY_TOKEN` and `401`, and the CLI exit code is 1.
  - `missing_token_env_is_a_clear_error`: no env var set gives an error naming the variable.
  - `archive_and_folder_read_the_same_rows`: `sanity-export` pointed at the tar.gz and at a folder holding `data.ndjson` gives identical raw-row sets.
  - `drafts_keep_their_prefix_in_raw_rows`: a raw row exists for `drafts.hb-1` with status `draft`.
- [ ] **Step 2: Run** `cargo test -p porter-engine --test sanity`. Expected: FAIL.
- [ ] **Step 3: Implement.** Stream the response body to `data.ndjson.tmp`, then rename it (atomic, so an interrupted stream leaves no half file). In the archive, `data.ndjson` sits under one top-level folder; take the first `*/data.ndjson`.
- [ ] **Step 4: Run** the tests. Expected: PASS.
- [ ] **Step 5: Commit** `feat(sanity): read exports from the API, archives and folders`.

### Task 3: Value shapes and Portable Text

**Files:**
- Create: `crates/porter-engine/src/sources/sanity/shape.rs`, `crates/porter-engine/src/sources/sanity/portable_text.rs`

**Interfaces:**
- Produces:
  ```rust
  pub(crate) enum Shape { Reference { id: String, weak: bool }, Image, File, PortableText, Builder,
      Slug(String), Geopoint { lat: f64, lng: f64 }, Date(DateTime<Utc>), Object, List, Scalar }
  pub(crate) fn shape(v: &Value) -> Shape
  pub(crate) fn asset_ref(v: &Value) -> Option<&str>        // image/file object -> "image-<sha1>-<w>x<h>-<ext>"
  pub(crate) fn pt_to_richtext(blocks: &[Value], cx: &mut PtCx) -> RichText
  pub(crate) struct PtCx<'a> { pub resolve_doc: &'a dyn Fn(&str) -> Option<ItemId>,
      pub resolve_asset: &'a dyn Fn(&str) -> Option<ItemId>, pub issues: &'a mut Vec<PendingIssue> }
  ```
  - `PortableText`: an array whose items are all objects with `_type == "block"`, or `block` mixed with inline objects.
  - `Builder`: an array of objects that all carry `_type`, none of them `block`, and not all `reference`.
  - A list of references stays `List`.
  - `Date` only for RFC 3339 or `YYYY-MM-DD` strings.

- [ ] **Step 1: Write failing unit tests**
  - `shape_classifies_each_kind`: one case per variant, plus `"2024-01-22"` → Date and `"next week"` → Scalar.
  - `pt_styles_lists_marks_links`: PT covering `h2`, `blockquote`, a `bullet` list with `level` 1 and 2 (the second nested), marks `strong`/`em`/`underline`/`code`/`strike-through`, an external `link` markDef (→ `LinkTarget::Url`), an internal one with a `reference` field (→ `LinkTarget::Doc`), and an inline `image` (→ `RtBlock::Media`). Assert the exact `RichText` value.
  - `pt_unknown_inline_object_is_converter_missing`: a `{_type:"youtube"}` gives `RtBlock::Raw` holding the JSON, plus one `ConverterMissing` warning.
- [ ] **Step 2: Run** `cargo test -p porter-engine sanity::shape sanity::portable_text`. Expected: FAIL.
- [ ] **Step 3: Implement.** Group consecutive `listItem` blocks into `RtBlock::List`, nesting by `level`. `ordered = listItem == "number"`.
- [ ] **Step 4: Run** the tests. Expected: PASS.
- [ ] **Step 5: Commit** `feat(sanity): value shapes and Portable Text conversion`.

### Task 4: Resumable asset fetch

**Files:**
- Create: `crates/porter-engine/src/sources/sanity/assets.rs`

**Interfaces:**
- Consumes: `Export` (Task 2), `asset_ref` (Task 3), `store.put_media_bytes`, `store.media_by_url`, `fetch::{media_client, download_capped, DEFAULT_MEDIA_CAP}`, `http::send_with_retry`.
- Produces:
  ```rust
  pub(crate) fn referenced(export: &Export) -> BTreeSet<String>                 // asset _ids used anywhere (drafts included)
  pub(crate) fn cdn_url(asset: &AssetDoc) -> String                             // asset.url with query stripped
  pub(crate) async fn fetch(ctx: &StageCtx, export: &Export, which: SanityAssets) -> Result<FetchSummary>
  pub(crate) struct FetchSummary { pub fetched: u64, pub skipped: u64, pub failed: u64, pub unreferenced: u64, pub unreferenced_bytes: u64 }
  ```
  - Items are statused per asset under a raw `ItemId::new(Media, SourceRef{connector:"sanity-assets", source_id: asset _id})`.
  - Skip when `!ctx.refresh && store.media_by_url(url)?.is_some()`.
  - Verify `sha1(bytes) == asset.sha1`. A mismatch raises a `MissingMedia` error, *"<asset id>: downloaded file doesn't match Sanity's SHA-1"*, and the item `Failed`.
  - Run with concurrency 4 (a `Semaphore`/`JoinSet`, as `connectors/crawl.rs:589` does).
  - Report progress via `ctx.progress(done, Some(total), "assets")`.
  - On cancel, mark the remaining items `Partial{"interrupted before download"}`.

- [ ] **Step 1: Write failing tests** (wiremock CDN; fixture `url`s point at the mock):
  - `fetches_only_referenced_by_default`: `fetched == 3` and `unreferenced == 1`. `assets = "all"` gives `fetched == 4`.
  - `retries_a_500_then_succeeds`: the first request gets 500, the second 200. The item is `Done` and the mock saw 2 requests.
  - `sha1_mismatch_is_an_error`: one issue of kind `MissingMedia` with severity Error.
  - `second_extract_skips_downloaded_assets`: the second run makes 0 requests and has `skipped == 3`.
  - `cancel_marks_remaining_partial`: cancel after the first download; the rest are `Partial`.
- [ ] **Step 2: Run** `cargo test -p porter-engine --test sanity assets_`. Expected: FAIL.
- [ ] **Step 3: Implement**, wiring `fetch` into both connectors' `extract`.
- [ ] **Step 4: Run** the tests. Expected: PASS.
- [ ] **Step 5: Commit** `feat(sanity): resumable, verified asset fetch`.

### Task 5: Media focal point

**Files:**
- Modify: `crates/porter-model/src/media.rs` (add `#[serde(default)] pub focal_x: Option<f64>, pub focal_y: Option<f64>`, percent 0–100)
- Modify: `crates/porter-engine/src/destinations/payload/load/media.rs` (send `focalX`/`focalY` on create and on PATCH when changed, next to `alt` at line 65)

**Interfaces:**
- Produces: the `Media.focal_x` / `Media.focal_y` fields. Task 6 fills them as Sanity `hotspot.x * 100.0` and `hotspot.y * 100.0`.

- [ ] **Step 1: Write failing tests**
  - `media_roundtrip_without_focal` (old JSON without the fields still decodes).
  - In `load/media.rs` tests: `upload_sends_focal_point` (the request body has `focalX: 30.0, focalY: 60.0`).
- [ ] **Step 2: Run** `cargo test -p porter-model media && cargo test -p porter-engine load::media`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests, plus `cargo test -p porter-desktop` on macOS (the desktop app reads Media). Expected: PASS.
- [ ] **Step 5: Commit** `feat(model): media focal point, sent to Payload as focalX/focalY`.

### Task 6: Normalise

**Files:**
- Create: `crates/porter-engine/src/sources/sanity/normalise.rs`
- Modify: `crates/porter-engine/src/sources/sanity/mod.rs` (wire `normalise`)

**Interfaces:**
- Consumes: Tasks 2–5; `ctx.canonical_items`, `store.put_item`, `drop_stale` pattern (`umbraco/normalise.rs:514-530`); `html::PendingIssue`.
- Produces (items under connector `CANON = "sanity"`):
  - **Document:** `ItemId::new(Document, SourceRef{connector:"sanity", source_id: <_id without "drafts.">})`. Its fields are:
    - `slug` from `slug.current`, else `slugify(title)`, else the id
    - `title` from the first of `title`, `name`, `heading`, `label`, or the per-type `title_field`, else the id
    - `path` from Task 7's patterns, else `/{type}/{slug}/`
    - `seo` from a `seoGroup`-shaped object, matching field names case-insensitively: `metaTitle|title` → title, `metaDescription|description` → description, `noIndex|noindex` → noindex, `ogImage|shareImage` → og_image
    - `original` = the raw doc
  - **Drafts:**
    - a published `X` with no draft → `Published`
    - only `drafts.X` → `Draft`
    - both → the published content, plus one Warning `UnmappedField` with message *"<type> \"<title>\" has unpublished changes in Sanity; only the published version is loaded"* (`IssueKind::UnmappedField` is reused; no new kind, to keep the model stable)
  - **Fields** (by `Shape`):
    - Reference → `Doc`. A strong missing target raises an `UnresolvedRelation` Error, a `_weak` one a Warning, and the field value becomes `Null`.
    - Image/File → `Media` (looked up via `media_by_url(cdn_url)`; not downloaded raises a `MissingMedia` warning). The first use sets `focal_x`/`focal_y`, `alt` and `caption`; asset `altText` wins for alt. Later disagreeing uses raise one Warning per asset, *"<asset id> is used with different focal points or alt text; the first use was kept"*. `crop` raises Info, *"<type> \"<title>\" crops <asset id>; Payload has no crop, so the full image is used"*.
    - PortableText → `RichText`.
    - Builder → `Layout`. One `Block` per item with `original: Some(Original{builder:"sanity", module: _type, classes: vec![], settings: item})`. `props` hold the item's fields, converted recursively. `kind` is `Image` for `imageItem`/`image`, `Video` for `video`, `Divider` for `separator`, `Accordion` for `accordion`, else `Raw`.
    - Geopoint → `Object{lat,lng}`. Date → `Date`. Slug (nested) → `Text`.
    - Object → `Object`, recursive, dropping `_key` and keeping `_type` only inside lists.
    - List → `List`, recursive.
  - **Form submission types** (Task 9's detection, or `mapping.forms` listing them):
    - one `Form` per type, `ItemId::new(Form, {connector:"sanity", source_id:"form:<type>"})`, with fields = the union of submitted field names (dotted when nested)
    - one `Submission` per doc, values flattened to dotted keys and scalars stringified
    - no `Document` items for these
  - **Routes:** one `Route` per published Document whose `path` rendered. Its URL is `{verify.site_url}{path}` when a site URL is configured, else none is written. Crawl-connector URLs merge by URL (Task 8).
  - **Redirects:** manual `[[redirects]]` → `Redirect{origin: RedirectOrigin::Manual}` items (copy `sources/wordpress/redirects.rs:306-326`).
  - **Stale items:** documents not written this run are soft-deleted and marked Skipped.
  - Run normalise on a 16 MB-stack scoped thread, catching panics per document (`umbraco/normalise.rs:65-80`).

- [ ] **Step 1: Write failing tests** in `tests/sanity.rs` (analyse, extract with pre-seeded media, then normalise):
  - `draft_and_published_merge_into_one_document`: exactly 1 Document for `hb-1` (`Published`) plus the unpublished-changes warning; `hb-new` is `Draft`.
  - `dangling_strong_and_weak_references`: one Error and one Warning `UnresolvedRelation`, and both field values `Null`.
  - `image_hotspot_becomes_focal_point`: the media item has `focal_x == Some(30.0)`, plus one crop Info issue.
  - `builder_array_becomes_layout_with_modules`: `hotelBreak.flexibleContent` is a `Layout` whose modules are `["hero","cardCarousel","customCardCarousel","accordion"]`.
  - `enquiries_become_form_and_submissions`: 1 Form with a field `holidayDetails.destination`, 3 Submissions, and no `enquiryForm` Documents.
  - `normalise_golden`: the JSON of all Documents (sorted by id) equals `tests/fixtures/sanity/normalised.golden.json`. Bless with `PORTER_BLESS=1`, as `lexical.rs:835` does.
- [ ] **Step 2: Run** `cargo test -p porter-engine --test sanity`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** with `PORTER_BLESS=1` once, review the golden file by eye against the fixture, then run without it. Expected: PASS.
- [ ] **Step 5: Commit** `feat(sanity): normalise documents, drafts, references, media, builders and submissions`.

### Task 7: Path patterns that follow references, and lookups

**Files:**
- Modify: `crates/porter-engine/src/mapping/paths.rs` (`render` at 55), `crates/porter-engine/src/mapping/mod.rs` (add `lookups`)

**Interfaces:**
- Produces:
  ```rust
  // MappingConfig
  #[serde(default)] pub lookups: IndexMap<String, IndexMap<String, String>>,
  // paths.rs
  pub fn render(pattern: &str, doc: &Document, parent_path: Option<&str>, cx: &PathCx) -> Result<String, String>
  pub struct PathCx<'a> { pub docs: &'a HashMap<ItemId, Document>, pub lookups: &'a IndexMap<String, IndexMap<String, String>> }
  ```
  - Token grammar: `{name(.name)*(|lookup:<table>)?}`. A segment that names a `Doc` field follows it to the target Document. `slug`, `title`, `type`, `id` and `path` are Document attributes; anything else is a field of type Text or Number.
  - An unresolvable token or a missing lookup key is an `Err` naming the token. The caller opens an `UnresolvedRelation` warning and the doc keeps the fallback path `/{type}/{slug}/`.
  - All existing callers (WordPress, Umbraco, verify) pass a `PathCx` with an empty `docs` map where they have none. Existing tokens behave exactly as before.

- [ ] **Step 1: Write failing tests** in `paths.rs`:
  - `follows_references`: city → country → region with `/destinations/{country.region.slug}/{country.slug}/{slug}` renders `/destinations/europe/spain/marbella`.
  - `lookup_maps_values`.
  - `missing_lookup_key_is_err_naming_token`.
  - Existing path tests still pass unchanged.
- [ ] **Step 2: Run** `cargo test -p porter-engine mapping::paths`. Expected: FAIL.
- [ ] **Step 3: Implement**, updating callers.
- [ ] **Step 4: Run** `cargo test -p porter-engine`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(mapping): path tokens follow references; lookup tables`.

### Task 8: Old-site URLs alongside a Sanity source

**Files:**
- Modify: `crates/porter-engine/src/sources/sanity/{mod,normalise,analyse}.rs`

**Interfaces:**
- Consumes: the existing `Crawl` connector (unchanged), whose rows are read via `ctx.canonical_items`/raw rows the way WordPress's merge reads them (`wordpress/analyse.rs`).
- Produces:
  - When a `crawl` connector is configured, its discovered URLs are merged with the pattern routes by normalised URL (`url::normalise_url`).
  - A crawled URL that no Document path produced raises a Warning `UnresolvedLink`, *"old site URL <url> has no document"*. Verify still checks it, and it keeps its `http_status` from the crawl.
  - A pattern URL the crawl didn't find raises Info, *"<url> wasn't found on the old site"*.

- [ ] **Step 1: Write failing test**
  - `crawl_urls_merge_with_pattern_routes`: a wiremock old site whose sitemap lists `/hotel/hb-1/` and `/about/`. With `path = "/hotel/{slug}/"` on hotelBreak, `/hotel/hb-1/` resolves to its Document, and `/about/` gives one Warning.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(sanity): merge old-site crawl URLs with pattern routes`.

### Task 9: Analysis, coverage and seeding

**Files:**
- Create: `crates/porter-engine/src/sources/sanity/analyse.rs`
- Modify: `crates/porter-engine/src/mapping/mod.rs` (`seed_from` at 338-373)

**Interfaces:**
- Produces:
  - `raw/sanity/analysis.json`, holding:
    - `types: {name: {published, draft_only, draft_of_published}}`
    - `blocks: {_type: count}`
    - `references: [{from, field, to, count}]`
    - `dangling: [{from_id, field, to_id, weak}]`
    - `submission_types: [name]`
    - `singletons: [name]`
    - `assets: {referenced, referenced_bytes, unreferenced, unreferenced_bytes}`
    - `unknowns: ["Studio field titles and validation", "frontend URL rules"]`
  - `report::write_coverage` with `platform: Some("sanity")`.
  - Detection rules:
    - **Submission type:** ≥ 50 docs, no `slug` field in any doc, and at least one field named (case-insensitive) `email`, `firstName`, `lastName`, `phone`/`phoneNumber`, or nested `*Details.email`.
    - **Singleton:** exactly 1 published doc whose `_id` equals its `_type` or doesn't look like a UUID.
  - Seeding (when `[mapping]` is empty):
    - types as today, with `key_type = "text"`
    - submission types listed under `mapping.forms`, and their types given `to = None, drop_reason = "form submissions; loaded as submissions"`
    - one `[[mapping.blocks]]` per block `_type`, `{ match = { module = "<_type>" }, to = "<kebab _type>" }`, with fields mapped to themselves
    - `raw_block` unset

- [ ] **Step 1: Write failing tests**
  - `analysis_counts_drafts_and_assets` (the fixture's exact numbers).
  - `detects_submissions_and_singletons` (detection is `fn detect_submissions(stats, min_docs: usize)` with `const MIN_SUBMISSION_DOCS: usize = 50` in production; the test passes `2`).
  - `seed_maps_every_block_type`.
  - `coverage_platform_is_sanity`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p porter-engine`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(sanity): analysis, coverage and mapping seed`.

### Task 10: Payload schema for nested data, a `path` field, and `--into`

**Files:**
- Modify: `crates/porter-engine/src/destinations/payload/schema_generate.rs` (`kind_for` 80-148, collection template 289-311, block configs 317-364)
- Modify: `crates/porter-engine/src/mapping/summary.rs` (nested field stats for Object / List of Object; Doc inside Object → relation_types)
- Modify: `crates/porter-engine/src/destinations/payload/load/documents.rs` (`build` 333-392: send `path`)
- Modify: `crates/porter-cli/src/main.rs` + its schema command (`--into <dir>`)

**Interfaces:**
- Produces:
  - `FieldStats.children: IndexMap<String, FieldStats>` (serde default) for Object and List-of-Object values. A `Doc` at any depth feeds `relation_types` at its own path.
  - `kind_for`:
    - an Object whose children are consistent → `group` with sub-fields
    - a List of Object → `array` with sub-fields
    - `json` only for mixed or inconsistent shapes, and then with an Info issue *"<collection>.<field> has mixed shapes; generated as json"*
  - Every generated collection gets `{ name: 'path', type: 'text', index: true, unique: true, admin: { readOnly: true } }`, and load sends `path`.
  - `pub fn generate_into(ctx: &StageCtx, repo: &Path) -> Result<IntoReport>`:
    - writes `src/collections/porter/<Pascal>.ts`, `src/blocks/porter/<Ident>Block.ts` and `src/collections/porter/index.ts`, which exports `porterCollections` and `porterBlocks`
    - refuses (exit 2, an Error issue per collision) a slug already declared in `repo/src/collections/*.ts` (regex `slug:\s*['"]<slug>['"]`) unless it's under `porter/`
    - prints the two `payload.config.ts` lines and the catch-all route file contents (the spec's builder-check item 1)
  - `IntoReport { written: Vec<PathBuf>, collisions: Vec<String> }`.

- [ ] **Step 1: Write failing tests**
  - `object_becomes_group`, `list_of_objects_becomes_array`, `doc_inside_object_is_relationship`, `mixed_shapes_fall_back_to_json_with_info` (golden TS snippets).
  - `every_collection_has_path_field`.
  - `into_refuses_existing_slug`: a temp repo with `src/collections/Pages.ts` declaring `slug: 'pages'` and mapping `to = "pages"` gives one collision and no files written.
  - `into_writes_index`.
  - Extend `tests/payload_live.rs` with `live_sanity_fixture_round_trip`: generate from the Sanity fixture into `tests/payload-app`'s porter folder, migrate, load with `--apply`, then fetch one hotelBreak and assert the group, array, relationship and `path` values. This runs in the `payload-golden` CI job.
- [ ] **Step 2: Run** `cargo test -p porter-engine schema_generate`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the unit tests, then `payload_live` locally against `tests/payload-app` (see `.github/workflows/ci.yml:67-114` for the setup). Expected: PASS.
- [ ] **Step 5: Commit** `feat(payload): group/array fields, path field, schema generate --into`.

### Task 11: Tidy rules: config, compile and accounting

**Files:**
- Create: `crates/porter-engine/src/tidy/{mod.rs,compile.rs,account.rs}`
- Modify: `crates/porter-engine/src/config.rs` (`ProjectConfig.tidy: Vec<TidyRule>`), `crates/porter-engine/src/lib.rs`
- Modify: wherever schema and load read `config.mapping` (`destinations/payload/mod.rs:93-128`, `load/mod.rs`), so they read `tidy::effective_mapping(&config)?`
- Modify: `crates/porter-engine/src/fingerprint.rs:36-64` (normalise and later stages hash `[[tidy]]`)

**Interfaces:**
- Produces:
  ```rust
  #[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
  pub enum TidyKind { MergeTypes { from: Vec<String>, into: String, discriminator: Option<Discriminator>,
                                   #[serde(default)] parent: IndexMap<String, String>,
                                   #[serde(default)] drop: IndexMap<String, String> },
                      MergeBlocks { from: Vec<String>, into: String,
                                    #[serde(default)] fields: IndexMap<String, String>,
                                    #[serde(default)] drop: IndexMap<String, String> } }
  pub struct TidyRule { pub id: String, #[serde(default)] pub enabled: bool, pub label: Option<String>,
                        pub why: Option<String>, #[serde(flatten)] pub kind: TidyKind }
  pub struct Discriminator { pub field: String, pub values: IndexMap<String, String> }
  pub fn effective_mapping(cfg: &ProjectConfig) -> Result<MappingConfig>
  pub fn account(store: &Store, cfg: &ProjectConfig, dest_fields: Option<&DestSchema>) -> Result<Vec<RuleOutcome>>
  pub struct RuleOutcome { pub rule: String, pub items_in: u64, pub items_out: u64,
                           pub carried: u64, pub renamed: u64, pub dropped: u64, pub problems: Vec<Problem> }
  pub enum Problem { ShapeConflict { field: String, types: Vec<String> }, Unaccounted { field: String, dest: String } }
  ```
  - `effective_mapping` compiles only enabled rules:
    - **`merge-types`:** for each from-type, set `to = into`; add the discriminator as a constant field (via the existing `Default` transform on a synthetic `_discriminator` source path, or a new `FieldMap.constant: Option<String>`; pick the smaller diff). Set the parent from the named reference field.
    - **`merge-blocks`:** add one `BlockMap{ match.module = <from>, to = into, fields = renames }` per from-type, ahead of existing block maps.
    - Hand-written `fields`/`path`/`status` on from-types are kept.
  - `account` runs at the start of `load` (dry run and apply). Each `Problem` opens an Error `SchemaMismatch` on no item:
    - *"tidy \"<id>\": <field> is <shape A> in <type> but <shape B> in <type>"*
    - *"tidy \"<id>\": <dest> has no field <field>"*
  - The outcomes are written to `reports/tidy.json` for Task 13.
  - `Unaccounted` is only possible when `into` names an existing destination collection (the schema check's view, `destinations/payload/schema_check.rs`).

- [ ] **Step 1: Write failing tests**
  - `disabled_rule_changes_nothing`: `effective_mapping` equals `mapping`.
  - `merge_types_compiles_to_one_collection_with_discriminator`: from the fixture, 6 destination pages go to `destinations`, with the `kind` values region/country/city and the parents set.
  - `merge_blocks_compiles_block_maps`.
  - `hand_renames_survive_compile`.
  - `shape_conflict_blocks_load`: make `cityPage.images` a list and `countryPage.images` text; load exits 2 with one SchemaMismatch error.
  - `existing_target_missing_field_is_unaccounted`.
  - `accounting_counts_in_out`: `items_in == items_out == 6`, `dropped == 0`.
- [ ] **Step 2: Run** `cargo test -p porter-engine tidy`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace --exclude porter-desktop`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tidy): merge-types and merge-blocks rules, compiled into the mapping, with accounting`.

### Task 12: Tidy suggestions and the `porter tidy` command

**Files:**
- Create: `crates/porter-engine/src/tidy/suggest.rs`, `crates/porter-cli/src/commands/tidy.rs`
- Modify: `crates/porter-engine/src/mapping/edit.rs` (`MappingDoc`: add `set_tidy_enabled(&mut self, id: &str, on: bool) -> Result<()>` and `add_tidy(&mut self, rule: &TidyRule)`, keeping comments), `crates/porter-cli/src/main.rs`
- Modify: `crates/porter-engine/src/session.rs:768` (after analyse seeding, also append suggestions not already present by id)

**Interfaces:**
- Produces:
  ```rust
  pub fn suggest(summary: &TypeSummary, existing: &[TidyRule]) -> Vec<TidyRule>  // all enabled == false
  ```
  - **Types:** cluster types pairwise where `jaccard(field names) >= 0.7`, using connected components; exclude submission and singleton types. When a member references another member, suggest `parent` from that field.
  - **Blocks:** cluster `BlockStats` by module with the same threshold.
  - **id:** kebab of `into`.
  - **`into`:** the plural noun shared by the member names, else the first member's name + `-merged`. Kirker's three page types should come out as `destination-pages`; tune the heuristic on the fixture only.
  - **label:** *"Merge <A>, <B> and <C> into <Into>"*.
  - **why:** *"They share <n> of <m> fields"*.
  - CLI, with every subcommand supporting `--json`:
    - `porter tidy list`: id, enabled, label, why
    - `porter tidy suggest`: writes new suggestions, prints them
    - `porter tidy enable <id>` / `porter tidy disable <id>`: unknown id exits 1 with the known ids listed

- [ ] **Step 1: Write failing tests**
  - `suggests_merging_the_three_page_types` (fixture): one MergeTypes rule with from = the three page types, `enabled == false`, and `why` mentioning shared fields.
  - `suggests_merging_card_carousels`.
  - `no_duplicate_suggestions_on_rerun`.
  - CLI: `tidy_enable_round_trip` (enable, then `list --json` shows `enabled: true`; the comment lines in `porter.toml` are kept); `tidy_enable_unknown_id_exits_1`.
- [ ] **Step 2: Run** `cargo test -p porter-engine tidy::suggest && cargo test -p porter-cli --test cli tidy_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the tests. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tidy): suggestions and porter tidy list/suggest/enable/disable`.

### Task 13: Flow graph and `porter flow`

**Files:**
- Create: `crates/porter-engine/src/flow.rs`, `crates/porter-cli/src/commands/flow.rs`
- Modify: `crates/porter-engine/src/session.rs` (after normalise and after load, write `reports/flow.json`), `crates/porter-cli/src/main.rs`

**Interfaces:**
- Consumes: `tidy::effective_mapping`, `reports/tidy.json` (Task 11), store statuses (`store.statuses(stage)`), `MappingConfig`.
- Produces:
  ```rust
  #[derive(Serialize)] pub struct FlowGraph { pub v: u8 /* 1 */, pub nodes: Vec<FlowNode>, pub edges: Vec<FlowEdge>, pub counted_at: Option<String> }
  #[derive(Serialize)] pub struct FlowNode { pub id: String, pub column: Column, pub kind: NodeKind, pub title: String,
      pub summary: String, pub enabled: bool, pub counts: Option<Counts>, pub example: Option<Example> }
  #[serde(rename_all = "kebab-case")] pub enum Column { Source, Tidy, Destination }
  #[serde(rename_all = "kebab-case")] pub enum NodeKind { DocType, BlockType, Submissions, MergeTypes, MergeBlocks, Collection, Block, Form }
  pub struct Counts { pub items_in: u64, pub ok: u64, pub skipped: u64, pub failed: u64, pub dropped: u64 }
  pub struct Example { pub title: String, pub changed: Vec<(String /*field*/, String /*before*/, String /*after*/)> }
  pub struct FlowEdge { pub from: String, pub to: String, pub count: Option<u64> }
  pub fn build(project: &Project, cfg: &ProjectConfig) -> Result<FlowGraph>
  ```
  - Node ids:
    - source nodes: `src:<type>` and `blk:<module>`
    - tidy nodes: `tidy:<id>`
    - destination nodes: `dst:<collection>`, `dstblk:<slug>`, `form:<name>`
  - Disabled rules are drawn with edges going straight from source to destination; the dashed card still appears.
  - `counts` and `counted_at` are `None` until a load (dry or apply) has run since the last normalise.
  - `summary` is plain English generated from the rule or mapping, e.g. *"Merges City pages, Country pages and Region pages into Destinations. Each keeps what it was in Kind."* or *"Moves Hotel breaks into Hotel breaks unchanged."*
  - `example` is the first item by title, showing up to 5 changed fields. It's always `None` for submission and form nodes.
  - `porter flow` prints a plain-text column list. `--json` prints the graph as one NDJSON `{"v":1,"event":"flow","graph":{...}}` line.

- [ ] **Step 1: Write failing tests**
  - `flow_before_dry_run_has_no_counts`.
  - `flow_after_dry_run_counts_match_load`.
  - `disabled_rule_is_drawn_and_bypassed` (edges go src → dst directly, and the tidy node has `enabled == false`).
  - `submissions_never_have_examples`.
  - `flow_json_shape_golden`.
- [ ] **Step 2: Run** `cargo test -p porter-engine flow`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace --exclude porter-desktop && cargo clippy --all-targets`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(flow): engine-built flow graph and porter flow`.

### Task 14: Docs and an end-to-end CLI run

**Files:**
- Create: `docs/sanity.md` (connectors, token creation in Sanity Manage, assets, drafts, submissions, path patterns with references, `--into` + the config lines, tidy-ups, flow)
- Modify: `README.md` (adapter list, a Sanity quick start), `UPGRADING.md` (new connector kinds; `[[tidy]]`; `lookups`)
- Test: `crates/porter-cli/tests/cli.rs`

- [ ] **Step 1: Write failing test** `sanity_end_to_end_cli`: against wiremock (API + CDN), run `porter init` → `analyse` → `extract` → `normalise` → `tidy enable destination-pages` → `flow --json`. Every command exits 0 or 2 as expected, and the final flow has a `tidy:destination-pages` node with `enabled: true`.
- [ ] **Step 2: Run** `cargo test -p porter-cli --test cli sanity_end_to_end_cli`. Expected: FAIL until the docs-only changes plus any CLI glue land.
- [ ] **Step 3: Implement** the remaining glue and write the docs.
- [ ] **Step 4: Run** `cargo test --workspace --exclude porter-desktop && cargo clippy --all-targets`. Expected: PASS.
- [ ] **Step 5: Commit** `docs(sanity): guide, README and UPGRADING; end-to-end CLI test`.

---

## After this plan (not tasks here)

- Hub Flow tab (Part D) and desktop Flow view (Part E), after Isaac reviews the design.
- The Kirker run: `foundry create`, then porter against the live dataset with `PORTER_SANITY_TOKEN`, timed.
