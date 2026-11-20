# Porter 2.0 roadmap: everything agreed so far

Last updated 2026-11-11. This is the single place to pick work back up. The detail lives in the linked documents; this file makes sure nothing discussed gets lost.

- Engine design (approved): `docs/superpowers/specs/2026-11-11-porter-engine-design.md`
- Engine phase 1 plan: `docs/superpowers/plans/2026-11-11-porter-engine-phase-1.md`

## Where we are

| Step | Status |
|---|---|
| Audit of porter 1.x | Done (findings below) |
| 1.x fixes: seed `_status`, duplicate titles, credentials, debug flag, seed test | Done: commit `b2215fd`, on `fix/audit-findings`, not pushed or released |
| 1.x `porter clean` deleting the working directory | **Not fixed.** Offered, not yet done. Dangerous on the current Homebrew release. |
| Engine design | Approved |
| Engine phase 1 | Built and reviewed on branch `porter-2.0` (RULINGS-engine-phase-1.md) |
| Terminal UI | Built and reviewed (RULINGS-tui.md); real-terminal checklist pending |
| Desktop app (GPUI) | Built and reviewed (RULINGS-desktop.md); smoke checklist needs a display |
| Hub (Next.js + Better Auth, Docker/Dokploy) | Built and reviewed; production compose smoke passed locally; not deployed (RULINGS-hub.md: decisions D2–D10 open) |
| Design (Isaac's Claude Design boards) | Applied to desktop and hub, plus engine stage views and config fingerprints (RULINGS-design.md) |
| Merge `porter-2.0` / release 2.0 | Not done; Isaac's call |
| Porter 2.0 release | **Released 2026-11-19**: v2.0.0 on GitHub + Homebrew (`umi-labs/tap/porter`), desktop DMGs + cask `porter-desktop` (unsigned, unnotarised) |
| Sanity source + tidy-ups + flow graph (engine + CLI) | Built and reviewed 2026-11-20 on local branch `sanity-source`, not merged or pushed (RULINGS-sanity.md). Hub Flow tab and desktop Flow view not started |
| Later engine phases (other sources/destinations) | Not planned yet; listed below |

## The vision (in Isaac's words, condensed)

Porter is Umi's migration tool. It runs as a CLI, as a backend process on a server, and inside agent skills when migrating client sites. A migration goes:

1. **Analyse** the old site (structure, content, media, routing, redirects) and show it in an output people can see.
2. **Extract** everything (metadata, every media file, page layouts, content, routing) and display it so people can understand it.
3. **Normalise** into a neutral format that is *not* Payload-shaped, so it can become many formats and platforms.
4. **Prepare the destination's schemas** in one of four ways: check existing ones, have porter write them, hand a prompt to an agent, or give a human the required structure.
5. **Load** everything into the new storage.

The one rule that matters most: **porter is honest.** It never fails quietly, says what it can't do, and every failure can be handled by a re-run or by a person. The worst outcome is "got all the content but messed up where it went or how it's connected."

## Decisions

### Engine
- Stays in **Rust**. The TypeScript Landmark pipeline (`~/Desktop/work/clients/landmark/migration/`) is the knowledge source, not the codebase.
- Ships as **porter 2.0**, with a clean break: the 1.x flags are removed and `UPGRADING.md` explains the changes.
- **Umbraco stays** as a source, rebuilt on the new model in phase 1.
- One **project folder** per migration: `porter.toml`, `project.db` (SQLite), `raw/`, `media/` (content-addressed), `reports/`.
- Neutral model: documents, rich text tree, layout block tree (page-builder neutral), media with URL variants and usages, routes, redirects, menus, taxonomies, forms, submissions, settings, authors.
- Every item gets an explicit status per stage. Issues are typed and can be marked `fixed-by-rerun`, `handled-manually` or `wont-fix`.
- Exit codes 0/1/2/3, `--json` NDJSON on stdout, no prompts unless TTY plus `--interactive`.
- Access varies per project (public site only, admin login, DB dump, server access, any mix), so porter works with what it gets and reports **coverage** (what each access revealed and what's unknown).
- Credentials come from env vars or the keychain, never from `porter.toml`.
- Mapping onto **existing hand-built schemas** is a main feature, not an edge case.
- Payload loads through its **REST API** (not the Node Local API), keyed by a `legacyId` field.
- The DB-dump connector uses **Docker** (MariaDB plus `wordpress:cli`) running a generic bundled PHP exporter.
- Per-client logic is **config plus built-in transforms**, with no scripting language.
- Pixel-matching the old site's CSS is **out of scope**. Layout structure is carried.
- Verify checks counts, parents, links, media and every old URL (against the new site's frontend when a URL is given).
- Store rows are hub-sync-ready from day one (UUIDv7, `updated_at`/`deleted_at`).
- **Landmark** is the acceptance test. It loads into Landmark's real app schema, from a recorded crawl snapshot of the old site, captured before launch replaces it.

### Interfaces
- **Terminal UI**: modelled on **lazygit / lazydocker**, built with `ratatui`. It replaces the current dialoguer prompts.
- **Desktop app**: a native Rust app built on **GPUI** (Zed's UI framework, Apache-2.0, on crates.io), probably with `gpui-component` for widgets. GPUI is pre-1.0, so expect API churn.
- Both are front ends over `porter-engine`. They subscribe to its event stream and query its store; neither re-implements logic.
- Build order: **engine → terminal UI → desktop app → hub.**

### Hub
- A central server that local projects connect to.
- **Shareable links**: push analysis and extraction results, and people view them in a browser. Internal by default; clients can be given access.
- **Client uploads**: clients upload a DB dump, export file or media archive straight into a project.
- **Remote runs**: run stages on the server (e.g. `porter extract --remote`) and pull the results back.
- The local project folder stays the source of truth. Porter works fully offline without a hub.
- Logins, per-project access and a data retention policy are needed from day one (dumps and submissions hold personal data; Landmark had 57,159 submissions).
- Cheap first step, already in phase 1: self-contained HTML reports you can share as a single file.

## Sources and destinations

Sources, in build order:
1. WordPress, with the Divi, Gutenberg, classic and shortcodes converters (phase 1), then Elementor and WPBakery (later)
2. Umbraco (phase 1)
3. Generic public-site-only source for anything else, including custom page builders
4. Sanity
5. Webflow
6. Squarespace
7. Gatsby (repo/markdown or built site)

Destinations, in build order:
1. Payload (phase 1)
2. Sanity
3. WordPress (Gutenberg first, then other builders; this covers WordPress → WordPress builder-to-builder moves)

Combinations Isaac has done or expects: WordPress builder → another builder, WordPress → Sanity, Sanity → Payload, custom builders/Webflow/Squarespace → Payload, Sanity or WordPress, and Gatsby.

## Feature ideas captured (not yet scheduled)

- **Automated visual comparison** between old and new sites (screenshots at desktop and mobile widths, diffed). Landmark did this by hand with about 60 screenshot pairs.
- Sitemap / URL coverage check against the new site. *(In phase 1 as part of verify.)*
- **Generate the mapping from ACF field definitions.** *(Partly in phase 1: the mapping is seeded from analysis.)*
- WPML / Polylang → destination locales. *(In phase 1 for WordPress.)*
- Smart Slider import (phase 1 only detects it and reports `converter-missing`).
- **Incremental re-sync**: on launch day, re-import only content changed since the last dump, keeping anything imported after it (Landmark re-ran everything from a fresh dump in about 20 minutes).
- SSH/SFTP server-access connector to fetch uploads, themes and plugins, and pull a DB dump.
- Geocoding addresses (Landmark-specific so far; a possible built-in transform later).
- Release packaging: a clean database dump plus media archive, as Landmark's `dump.sh` produced.
- Agent skill integration: point the existing `payload-site-migration` skill at porter's headless commands and verify step.

## Lessons from Landmark to keep in mind

- Staging lagged production, so always extract from a fresh production dump, and expect a launch-day re-run.
- Parallel media uploads hit filename collisions; retry those one at a time.
- Payload pitfalls:
  - a `draft: true` write updated only the versions table and left the main row stale
  - `null` arrays are rejected
  - link URLs containing spaces are rejected
  - phone numbers lost their leading zero
- Many commits went into copying Divi's CSS, then partly undoing it for editor-friendly blocks. Hence the decision that porter carries structure, not styling.
- Manual follow-ups always exist (geocoding misses, data retention, payment forms, analytics triggers, credentials found in old theme code). Porter's issue list should be where those get tracked.

## Porter 1.x audit findings (for reference)

Fixed in `b2215fd`:
- invalid seed TypeScript (`_status`)
- duplicate titles overwriting each other
- plaintext credentials (now env vars, file mode 0600 and gitignored)
- `static mut` debug flag
- debug output on stdout

Not fixed in 1.x (all superseded by 2.0):
- `porter clean` can delete the working directory
- WXR parser returns 0 posts
- WordPress source never initialised from the CLI
- `generate`/`migrate` mapping format mismatch
- `--dry-run` exits 0 on failure
- no HTML → Lexical, media, relationships or redirects
- no streaming
- dead batch and plugin code
- CI disabled
- about 280 clippy warnings
- 10 dependency vulnerabilities through `reqwest 0.11`
- library unusable with fresh dependency resolution

## Isaac's instructions, 2026-11-11 evening
- Keep building without stopping: engine phase 1, then the terminal UI, the desktop app and the hub.
- With Isaac offline, Claude makes the design calls for the terminal UI, desktop app and hub itself and records them as rulings for review. Each still gets a written design and plan before code.
- **The hub is deployed with Docker on Dokploy.**
- **The hub uses Better Auth for authentication.** Better Auth is a TypeScript library, so the hub's web/auth layer is TypeScript. It drives porter through the headless CLI's `--json` event stream and the project folder, and the engine stays Rust. This replaces the engine design's `porter-hub` Rust crate idea, and is recorded as a ruling in the hub design.
- No pushes, deploys or releases without Isaac. All work stays on local branches.

## Open items

- [ ] Decide whether to fix and release `porter clean` in 1.x now, while 2.0 is built.
- [ ] Decide whether to open a PR for `b2215fd` (1.x fixes) separately from 2.0.
- [ ] Capture the Landmark old-site crawl snapshot **before Landmark launches** (phase 1 plan, Task 23 Step 1).
- [ ] Design conversation: terminal UI (lazygit/lazydocker reference, mockups).
- [ ] Design conversation: desktop app (GPUI, mockups).
- [ ] Design conversation: hub (sync model, access, uploads, remote runs, retention).
- [ ] Plan later engine phases: SSH connector, Elementor/WPBakery, public-site source, Sanity both ways, WordPress destination, Webflow, Squarespace, Gatsby.
