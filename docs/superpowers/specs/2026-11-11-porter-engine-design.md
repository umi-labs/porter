# Porter engine design

Status: draft for approval · 2026-11-11

This is the first of four designs. The later three each depend on this one:

1. **Engine** (this document): the pipeline, the neutral data model, adapters, and the CLI.
2. **Terminal UI**: a lazygit/lazydocker-style interface over the engine (`ratatui`).
3. **Desktop app**: a native Rust app over the engine (GPUI, Zed's framework).
4. **Hub**: a central server that local projects sync to. It provides shareable views, client uploads and remote runs.

The engine must not assume any of them exists, and must make all of them possible.

## Why

Porter is meant to be Umi's migration tool. In practice it can't migrate a site. The audit on 2026-11-11 found:
- The WXR parser returns 0 posts.
- Two of the three source paths error out before reading anything.
- Body content is written as raw HTML.
- Media, relationships and redirects are absent.
- Failures exit 0.

The last real migration, The Landmark London (WordPress/Divi → Payload), was done instead with roughly 3,150 lines of TypeScript and PHP written for that one site (`~/Desktop/work/clients/landmark/migration/`). That code worked: 2,865 media items, 248 redirects, and 417 of 418 old URLs resolving. It holds most of the domain knowledge this engine needs.

Porter stays in Rust. The engine is rebuilt using what the Landmark pipeline learned, so the next migration (from any source, to any destination) runs through porter instead of one-off scripts.

## Goals

- One pipeline, five stages, each runnable on its own and resumable: **analyse → extract → normalise → prepare destination → load**, with **verify** after load.
- A **neutral content model** that is not shaped like any one platform. It covers documents, rich text, page layouts, media, routes, redirects and site-wide data.
- **Sources and destinations are adapters** around that model. Nothing in the core names a platform.
- **Works with whatever access exists.** The public site alone is a fully supported starting point. An admin login, API, database dump or server access each add detail. The analysis states what each kind of access did or didn't reveal.
- **Honest by construction.** Every item ends with an explicit status, every problem is a typed issue, exit codes tell the truth, and porter states up front what it can't convert.
- **Headless first.** Every action is a CLI command with `--json` output and no prompts unless it runs in a terminal and `--interactive` is set. The same engine is a library the terminal UI, desktop app and hub call directly.
- **Re-runs are always safe.** Everything is keyed by source identity, and a re-run updates instead of duplicating.

## Non-goals (for this design)

- The terminal UI, desktop app and hub. Each gets its own design. This one only guarantees the engine exposes what they need: a library API, progress events, and a queryable project store.
- Matching the old site's look pixel for pixel. Landmark spent many commits copying Divi's CSS and then partly undid it. Porter carries layout *structure* and records original styling as metadata. It does not regenerate CSS.
- Image re-encoding and resizing. The destination's own pipeline does that (Payload's `sharp` sizes, Sanity's asset pipeline).
- Scripting or plug-in code for per-client logic. Per-client decisions are configuration (see Mapping). Anything config can't express falls back to a raw block plus an issue.

## Anti-goals (failure even if it "works")

- Content arrives but is **wired up wrong**: wrong parents, links pointing at the wrong pages, images detached from their content, old URLs leading nowhere.
- Anything is **dropped or downgraded silently**.
- A run reports success when it wasn't one.
- The neutral model drifts towards Payload (or any other platform), so other destinations become second-class.

## The project

A migration is a **project folder** on disk, one per client migration. It is the source of truth; the hub (later) syncs a copy of it.

```
<client>-migration/
  porter.toml          # connectors, mapping decisions, destination
  project.db           # SQLite: items, statuses, issues, runs, the normalised model
  raw/<connector>/...  # what each connector fetched, untouched (HTML, JSON, SQL rows, WXR)
  media/ab/cd/<sha256> # content-addressed media files
  reports/             # generated HTML reports (self-contained, shareable as a single file)
```

- **SQLite** (`rusqlite`, bundled) holds everything structured. The terminal UI, desktop app and hub all need filtered queries ("failed items of type page", "issues of kind unresolved-link") that a pile of JSON files makes painful. `porter export --json` dumps the normalised model for anyone who wants files.
- **Raw is kept** so normalisation can be re-run after a converter improves, without fetching again.
- **Media is stored by content hash**, which dedupes it. The URL variants (sizes, `-scaled`, case, other hosts) all map to one hash.
- **Every stage run is recorded** (start, end, counts by status, porter version), so reports can show history and a launch-day re-run can be compared with the first run.
- Credentials never go in `porter.toml`. They come from environment variables, or from the OS keychain via the `keyring` crate for interactive use, referenced by name.

## The neutral model

The rules: all of it serialises to JSON, every entity carries its **source identity** (connector, source ID, source URL), and every entity carries a **status**.

- **Document**: `id` (porter's own, stable across runs, derived from source identity), `type` (the source's type name, e.g. `post`, `page`, `room`), `status` (published/draft/private/scheduled), `locale` plus a link to its translations, `slug`, `path`, `parent`, `title`, `fields` (typed values: text, number, bool, date, rich text, layout, media ref, document ref, list, object), `seo`, `created`/`updated`/`author`, and `original` (source-specific leftovers kept for reference).
- **Rich text**: a neutral tree. Block nodes: paragraph, heading 1-6, list (ordered/unordered, nested), quote, code, horizontal rule, table, media (ref + caption + alignment + width), embed (provider + URL), and raw (original markup). Inline nodes: text with marks (bold, italic, underline, strike, code, sub, sup), link (internal document ref or external URL, plus new tab), and line break. Conversions to Lexical, Portable Text and Gutenberg HTML happen only at load.
- **Layout**: a tree of neutral blocks for page-builder content. Containers: section, row, column (with width fraction). Content blocks: rich text, image, gallery, button, video, embed, map, form ref, tabs, accordion, slider, spacer, divider, and raw. Each block keeps `original` (builder module name, classes, settings) so nothing is lost. Converters map builder modules to these blocks, and anything unrecognised becomes a raw block plus an `unsupported-block` issue.
- **Media**: content hash, every URL variant seen, filename, MIME type, dimensions, alt, caption, title, credit, folder (from the upload path), and **usages** (which documents and fields reference it).
- **Route**: every URL known from any connector, with what it resolves to: a document, a media file, a redirect, or nothing. Plus HTTP status and title as crawled.
- **Redirect**: from, to, status code, and source (plugin, crawl, field, or manual). Chains are collapsed to one hop and loops removed, each recorded as an issue.
- **Site-wide data**: menus (tree of items, each a document ref or URL), taxonomies and terms (and their assignments), forms (fields, validation, conditions, notifications, confirmation) and optionally submissions, site settings (name, front page, permalink structure, SEO defaults), and authors.

### Items, statuses and issues

Every entity is an **item**. Each item has a status per stage: `done`, `partial`, `skipped` (with a reason), or `failed` (with an error). An item can't leave a stage without one.

An **issue** has:
- a `kind` from a fixed, documented list, e.g. `unresolved-link`, `missing-media`, `unmapped-field`, `unsupported-block`, `unresolved-relation`, `parent-missing`, `redirect-chain`, `redirect-loop`, `write-failed`, `access-gap`, `converter-missing`, `schema-mismatch`
- a severity: `error`, `warning` or `info`
- the item and stage it came from
- a message, with details as JSON
- a resolution: `open`, `fixed-by-rerun`, `handled-manually` (with a note), or `wont-fix`

Re-running a stage resolves its issues that no longer occur. "Handled manually" survives re-runs.

## Stages

Each stage is a CLI command and a library function. Each reads the project store, writes the project store, and emits progress events.

1. **Analyse** (`porter analyse`). Read-only. Using every configured connector, it inventories:
   - platform and plugins/builders detected
   - content types and counts
   - the URL tree
   - media count and total size
   - redirects found
   - forms, menus and languages
   - what can't be converted (`converter-missing` issues)

   It also produces a **coverage** section listing which connectors ran, what each contributed, and what's unknown because of missing access. Where several connectors see the same item, it merges them by URL (and by ID where both have one). Mismatches are reported, e.g. "47 published pages the crawl can't reach" and "12 live URLs with no content behind them". The output is the project store plus `reports/analysis.html`.
2. **Extract** (`porter extract`). Fetches everything into `raw/` and `media/`: all content, metadata, every media file, menus, forms, redirects, and the rendered HTML of every page. Resumable: it skips what's already fetched unless `--refresh` is set. It writes `reports/extract.html`.
3. **Normalise** (`porter normalise`). Converts `raw/` into the neutral model using the source adapter and its builder converters. It resolves internal links and media references to model refs, collapses redirects and builds the route table. It is pure (no network), so it is fast to re-run.
4. **Prepare destination** (`porter schema <mode>`). Works out what the destination must hold, from the normalised model plus the mapping. Modes:
   - `check`: read the destination's existing schema and report every mismatch as a `schema-mismatch` issue. For Payload, porter reads its collection/global TS configs statically with the existing swc parser; for Sanity, the schema files; for WordPress, the registered types via REST.
   - `generate`: write schema files for the destination (Payload collection configs, Sanity schema types).
   - `prompt`: write a self-contained prompt that tells an AI agent to create the schema, with the required structure, sample data and the check command to verify against.
   - `spec`: write a human-readable spec (HTML and Markdown) of the required structure.
5. **Load** (`porter load`). Writes into the destination through the destination adapter, keyed by source identity, so a re-run upserts. Order: media, then taxonomies, then documents (parents first, two passes: create everything, then fill references), then menus, then forms, then redirects. `--dry-run` is the default, and `--apply` writes.
6. **Verify** (`porter verify`). Runs after load and checks the result against the model. It checks counts per type, then for every loaded document that:
   - its parent matches
   - every internal link and reference points at the item it pointed at in the source
   - every embedded media ref resolves to an uploaded file
   - every old URL reaches a 200 directly or via one redirect hop to a 200 (the Landmark `urls.test.ts` check, generalised)

   The URL check needs the new site's frontend, because whether `/rooms/foo/` returns 200 depends on the frontend's routing, not on the CMS. If `verify.site_url` is set (a local dev server or staging), porter requests every old URL against it. If not, porter checks routes inside the model only (each old URL maps to a loaded document via path patterns, or to a redirect), and records an `access-gap` issue saying the live check was skipped.

   Failures become issues. This is the main guard against the "wired up wrong" anti-goal.

`porter run` runs stages in order and stops at the first stage that ends with error-severity issues, unless `--continue` is set. `porter status` summarises the project. `porter issues` lists and filters issues and marks resolutions. `porter report` regenerates all HTML reports.

## Adapters

### Connectors (how porter reaches a source)

A source adapter uses one or more connectors. Each is optional. All are declared in `porter.toml`, with credentials referenced by environment variable or keychain name.

- **Public site** (always available): crawls `sitemap.xml`, `robots.txt` and links within the domain, with configurable concurrency, politeness and URL include/exclude patterns. It records status, redirect hops, title, meta, OG tags, canonical, hreflang and the rendered HTML, and fetches media referenced from pages. Platform detection uses generator tags, asset paths and markup signatures.
- **Admin API**: platform REST/GraphQL APIs with credentials (WordPress REST with application passwords; the Sanity, Webflow and Squarespace APIs).
- **Export file**: WordPress WXR, Squarespace's WXR-style export, Webflow CSV exports, Sanity NDJSON dataset exports.
- **Database dump** (WordPress): load the SQL dump into a disposable MariaDB container and run porter's bundled PHP exporter inside WordPress via the `wordpress:cli` image, as Landmark's `extract/export.php` did. This gives custom-field values under their real names (ACF `get_fields` unformatted), real permalinks including private posts, menus, Gravity/Ninja form definitions and submissions, and plugin tables (Redirection, Yoast, Smart Slider). It needs Docker and the site's theme and plugins (from server access, or a zip). If they're missing, porter reports an `access-gap` and falls back to reading tables directly with reduced fidelity.
- **Server access** (SSH/SFTP): fetches uploads, themes and plugins directly, and can pull a DB dump.
- **Source repository** (Gatsby and other static generators): reads markdown/MDX and frontmatter from a local checkout.

### Sources (build order)

1. **WordPress**. The converters for the content inside it are separate:
   - a shortcode parser following WordPress's own rules (port Landmark's `lib/shortcodes.ts`)
   - Divi (port `divi-to-blocks.ts`, including global-module expansion and hidden-module skip)
   - Gutenberg block comments
   - classic editor (wpautop and wptexturize, port `texturize.ts`)
   - Elementor (`_elementor_data` JSON)
   - WPBakery

   Yoast and Rank Math SEO come with variable substitution, ACF goes into typed fields, WPML/Polylang into locales, Gravity/Ninja forms into forms, and Redirection into redirects.
2. **Public-site-only (generic)**. Turns crawled HTML into documents. It strips header, footer and nav using signatures repeated across pages, detects main content, and turns that content into layout blocks and rich text. Fidelity is lower, and that is recorded as `partial` statuses.
3. **Sanity**: documents, Portable Text, assets and references via the API or an NDJSON export.
4. **Umbraco**: the existing JSON export reader (`src/sources/umbraco.rs`, with its `_variants` locale handling) rebuilt as a source adapter on the new model, keeping its existing local-fixture test. It's built in the first phase alongside WordPress, because it already has users.
5. **Webflow**, **Squarespace** and **Gatsby**.

### Destinations (build order)

1. **Payload**, via its REST API with an API key. A Rust process can't use Payload's Node Local API, and REST works against local and remote instances alike.
   - Upsert uses a hidden `legacyId` field on every collection (`where[legacyId][equals]`). Schema `check` flags it if missing, and schema `generate` adds it.
   - Media is uploaded as multipart.
   - Rich text goes to Lexical (porting Landmark's `html-to-lexical.ts` knowledge, but from the neutral tree rather than HTML).
   - Layouts go to blocks through the mapping.
   - Redirects go to the redirects plugin's collection.
   - Known Payload pitfalls from Landmark are handled: drafts written so the main row isn't stale, `null` arrays omitted, link URLs encoded.
2. **Sanity**: Portable Text, asset uploads, references, with deterministic `_id`s from source identity.
3. **WordPress**: WordPress REST with application passwords. Gutenberg block markup first, other editors later. Media goes to the media library, plus menus and redirects (Redirection's REST API, when installed).

Every adapter declares its **capabilities** (what it can read or write: layouts, forms, submissions, locales, drafts and so on). Analyse and schema use them to raise `converter-missing` and `access-gap` issues *before* anything is attempted.

## Mapping (per-client decisions)

The `[mapping]` section of `porter.toml` holds the Landmark-style decisions:
- which source types go to which destination types (or are dropped, with a reason)
- field renames and transforms
- builder-module → block overrides
- status rules (private → draft)
- locale handling
- forms to retire

`porter schema generate` and the mapping are seeded from analysis with sensible defaults, so a typical project needs only edits, not authoring.

**Mapping onto schemas that already exist is the main case, not an edge case.** Client sites usually have hand-built collections and blocks before content arrives (Landmark merged WordPress `post` into `pages`, `events` into `meeting-rooms`, and Divi output into that site's own blocks). So the mapping can target arbitrary existing destination shapes:
- type → type, many-to-one allowed
- neutral field path → destination field path, with transforms
- neutral layout block (matched by kind and `original` builder module) → destination block slug, with field mapping
- the destination's upsert key field and its type (porter's own string ID by default, or an existing field such as a numeric `legacyId`)

`schema check` validates the mapping against the destination schema, and reports every unmapped neutral field and every required destination field with no source as an issue.

**URLs on the new site.** Each mapped type declares its new path pattern (e.g. `/rooms/{slug}/`, or `{parent.path}/{slug}/` for nested pages). Porter uses those patterns to build redirect targets and the new route table.

**Manual redirects** (rules that come from no source, such as Landmark's members area and payment pages) are listed in `[[redirects]]` in `porter.toml` and treated like any other redirect source.

A small fixed set of transforms is built in: date parse/format, string case, split/join, number parse, URL rewrite, regex replace, default value, and lookup table. There is no embedded scripting language (see Non-goals).

## CLI contract

- Commands: `init`, `analyse`, `extract`, `normalise`, `schema check|generate|prompt|spec`, `load`, `verify`, `run`, `status`, `issues`, `report`, `export`, `upgrade`.
- Every command accepts `--project <dir>` (default: current directory) and `--json`.
  - With `--json`, stdout carries only newline-delimited JSON events (progress, item results, issues, final summary). Human text and logs go to stderr.
  - Without `--json`, output is human text with a final summary.
- Never prompts unless stdin is a TTY *and* `--interactive` is set. Missing configuration is an error naming exactly what's missing.
- Exit codes:

  | Code | Meaning |
  |---|---|
  | 0 | Stage finished; no error-severity issues opened by this run |
  | 1 | Stage could not run (bad config, unreachable source, internal error) |
  | 2 | Stage finished with error-severity issues or failed items |
  | 3 | Interrupted; resumable |
- `porter clean` only removes paths inside the project folder that porter created (`raw/`, `media/`, `reports/`, `project.db`), never the folder itself or anything outside it. This replaces the current behaviour, which can delete the working directory.
- The rebuild ships as **porter 2.0**. The 1.x flags (`--source`, `--target`, `--collection`) are removed, not kept working, and the release notes and README carry a short 1.x → 2.0 upgrade note.
- `upgrade` keeps Homebrew support and adds a plain binary self-update (via `self_update` against GitHub releases) for servers and containers.

## Code structure

A Cargo workspace:

- `porter-model`: the neutral model types, items, statuses, issue kinds. No I/O.
- `porter-engine`:
  - the project store (SQLite + files)
  - stage orchestration and progress events (a `tokio` broadcast channel the UIs subscribe to)
  - connectors, source adapters and their builder converters, destination adapters
  - verification and HTML reports

  Each source and destination is a module behind a Cargo feature, all enabled by default.
- `porter-cli`: the `porter` binary, a thin layer over `porter-engine`.
- (later) `porter-tui`, `porter-desktop`, `porter-hub`, each depending on `porter-engine`.

The existing code is mostly replaced. It keeps:
- the swc-based TypeScript schema parser (`src/parser/`), for Payload schema `check`
- the WordPress REST connector's pagination and retry logic

Everything else in `src/` is superseded and deleted once its replacement lands. The library compiles with fresh dependency resolution: drop or upgrade the pinned `swc` 0.90-era crates, and move `reqwest` to 0.12 with rustls.

## Testing and acceptance

- Unit tests for every converter (shortcodes, each builder, rich text in each direction), using synthetic fixtures checked into the repo. Porting the Landmark Vitest cases (`migration/__tests__/`) gives a head start.
- A golden-file test per destination: a small neutral fixture loads into a throwaway local instance (Payload, and a WordPress in Docker) and verify passes, run in CI.
- **Acceptance: Landmark.** Pointed at Landmark's local DB dump, uploads and a **recorded crawl snapshot** of the old live site, porter must load into **Landmark's own Payload app schema** (its existing collections and blocks, via a `porter.toml` mapping), not a porter-generated one, because that is the real-world case. The site will be replaced at launch, so the snapshot is captured once and kept. All of it is kept local and gitignored, as the existing Umbraco fixture is. Porter must:
  - extract and normalise every published item Landmark's importer did, with per-type counts matching
  - load into a fresh database for that app and pass verify against its dev server: 2,865 media, 417/418 sitemap URLs resolving, and 248 redirects (with Landmark's hand-written rules supplied as `[[redirects]]` in config)
  - import media folders from upload directories, and forms with their past submissions
  - report everything Landmark logged as an issue (unmapped fields, unresolved media, Smart Slider layers skipped) as typed issues

  Outside the bar: geocoding neighbour pins, Divi CSS reproduction, and `clean-page.ts`'s page-template heuristics. Those were Landmark-specific.

  Landmark's `extract/export.php` hardcodes that site's post types and URL. Porter's bundled exporter is a new, generic one that discovers post types, taxonomies and ACF groups. It uses `export.php` as a reference, not as code to copy.
- CI runs on push and PR again, with clippy at warn level until the existing backlog is gone, then `-D warnings`.

## Left to the builder

- Exact crate choices for HTML parsing (`scraper`/`html5ever`), XML (`quick-xml`) and crawling.
- SQLite schema details and migrations.
- Event and JSON payload shapes, as long as they're documented and versioned.
- Crawl politeness defaults.
- Order of work inside each phase.
- How Payload's Lexical node shapes are generated (port from Landmark or emit from the schema).

## Calls made without explicit agreement (to check)

- SQLite as the project store, rather than plain JSON files.
- Payload loads through REST, not by generating a Node script to run the Local API. REST works remotely and keeps porter a single binary. The cost: Payload must be running and reachable during load.
- The DB-dump path needs Docker.
- Per-client logic is configuration plus built-in transforms only, with no scripting.
- Pixel-matching the old site's styling is out of scope; layout structure is carried, CSS is not.
- Build order: WordPress → Payload first (Landmark is the acceptance test), then public-site generic, Sanity both ways, WordPress as a destination, then Webflow, Squarespace, Gatsby.
- Mapping onto existing hand-built destination schemas is a first-class feature, and Landmark acceptance runs against Landmark's real app schema.
- Verify's URL check runs against a frontend URL when one is given, and otherwise falls back to an in-model check plus an `access-gap` issue.
- The Landmark acceptance bar includes media folders, forms and submissions, and manual redirects via config. It excludes geocoding, Divi CSS and `clean-page.ts`.
- Acceptance uses a recorded crawl snapshot of the old site, never a live crawl in CI.
- Store rows use UUIDv7 IDs and `updated_at`/`deleted_at` columns from the start, so the hub can sync rows later without a schema rewrite. The hub's sync design itself is deferred to the hub design.
- Implementation is planned in phases, one plan per phase. The first plan runs through Landmark acceptance (core + WordPress + Payload). Each later source or destination gets its own plan.
