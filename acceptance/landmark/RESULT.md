# Landmark acceptance: result

Run on 2026-11-12 with `acceptance/run.sh landmark --local-only` (wp-db connector only). Counts
only. "Landmark" is what its importer produced from the same data (see README.md for how each
row is worked out); "porter" is after normalise.

## What ran

| Step | Run? |
| --- | --- |
| analyse, extract, normalise (wp-db connector, local Docker) | yes |
| `schema check` against the app's `src/` (offline) | yes: 0 errors, 22 warnings |
| crawl snapshot of the old site | **no**: needs the go-ahead to crawl the live site |
| load into the app on a fresh database (`porter run --apply`) | **no**: needs the go-ahead to run the app and create the database |
| verify (old URLs against the dev server; brief: at least 417 of 418) | **no**: needs the load |

## Counts

| | Landmark | porter | |
| --- | ---: | ---: | --- |
| page | 353 | 354 | one more, see note 1 |
| post (into pages) | 2 | 2 | |
| rooms | 15 | 15 | |
| events (into meeting-rooms) | 12 | 12 | |
| promotions | 102 | 102 | |
| bars | 1 | 1 | |
| guides | 1 | 1 | |
| blog (into posts) | 28 | 28 | |
| neighbour | 168 | 168 | |
| slides | 770 | 770 | |
| popups | 5 | 5 | |
| notifications | 1 | 1 | |
| media | 2,865 | 2,639 files for 2,865 attachments | see note 2 |
| media folders | 132 | 130 | see note 2 |
| redirects | 257 (brief: 248) | 257 | see note 3 |
| submissions | 57,159 | 57,159 | normalised; not loaded |

## Issues (open, by kind)

| Stage | Kind | Severity | Count |
| --- | --- | --- | ---: |
| analyse | converter-missing (Smart Slider) | warning | 1 |
| analyse | access-gap (no crawl connector) | info | 1 |
| normalise | missing-media | warning | 118 |
| normalise | unresolved-link | warning | 31 |
| normalise | unsupported-block | warning | 23 |
| normalise | unresolved-relation | warning | 13 |
| normalise | redirect-chain | info | 15 |
| normalise | unmapped-field | info | 1 |
| schema | schema-mismatch | warning | 22 |
| schema | unmapped-field | warning | 10 |

The kinds the brief expects are present: unmapped fields, unresolved media and Smart Slider
(`converter-missing`).

## Notes

1. **page +1.** Post 11166 (`/home/`) is a published page in this dump but is not in the
   importer's extract (`migration/data/page.json`). Everything else in that file matches
   one to one. Most likely the extract and the dump were taken at different times. The
   importer also redirects `/home/` to `/` (as porter does, from the same rule).
2. **media.** Each of the 2,865 attachments with a file resolves; 2 have no file, matching
   the importer's missing list. Porter stores media by content, so the 226 attachments whose
   bytes repeat another attachment's share one media item: 2,639 items. The importer made one
   Payload doc per attachment. The two folders that hold only such repeats are why there are
   130 folders, not 132. Matching the importer would need one media item per attachment that
   shares the stored file. That is a change to the store's media identity, so it was left as a
   decision. Also, on a case-insensitive disk, 4 files that differ only by case read as their
   sibling. The importer kept those 4 in a separate folder.
3. **redirects.** Replaying `import-redirects.ts` on its own data today gives 257, not 248.
   Porter's 257 match it except for two rows. The importer keeps
   `/promotions/family-escape-break-london//1000/` and `.../1000/` as two redirects, while
   porter's URL keys treat them as one. Porter keeps one Redirection row whose source holds
   commas, which the importer rejects as malformed.
