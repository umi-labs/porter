# Sanity source, tidy-ups and the flow view: design

2026-11-20. Status: written by Claude from the 2026-11-19/20 conversation with Isaac; not yet approved.

## Why

Isaac has been challenged to use porter to move **Kirker Holidays** (`~/Desktop/work/clients/kirker`: Sanity v3 Studio plus a Gatsby frontend) to **Payload CMS + Next.js**. It's a test of porter more than a client job: does porter's neutral model hold up for a source that isn't WordPress or Umbraco?

**Winning** means a full data migration into a Payload CMS site within one working day: every document, image, reference, draft and form submission, in a format that makes sense in Payload. The day starts once porter can read Sanity. It covers scaffold, analyse, extract, normalise, schema, load and verify, ending with a running site. The Gatsby frontend isn't ported. The site only has to show the data is there and reachable at its old URLs.

Isaac wants porter to be able to **tidy** the structure on the way (merge near-identical types and blocks), but only if:

- tidying is **optional**: a straight move that mirrors Sanity is always available, and each tidy-up is switched on one at a time
- setting it up is **trivial**: porter suggests tidy-ups; nobody writes them by hand
- what happens to the data is **visible as a flow diagram** that non-technical stakeholders can look at and informally sign off. The visual reference is the workflow canvas in `~/Desktop/personal/dockyard/internal/lead-engine` (`apps/web/components/flow/flow-canvas.tsx`, `run-view.tsx`).
- the desktop app has a **dry run / simulation**: switch tidy-ups on and off and watch the counts change before anything is written

## Goals

1. A `sanity` source adapter in porter-engine, with CLI support, that reads a Sanity dataset in full: documents, drafts, references, Portable Text, page-builder arrays, images (with hotspot and crop), files, and documents that are really form submissions.
2. Porter-generated Payload collections and blocks dropped into a repo scaffolded by `foundry create`, without porter editing the app's own config.
3. Tidy-up rules: optional, suggested by porter, accounted for (nothing silently lost), and explained in plain language.
4. A flow graph produced by the engine, drawn by the hub (React Flow, read-only, shareable) and natively by the desktop app (GPUI, with toggles and a dry run).
5. Kirker migrated end to end, with `porter verify` green.

## Non-goals

- Porting the Gatsby frontend, or matching its look.
- A recorded sign-off. Sign-off is an informal look for now: no approvals table, no load gate.
- Sanity as a destination; live or incremental sync.
- Executing the Sanity Studio's JavaScript schema. Kirker's Sanity (3.20.1) is too old for `sanity schema extract`, and running a client's Studio code is out of bounds. Porter infers the schema from the data.
- A general-purpose transformation language. Tidy-ups are a small fixed set of rule kinds.

## Anti-goals (failure even if it technically works)

- A tidy-up that silently drops a field holding data, or puts content in the wrong place, or breaks a reference. This is the honesty rule's worst case.
- A tidy-up whose effect can't be explained to a non-technical person from the diagram alone.
- Setup that needs hand-written TOML for anything porter could have suggested.
- Personal data (form submissions) showing up in shared reports, flow examples or hub share links.

## Constraints

- Rust engine; the hub stays TypeScript (Next.js + Better Auth); the desktop app stays GPUI.
- The honesty rule: explicit per-item statuses, typed issues, truthful exit codes, `--json` NDJSON.
- Credentials only from environment variables, never in `porter.toml`.
- **Kirker's dataset** (`f489xjk7` / `production`), measured 2026-11-19:
  - 20,156 documents, 1,173 of them drafts
  - about 3,900 content documents in 20 types, plus 16,240 form submissions stored as documents (`newsletterForm` 7,337, `brochureForm` 5,818, `enquiryForm` 3,085)
  - 18,541 image assets and 2 file assets, **24.2 GB** in total. A single `sanity dataset export` tarball failed twice on connection resets.
  - 49 page-builder block types; about 6,000 Portable Text blocks, whose only annotation is `link`; about 6,900 references, 11 of them weak
  - one locale, no i18n plugin
- A `--no-assets` export strips image asset references entirely. It can't be the source of truth for images.
- **`foundry create`** (branch `feat/cli-create` in the foundry repo, `packages/cli/templates/payload-site`) scaffolds a Payload app with `Pages`, `Media` and `Users` collections, optional plugins (SEO, media, forms, redirects, menus...) and `db.push: false`. Schema changes therefore go through `payload migrate:create`.
- The URL rules live in Gatsby (`gatsby-node.mjs` `getSlug()`), not in Sanity. For example: `/hotel/{slug}`, `/holidays/{slug}`, `/destinations/{europe|further-afield}/{country}/{city}`, `/about/news/{slug}`.

## Approaches considered

### Drawing the flow (Isaac picked 2)

1. **One web page everywhere.** A React Flow canvas inside porter's shareable HTML report. The hub serves it through share links, and the desktop app opens it in the browser. One renderer, but you can't toggle tidy-ups from the canvas, and the canvas isn't inside the desktop app.
2. **React Flow on the hub, a native GPUI canvas in the desktop app, both drawing one graph the engine produces.** Chosen: the desktop's dry run works best when the canvas is the thing you click, and a single engine-made graph keeps the two renderers from drifting. The cost is two renderers. The desktop one is simple because the columns are fixed (sources → tidy-ups → destinations), so no free-form layout is needed.
3. **Embed a web view in the desktop app (`wry`).** One renderer, but GPUI has no supported way to host a web view, and GPUI is already pre-1.0.

### Reading Sanity (Claude's call)

1. **The Sanity HTTP export API** (`GET https://<project>.api.sanity.io/v2021-06-07/data/export/<dataset>`), with a read token from an env var. It streams NDJSON with asset references intact, and the `sanity.imageAsset`/`sanity.fileAsset` documents give each asset's CDN URL, size and SHA-1. Assets are then fetched one by one from the CDN.
2. **A `sanity dataset export` tarball** someone hands over. Supported as a second connector, because clients send exports, but it isn't the default: at Kirker's size the tarball doesn't survive a flaky connection, and without assets it has no image references.
3. **GROQ queries per type.** More requests, and nothing the export endpoint doesn't already give.

Chosen: 1 as the default connector (`sanity-api`), 2 as an alternative (`sanity-export`). No Node or Sanity CLI is needed at run time.

## Part A: the Sanity source

### Connectors

```toml
[source]
adapter = "sanity"

[[connectors]]
kind = "sanity-api"
project_id = "f489xjk7"
dataset = "production"
token_env = "PORTER_SANITY_TOKEN"   # default; a read token (Viewer role)
# assets = "referenced"             # default; "all" also fetches unreferenced library assets

# or, from a file someone sent:
# [[connectors]]
# kind = "sanity-export"
# path = "exports/production.tar.gz"   # or a folder holding data.ndjson (+ images/, files/)
```

- **Extract** saves the exported NDJSON to `raw/sanity/data.ndjson` and writes one raw item per document (`sanity-api` or `sanity-export`, source id = the Sanity `_id`, drafts keeping their `drafts.` prefix).
- **Assets** come from the `sanity.imageAsset` / `sanity.fileAsset` documents in the same export: `url`, `size`, `mimeType`, `sha1hash`, `metadata.dimensions`, `originalFilename`, `altText`.
  - By default only **referenced** assets are fetched. Unreferenced ones are counted (number and bytes) in analysis, and `assets = "all"` fetches them too.
  - Assets are downloaded individually with bounded concurrency, retries with backoff on network errors and 5xx, and a per-file size cap.
  - Sanity's `sha1hash` is the hash of the file, so an asset already in porter's content-addressed `media/` store with a matching hash is skipped. **A re-run resumes** instead of starting over. Each download is verified against `sha1hash`, and a mismatch becomes an issue.
  - Kirker's 24 GB fetch is the slowest step of the day. Extract reports progress events, and an interrupted extract continues on the next run.

### Analyse

Analysis reports, from the data alone:

- **Document types**, with counts (published, draft-only, and drafts of published documents), and field statistics inferred from values (the existing `TypeSummary`/`FieldStats`).
- **Value shapes**, classified as:
  - reference
  - image or file
  - Portable Text array
  - page-builder array (an array of objects each carrying `_type`, which isn't Portable Text)
  - `slug`, `geopoint`, datetime string, plain object, scalar
- **Block types**: every `_type` found inside page-builder arrays, with counts and field statistics (`BlockStats`).
- **The reference graph**: type → type edges with counts, weak references, and dangling references (target missing).
- **Probable form submissions**: types with no slug, many documents and contact-like fields (email, name, phone, message). They're reported for the mapping to confirm. Kirker's three `*Form` types land here.
- **Singletons**: types with exactly one document and a fixed id (`siteSettings`, `homepage`, `headerNavigation`, ...), which become globals.
- **Assets**: referenced and unreferenced counts and bytes.
- **Coverage**: the export reveals all content, drafts and assets. It doesn't reveal the Studio's field titles, validation or option lists, or the frontend's URL rules. Those are listed as unknowns.

Analysis seeds `[mapping]` as a straight move: one collection per document type, one block per block type, singletons as globals, and probable submissions as forms.

### Normalise: Sanity → neutral model

| Sanity | Neutral model |
|---|---|
| document | `Document`. Id from the `_id` without `drafts.`; `doc_type` = `_type`; `created`/`updated` from `_createdAt`/`_updatedAt` |
| published document | `DocStatus::Published` |
| draft only (`drafts.x` with no `x`) | `DocStatus::Draft`; loads as a Payload draft |
| draft of a published document | Not loaded. One `unpublished-changes` warning per document, listing it, so a person can publish or discard it in Sanity before the final run |
| `slug` object | `Document.slug` (`slug.current`) |
| title | First of `title`, `name`, `heading`, `label` (overridable per type with `title_field`) |
| `{_ref, _type: "reference"}` | `FieldValue::Doc(<target id>)`. Missing target: an error issue (`dangling-reference`), or a warning when `_weak` |
| image object | `FieldValue::Media(<asset id>)`. The usage keeps `hotspot` (→ Payload `focalX`/`focalY`), `crop` and the image's own fields (`alt`, `caption`) |
| file object | `FieldValue::Media` |
| Portable Text array | `FieldValue::RichText`: `normal`/`h1`–`h6`/`blockquote` styles, `bullet`/`number` lists (with nesting by `level`), marks `strong`/`em`/`underline`/`code`/`strike-through`, `link` annotations (internal links resolved to `LinkTarget::Doc`), inline images (`RtBlock::Media`). Any other inline object becomes a `converter-missing` warning with its raw JSON kept |
| page-builder array | `FieldValue::Layout(Vec<Block>)`. One `Block` per item; `Original { builder: "sanity", module: _type, settings: item }`; its fields converted recursively into `props`; `kind` = the closest generic kind when it's obvious (`imageItem` → Image, `video` → Video, `separator` → Divider, `accordion` → Accordion), else `Raw` with the module name carrying the meaning |
| `geopoint` | `Object { lat, lng }` |
| datetime / date string | `Date` when it parses (RFC 3339 or `YYYY-MM-DD`), else `Text` |
| other object | `Object`, recursively; `_key` dropped; `_type` kept inside arrays as the discriminator |
| SEO object (`seoGroup`) | `Document.seo` through a field map seeded from the field names found |
| form-submission document | `Submission` under a `Form` named after the type; fields kept as submitted |
| singleton | settings for a global |

Unknown shapes are kept as JSON text with a warning. Nothing is dropped without an issue saying so.

### URLs and routes

Mapping path patterns gain **dereferencing**. A token can follow references, and a `lookup` maps values:

```toml
[mapping.types.cityPage]
path = "/destinations/{country.region|lookup:regions}/{country.slug}/{slug}"

[mapping.lookups.regions]
europe = "europe"
"further-afield" = "further-afield"
```

The exact Kirker patterns are transcribed from `gatsby-node.mjs` `getSlug()` during the Kirker run. They're config, not code. Verify checks every old URL against the new site, as it already does.

### Schema into a `foundry create` repo

`porter schema generate --into <repo>` writes:

- `src/collections/porter/<Name>.ts` and `src/blocks/porter/<Name>.ts`
- `src/collections/porter/index.ts`, exporting `porterCollections` and `porterGlobals`

Porter never edits `payload.config.ts`. The person adds one import and spreads the arrays (the command prints the two lines), then runs `payload migrate:create`.

A generated collection whose slug already exists in the app (Kirker's `pages` against the template's `Pages`) is refused, with the two ways out spelled out:

- map onto the existing collection, then add the missing fields from `porter schema check`'s list or hand `porter schema prompt` to an agent
- rename the generated collection with `to = "..."`

Without `--into`, output stays in the project's `generated/payload/` as today.

## Part B: tidy-ups

### Rules

Tidy-ups are rules in `porter.toml`, applied by the mapping layer. Normalise output stays a faithful copy of the source. The rules change where things go in schema generate and load, not what the source said.

```toml
[[tidy]]
id = "destinations"
kind = "merge-types"
enabled = true
from = ["regionPage", "countryPage", "cityPage"]
into = "destinations"
discriminator = { field = "kind", values = { regionPage = "region", countryPage = "country", cityPage = "city" } }
parent = { cityPage = "country", countryPage = "region" }   # parent from a reference field
# fields present in only some types are kept, optional
# drop = { "cityPage.addAnchor" = "unused layout flag" }   # explicit, with a reason

[[tidy]]
id = "card-carousels"
kind = "merge-blocks"
enabled = false
from = ["cardCarousel", "customCardCarousel"]
into = "card-carousel"
fields = { "customCardCarousel.cards" = "items" }          # align differing names
```

Version 1 has two rule kinds:

- **`merge-types`**: several document types go into one collection, with a discriminator field. Optionally a type's parent is set from one of its reference fields, so routes and the admin tree follow it. The merged collection's fields are the union; a field missing from some types is optional.
- **`merge-blocks`**: several block types go into one block, with field-name alignment.

More kinds are added only when a real migration needs them.

### Accounting (the honesty rule)

For every enabled rule, every field of every input item is either **carried** (same name), **renamed** (listed), or **dropped** (listed, with a reason). A field that holds data and isn't accounted for is an **error issue** that blocks load. Each rule records per-item outcomes, so the flow can say "467 in · 467 out · 0 dropped". Merging two fields of different shapes into one name is an error, not a coercion.

### Suggestions

Analysis (and `porter tidy suggest`) proposes rules by comparing shapes:

- **Document types** whose field-name sets overlap by at least 70% (Jaccard similarity), and that are referenced the same way, become a `merge-types` suggestion. If the types reference each other, a `parent` is suggested too.
- **Block types** with the same overlap threshold become a `merge-blocks` suggestion.

Suggestions are written to `porter.toml` **disabled**, each with a plain-English `label` and a `why` ("City, Country and Region pages share 14 of 17 fields"). Turning one on is `porter tidy enable <id>`, or a toggle in the desktop app. Nobody writes a rule by hand unless they want to.

## Part C: the flow graph

The engine builds one graph, and both front ends draw it.

`porter flow [--json]` prints it. Normalise and every dry-run or applied load also write it to `reports/flow.json`. It contains:

- **Nodes**: `{ id, column: "source" | "tidy" | "destination", kind, title, summary, enabled, counts { in, ok, skipped, failed, dropped }, example? }`
  - `summary` is engine-written plain English ("Merges City, Country and Region pages into Destinations. Each keeps what it was in Kind.").
  - `example` is one real item before and after (its title and the fields that changed). Form submissions **never** have an example; they show counts only.
- **Edges**: `{ from, to, count }`. Edges come from the mapping and the rules. Counts come from the last normalise and load in the store. A graph built before any dry run says so (`counts: null`) rather than showing zeros.

One source node is drawn per type and block type, one tidy node per rule (disabled rules included, drawn dashed with "passes straight through"), and one destination node per collection, global and block.

## Part D: hub flow page

- A **Flow** tab on the project page and on share links, drawn with `@xyflow/react` following the lead engine's `FlowCanvas`:
  - a 230px card per node: a group dot and label (Source / Tidy-up / Destination), the bold title, and a footer of count chips
  - disabled nodes dashed and faded
  - a fit-to-view, read-only canvas
- Clicking a card opens a side panel with the engine's summary and the before/after example.
- `porter push` uploads `reports/flow.json` with the version it already pushes, and the hub stores it beside the stage views.
- It's read-only: there's no sign-off button, and anyone who can see the project or its share link can see the flow.

## Part E: desktop flow view

- A **Flow** section in the desktop app, drawn natively in GPUI:
  - three fixed columns, the same card anatomy as the hub, and curved edges
  - the canvas pans and zooms, but there's no free-form dragging
- Each tidy card has an on/off switch. Switching it writes `porter.toml` (through the existing mapping editor, keeping comments) and starts a dry run (normalise if stale, then `load` dry run). The cards show the new counts when it finishes.
- Clicking a card shows the same summary and example as the hub. Suggested rules appear as disabled cards, so switching one on is accepting the suggestion.

## Order of work

1. **Part A** in the engine and CLI. Kirker can then do a straight move.
2. **Parts B and C** in the engine and CLI: rules, accounting, suggestions, the `porter tidy` and `porter flow` commands.
3. **Part D**: the hub.
4. **Part E**: the desktop app.
5. **The Kirker run**, timed from `foundry create` to `porter verify`.

Isaac asked for 1 and 2 to start straight away (2026-11-20); 3 and 4 wait until he has read this design.

## Testing

- **A fixture** in `test-data/sanity/`: a small hand-made NDJSON in Kirker's shapes (draft-only, draft-of-published, a weak and a dangling reference, Portable Text with a link and an inline image, a page-builder array, a geopoint, a singleton, three near-identical page types, and a synthetic form type). No real Kirker data or personal data is committed.
- **The `sanity-api` connector** is tested against a local mock HTTP server (export stream, asset CDN with one failure then success, and a SHA-1 mismatch).
- **Normalise golden output** for the fixture.
- **Schema generate golden output**, and the existing `payload-golden` CI pattern extended with a Sanity fixture loaded into a test Payload app.
- **Tidy accounting** tests: an unaccounted field blocks load, a rename is carried, and a field-shape conflict is an error.
- **Flow graph** tests on the fixture, with and without a dry run.
- **The Kirker acceptance run** happens locally (its data stays outside the repo, in `~/Desktop/work/clients/kirker-sanity-export/`).

## Left to the builder

- Concurrency and retry numbers for asset downloads; the progress event cadence.
- The exact similarity threshold, tuned on the fixture and Kirker (start at 0.7).
- How the flow graph positions nodes within a column (order by count, then name, is fine).
- Exact NDJSON event names, following `events.rs` conventions.

## Builder check: decisions

A fresh reviewer read this design as a builder would and raised six questions. Isaac was offline and had asked for the engine and CLI build to start, so Claude decided each one. Each is marked **(Claude's call, Isaac to confirm)** where it changes what gets built.

1. **Serving old URLs on the new site (Claude's call, Isaac to confirm).**
   - Every generated collection gets a `path` text field (indexed, unique per collection), which load fills from the mapping's path pattern.
   - Porter still writes no frontend code. `porter schema generate --into` *prints* a one-file catch-all Next route (`app/(frontend)/[...path]/page.tsx`) that looks a path up across the generated collections and renders title, fields and rich text plainly. Pasting it is part of the Kirker day; it's minutes, not hours.
   - Old URLs aren't redirected to template routes.
2. **Where the old URLs come from (Claude's call).**
   - A Sanity project may also have a `crawl` connector (the existing public-site connector) pointed at the live site's sitemap.
   - Its URLs are merged with the pattern-made routes by URL. Analysis reports old-site URLs that no pattern produced (warning) and pattern URLs the old site doesn't have (info).
   - Verify checks the union, so a mistake in a copied `getSlug()` pattern shows up instead of passing.
3. **Per-use image details (Claude's call, Isaac to confirm).**
   - No model change.
   - Hotspot becomes the media item's focal point (Payload `focalX`/`focalY`), taken from the first use. Uses that disagree raise one `media-use-conflict` warning per asset, listing them.
   - `alt` and `caption` follow the same rule, and Sanity's asset-level `altText` wins when present.
   - `crop` isn't carried (Payload has no equivalent). Each cropped use raises an info issue, so someone can re-crop by hand if it matters.
   - Upgrading every image field to `{image, alt, caption, focal}` groups is a later, all-adapter change if this proves too lossy.
4. **Tidy rules and the existing mapping (Claude's call).**
   - An enabled rule is **compiled into** the from-types' `[mapping.types.X]` entries: `to = into`, plus the discriminator field set to a constant and `parent` from the named reference field.
   - Hand edits on a from-type (`fields` renames, `path`, `status`) still apply on top.
   - `into` may name an existing destination collection or block. Then `porter schema check` runs against it, and **a carried field the destination lacks is the "unaccounted" error that blocks load**, as is a shape conflict between merged fields.
   - Against a porter-generated destination the union schema is generated, so the only errors are shape conflicts and listed drops.
   - "Dropped" in the flow counts listed drops only.
5. **Singletons (Claude's call).**
   - Version 1 loads each singleton as a document in its own one-document collection, so there are no model changes.
   - `homepage` is mapped to the `pages` collection at path `/` through config (`[mapping.types.homepage] to = "pages", path = "/"`), so `/` has a document behind it.
   - Real Payload globals for singletons with arbitrary fields are a later change.
6. **Form submissions (Claude's call).**
   - Porter synthesises one `Form` per submission type, with fields from the union of submitted field names. The Payload form-builder plugin (the scaffold's `form-plugin` option) is required, and `porter schema check` says so when it's missing.
   - Nested objects in a submission (`addressFields`, `holidayDetails`, `leadPassengerDetails`) are flattened to dotted keys (`holidayDetails.destination`). That's lossless: Kirker's submissions hold only scalars and objects, with no references.

## Calls Claude made (for Isaac to check)

- The HTTP export API with a read token is the default way in, not the Sanity CLI tarball (because of the 24 GB of assets and the flaky downloads).
- Only referenced assets are fetched by default.
- A draft of a published document isn't loaded. It's reported as `unpublished-changes`, so someone decides in Sanity.
- The 16,240 form submissions are loaded (as "full" asked), but never appear in examples or share links.
- Porter writes generated schema into `src/collections/porter/` and `src/blocks/porter/` and never edits `payload.config.ts`.
- Tidy-ups are mapping-layer rules. Normalise output stays a faithful copy of Sanity.
- Version 1 has two rule kinds only: `merge-types` and `merge-blocks`.
