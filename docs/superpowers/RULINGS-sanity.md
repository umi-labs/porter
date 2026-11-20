# Sanity source, tidy-ups and flow graph: rulings and backlog

Built 2026-11-20 on branch `sanity-source` (engine and CLI only: Parts A–C of `specs/2026-11-20-sanity-source-and-tidy-flow-design.md`). Isaac was offline; Claude made these calls. Each line says what it costs if it's wrong.

## For Isaac to check first
- **R16, what porter suggests merging.** Similarity uses each type's own field names, at 0.7, or 0.5 when the types reference each other. On Kirker's real data that suggests four merges: destinations (city, country, region pages), holidays (rail journeys, river cruises, tailor-made, cultural tours, exhibitions), people (artists, tour leaders, experts), and reviews with testimonials. Opera and hotel breaks stay separate.
- **R17, fields filled in after normalise.** Every adapter now adds a mapping entry for each field it finds that isn't in `porter.toml` yet. This fixed a critical bug where a Sanity move loaded only titles and slugs. WordPress and Umbraco projects will see new entries appear too.
- **R18, unmapped fields are never silent.** Data in a field with no entry blocks the load under an enabled tidy-up, and gives a counted warning otherwise. To leave a field out on purpose, use `drop = true`; don't delete the entry.
- **R9, page builder or array.** Decided per field across all documents. A field that holds two or more `_type`s becomes page-builder blocks; a field that only ever holds one becomes a Payload `array`.
- **R7, singletons.** A type is a singleton only when its one document's `_id` equals its type name. On Kirker that's `siteSettings`, `headerNavigation` and `homepage`.

## Ledger (verbatim)
## Pre-flight scan
| Tasks | Shared file/interface | Finding |
|---|---|---|
| T1/T2 | ConnectorConfig::SanityApi/SanityExport, SanitySource::connectors | consistent |
| T2/T4 | Export, AssetDoc; fixture asset urls | fixture urls are static but T4 needs mock CDN → see R1 |
| T3/T6 | Shape, asset_ref, pt_to_richtext, PtCx | consistent |
| T5/T6 | Media.focal_x/focal_y | consistent |
| T6/T7 | path rendering | T6 precedes T7's dereferencing → R2 |
| T6/T9 | submission detection | T6 consumes detection T9 defines → R3 |
| T6/T8 | Route items | consistent |
| T9/T10 | FieldStats/seed | consistent |
| T10/T11 | effective mapping read by schema generate | T11 rewires readers T10 touches; T11 runs after, fine |
| T11/T13 | reports/tidy.json | consistent |
| T11 | `DestSchema` type not defined anywhere | → R4 |
| T12/T14 | suggested id `destination-pages` | T14 depends on T12's naming heuristic; fine |
| T2 self | test expects CLI exit 1 inside an engine test | → R5 |
| other tasks self | tests vs code agree | ok |

- Ruling R1: fixture asset `url`s use the literal host `http://cdn.test`; T4 tests rewrite it to the wiremock URI when loading the fixture — keeps one fixture — cost if wrong: a test helper.
- Ruling R2: T6 writes paths with the existing token set (fallback `/{type}/{slug}/`); T7 switches sanity normalise to the dereferencing renderer — plan order — cost: T7 touches normalise.rs again.
- Ruling R3: T6 implements `detect_submissions(stats, min_docs)` (pub(crate), in sanity/analyse.rs created early) and singleton detection; T9 reuses them for analysis.json and seeding — spec needs submissions in normalise — cost: T9 smaller.
- Ruling R4: `DestSchema` = whatever destinations/payload/schema_check.rs already parses for an existing app's collections (alias or small wrapper); no new parser — spec says "schema check runs against it" — cost: a wrapper type.
- Ruling R5: T2's 401 test asserts the engine error text; the exit-code-1 check moves to T14's CLI e2e — engine tests can't see CLI exit codes — cost: none.

## Tasks
- Task 1: complete (commits 710404a..2b1210d, review clean). ⚠️ registry test iterates source_names() — confirmed (registry.rs). Parked minors: empty project_id/dataset/path not rejected by validate (→ Task 2 to reject empty), stub declares reads:Media.
- Task 2: complete (commits 2b1210d..28a5ec8, review clean). Parked minors: .tmp left on failed read_archive (export.rs:105-134); #[allow(dead_code)] on AssetDoc/Export.assets → remove in Task 4; stale raw rows not pruned on re-analyse → Task 6 drop_stale should cover canonical; PORTER_SANITY_API_BASE read per call + tokio mutex in tests (no injectable base).
- Task 3: minor (deferred): `\n` in span text not turned into Inline::Break (PT soft breaks are common — final review should weigh fixing before Kirker); consecutive blockquotes not merged; unknown styles and dangling mark keys silently ignored (honesty); thin edge-case tests; no strict RED run (tests+impl written together).
- Task 3: fix round 1/5 (1 addressed, 0 open — inline image without ref now warns, caption carried; commits 8343d68..c588bd6)
- Task 3: complete (commits 28a5ec8..c588bd6, review clean after 1 fix round)
- Task 4: fix round 1/5 (1 addressed, 0 open — filenames carry the real extension; commits ad09c3d..2b9e792)
- Task 4: complete (commits c588bd6..2b9e792, review clean after 1 fix round). Note: implementer added a `brochure` file field on fixture `homepage` (PDF was unreferenced). Minor (deferred): dotted stem `v1.2` → `v1.jpg`; FetchSummary.unreferenced* not asserted/consumed yet (Task 9 analysis should consume).
- Task 5: complete (commits 2b9e792..5fb84d5, review clean)
- Task 6 implemented 01bb509 (DONE_WITH_CONCERNS). Rulings on its concerns:
  - Ruling R6: fixture edits (full reference ids, hotspot x=0.3, holidayDetails.destination) accepted — they fixed fixture bugs, not behaviour — cost: none.
  - Ruling R7: singleton = exactly one published doc AND `_id == _type` (Kirker's real singletons: siteSettings, headerNavigation, homepage; blogCategories has a UUID and 1 doc and is NOT a singleton) — the UUID heuristic misfires on fixture ids — cost: a type with a custom singleton id isn't detected (seed edit by hand).
  - Ruling R8: types whose name contains '.' (plugin/system docs, e.g. Kirker's `media.tag` from sanity-plugin-media, `sanity.*`) are not content: skipped with one Info issue per type — spec says nothing dropped silently — cost: none.
  - Ruling R9: Builder vs array is decided per (doc type, field path) across ALL docs: ≥2 distinct `_type`s seen in that array field → Layout (page builder); exactly one `_type` → List of Object (Payload `array`). Kirker: flexibleContent → builder; includes (includesListItem), accordion items, cards, seasonPricing → arrays — "format that makes sense in Payload" — cost: a builder field that only ever used one block type becomes an array (acceptable; seen data decides).
  - Ruling R10: add `title_field: Option<String>` to `TypeMap` (spec Part A table names it) — small — cost: none.
  - Media conflict / crop issue kind UnmappedField: accepted (no better kind; message carries meaning).
  - Routes only for rendered configured patterns: accepted for now; Task 7/8 revisit with dereferencing + crawl.
- Task 6: review clean (no Critical/Important). Routes only for configured-pattern paths: confirmed (fallback isn't an old-site URL). Fix round 1 dispatched to implement R7–R10 + draft-count fix.
- Task 6: minor (deferred): seo_of last-key-wins / null clears; ogImage null → spurious MissingMedia; untested routes/redirects/media-conflict/missing-media/panic paths; redirect ids use raw `from` (/a vs /a/ dup); stale media details kept when asset unused this run; reference to a submission doc reported as dangling Error; UnmappedField overloaded (add details.code).
- Task 6: fix round 1/5 (5 addressed, 0 open — R7–R10 + draft counting; commits 01bb509..8c9ecf3). Array path scheme: `<docType>.<key>[<itemType>]...`. Minor (deferred): array_types depth counter off-by-one vs conv near MAX_DEPTH (harmless fallback to List); plugin docs still get raw analyse rows.
- Task 6: complete (commits 5fb84d5..8c9ecf3, review clean after 1 fix round)
- Task 7: complete (commits 8c9ecf3..e930e3e, review clean). ⚠️ render_all callers compile — confirmed (workspace tests green per implementer; controller cargo build -p porter-desktop OK). Minors in task-7-review.md (deferred). PathCx clones all Documents (ponytail note).
- Task 8: fix round 1/5 (1 addressed, 0 open — crawled URLs are canonical sanity routes; commits 4a94bca..95797bf)
- Task 8: complete (commits e930e3e..95797bf, review clean after 1 fix round). Minor (deferred): unmatched crawl routes keep old-domain URL while matched use site_url; `hit` not gated on routable; no Info without site_url; Info reuses UnresolvedLink.
- Task 9: fix round 1/5 (1 addressed, 0 open — Sanity seeds no URL patterns; commits 9be4ed8..a8d53ca)
- Task 9: complete (commits 95797bf..a8d53ca, review clean after 1 fix round). Seeding: sanity::seed_mapping called from session.rs Env::seed, no-op without raw/sanity/analysis.json; analysis.json gains block_fields. Minor (deferred): no true RED run; crawled URL equal to fallback /{type}/{slug}/ still matches (acceptable: genuine match).
- Ruling R11 (Task 10 live test): tests/payload-app's config is static and CI starts it once, so "generate into the app, migrate, load" can't happen mid-test. Instead: commit the Sanity fixture's generated collections into tests/payload-app/src/collections/porter/ (+ index), include them in the app config, and add a non-live golden test that `generate_into` output equals the committed files (bless with PORTER_BLESS=1). The live test (gated like the others on PORTER_PAYLOAD_TEST_URL/API_KEY) loads the Sanity fixture with --apply and asserts group/array/relationship/path. — keeps CI shape unchanged — cost: committed generated files must be re-blessed when generation changes.
- Task 10: review clean (spec ✅, quality approved). ⚠️ live test can't be verified from diff — implementer ran it 3× locally on Node 22; CI payload-golden will confirm on push. Ruling R12: printed catch-all must be optional `[[...path]]` so "/" (homepage) is served — fix round 1 dispatched.
- Task 10: minor (deferred): slug-collision regex may flag inline block slugs; unique `path` means duplicate paths fail at load (honest, but message quality?); Info says "mixed shapes" for single-shape json fallbacks; --into checks collection slugs only, not block slugs; local live tests clash on leftover media filenames (pre-existing); hb-1 fixture gained `highlights` array.
- Task 10: fix round 1/5 (1 addressed, 0 open — optional catch-all [[...path]] serves /; commits 130e20c..02f9919). Design doc updated by controller (docs only).
- Task 10: complete (commits a8d53ca..02f9919, review clean after 1 fix round)
- Task 11: review → 1 Important (discriminator overwrites a mapped field silently) → fix round 1 dispatched. Discriminator implemented as synthetic transform source `_tidy.discriminator`; `TypeMap.parent` added for load/verify copies. ⚠️ tidy.json shape vs Task 13: Task 13 owns consuming it — carry the RuleOutcome shape into its dispatch.
- Task 11: minor (deferred): parents in another collection only warn per item; `dropped` counts hand-written mapping drops too; merge-blocks drop still triggers load's "no destination field" warning; unlisted-field data not flagged (pre-existing behaviour).
- Task 11: fix round 1/5 (1 addressed, 0 open — discriminator clash is a config error, exit 1; commits c95213d..2064af1)
- Task 11: complete (commits 4c0e6bd..2064af1, review clean after 1 fix round)
- Ruling R13 (Task 12): `TypeSummary::from_store` reads normalised Documents only, so suggestions are appended after NORMALISE (not after analyse as the plan said), and `porter tidy suggest` exits 1 "run porter normalise first" when nothing is normalised — field stats don't exist earlier — cost: suggestions appear one stage later.
- Ruling R14 (Task 12/14): naming heuristic = kebab plural of the members' common camelCase suffix (regionPage/countryPage/cityPage → "pages"), else first member + "-merged"; ids not hard-coded to `destination-pages`; Task 14's e2e test reads the id from `porter tidy list --json` — a heuristic tuned to produce "destination-pages" would be overfitting — cost: less friendly default names (user can rename `into`).
- Ruling R15 (Task 12): exclusions — submission types (forms.submission_types or types mapped with to=None) and singletons (raw/sanity/analysis.json `singletons` when present, else types with count==1).
- Task 12 implemented bb992c1. Ruling R16: similarity on the type's own field names only (no seo/created/updated paths — they inflate every pair); 0.7 Jaccard generally, 0.5 when the pair is linked by a reference. Measured on real Kirker: destinations (city/country/region 0.53–0.67, linked) cluster; holidays (rail/river/tailor-made/cultural/exhibition ≥0.71) cluster; people (artist/tourLeader/expert) cluster; review/testimonial 0.71; opera/riverCruise 0.67 and hotelBreak/tailorMadeHoliday 0.42 unlinked → not merged — design's "referenced the same way" intent — cost: linked-but-different types at ≥0.5 may be suggested (they're disabled suggestions; a person decides).
- Task 12: complete (commits 2064af1..840c53a, review clean; R16 applied in 840c53a). Fixture yields `pages` (countryPage+regionPage, linked) and `card-carousels`; three-way destinations covered by synthetic Kirker-ratio test.
- Task 12: minor (deferred): R15 exclusions untested directly; empty normalise → spurious suggest warning; BlockStats same-module props dropped in `sorted`; no way to dismiss a suggestion (deleted rule re-suggested each normalise — consider `dismissed` list); acronym names split into letters; CLI `suggested` events untested; from_set / show duplication.
- Task 13: review → 1 Important (personal data could reach flow examples for contact-like doc types below the submission threshold) → fix round 1 dispatched (source-agnostic `contact_like` guard). ⚠️ hub upload of flow.json is Part D (out of this plan) — not a gap here.
- Task 13: minor (deferred): dry-run "skipped" counted as ok (intended); block counts inherit the doc's status; dropped counts stale until next load; plain-text alignment with long titles; 2 CLI tests not RED-first.
- Task 13: fix round 1/5 (1 addressed, 0 open — contact_like guard on all example paths; commits fae5f55..93f524f). Minor (deferred): no nested-only contact field test.
- Task 13: complete (commits 840c53a..93f524f, review clean after 1 fix round)
- Task 14: complete (commits 93f524f..a93f5b4, review clean). Minor (deferred): docs/sanity.md:54 calls the draft warning `unpublished-changes` but it's emitted as `unmapped-field`; submission_types must be added after analyse seeds [mapping] (documented).

## Final review (710404a..a93f5b4): NOT READY — C1, I1–I6 + must-fix minors. Rulings for the single fix wave:
- Ruling R17 (C1): after every normalise, top up `[mapping.types.<t>.fields]` for each mapped type with entries (`src = src`) for fields present in normalised docs but not mentioned at all (never change or remove existing entries, never re-add dropped ones), via MappingDoc (comments kept); applies to all adapters since the same seeding gap exists for WordPress/Umbraco — cost: WP/Umbraco projects gain field entries they didn't have (additive, visible in porter.toml).
- Ruling R18 (I1): data in a field with no mapping entry is never silent: for types covered by an enabled tidy rule → Error that blocks load (spec Part B accounting); for other types → one Warning `UnmappedField` per type+field with the count of docs holding data. With R17, this only fires after a person deletes an entry instead of setting `drop = true` — cost: noise if someone deletes entries on purpose (docs say use drop).
- Ruling R19 (I2): normalise detects duplicate `path`s per destination collection (effective mapping): fallback paths (`/{type}/{slug}/`) are invented, so later duplicates get `-2`, `-3` (stable order by id) with an Info issue; duplicates from a configured pattern → Error issue naming both documents (load would fail on the unique index) — cost: fallback URLs change shape for duplicates.
- Ruling R20 (I3): `schema generate --into` skips generating a collection that maps onto an existing app collection (reports "maps onto existing <slug>; run porter schema check for missing fields") instead of refusing everything; it refuses only when porter would generate a collection whose slug already exists unmapped-to-by-design; also check block slugs (deferred minor) — cost: none.
- Ruling R21 (I4): a suggested `into` may not equal any collection already targeted by a non-member type's mapping (or an app collection found by --into's scan when available); on clash use `<into>-merged` (+ -2…) — cost: less pretty default names.
- Ruling R22 (I5): fingerprints hash the EFFECTIVE mapping for schema/load only; normalise's fingerprint excludes `[[tidy]]`; disabled rules don't change any fingerprint — cost: none.
- Ruling R23 (I6): doc fix only — docs/sanity.md states `sanity-export` reads documents but images need the asset documents a `sanity-api` export includes (a CLI `sanity dataset export` archive's `_sanityAsset` images aren't read yet); Kirker uses sanity-api — cost: sanity-export without API access can't carry images yet.
- Ruling R24 (must-fix minor): Portable Text `\n` in span text → `Inline::Break` (1,055 spans in 372 Kirker docs).
- Ruling R25 (must-fix minor, docs): fix docs/sanity.md:54 warning name, :102 `porterGlobals` → `porterBlocks`, :136 claim to match R18.
- Ruling R26 (before Kirker): `porter tidy dismiss <id>` sets `dismissed = true` (and enabled = false) on the rule instead of deleting it; suggest keeps skipping its `from` set; flow omits dismissed rules — cost: dismissed rules stay in porter.toml.
- Fix wave: 22f88da..7f52ce0 (10 commits, R17–R26). Fixer concerns: R19 only in Sanity normalise (WP/Umbraco duplicates still surface at load); R21 can't see app collections at suggest time; R20 "on purpose" = mapped type name differs from app collection (skipped collection isn't served by the catch-all route); TUI e2e_analyse_wxr_fixture flaked 2× in full workspace runs — controller: passes alone on base 710404a ×3 and full porter-tui suite on HEAD ×3 (105/105), so a load-dependent flake, likely pre-existing; parked for the final message.
- Final re-review: all 10 ADDRESSED, no new Critical/Important. Minor (parked): unique_paths reads tidy/`to` but normalise fingerprint excludes them (pattern-path dupes then fail per-doc at load); --into-skipped collections not served by catch-all; hand-edited dismissed+enabled rule applies but hidden from flow.
