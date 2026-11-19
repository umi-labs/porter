# Landmark acceptance: result

Run on 2026-11-12 from a clean project with `acceptance/run.sh landmark --local-only` (wp-db
connector only). Counts only.

The **Landmark** column is a reconstruction, not the importer's logged output. It applies the
importer's logic to the importer's own extract (`migration/data/`), and README.md describes each
row. The **porter** column is after normalise.

## What ran

| Step | Run? |
| --- | --- |
| extract, analyse, normalise (wp-db connector, local Docker) | yes |
| `schema check` against the app's Payload config (offline) | yes: 0 errors, 22 warnings |
| crawl snapshot of the old site | **no**: needs the go-ahead to crawl the live site |
| load into the app on a fresh database (`porter run --apply`) | **no**: needs the go-ahead to run the app and create the database |
| verify (brief: at least 417 of 418 old URLs) | **no**: needs the load |

## Counts

| | Landmark (reconstructed) | porter | |
| --- | ---: | ---: | --- |
| page | 353 | 354 | note 1 |
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
| media: attachments resolved | 2,865 | 2,861 | note 2 |
| media: items | 2,865 | 2,639 | note 2 |
| media folders | 132 | 130 | note 2 |
| redirects | 257 (brief: 248) | 257 | note 3 |
| submissions | 57,159 | 57,159 | normalised; not loaded |

## Open issues by kind

| Stage | Kind | Severity | Count |
| --- | --- | --- | ---: |
| analyse | converter-missing | warning | 1 |
| analyse | access-gap | info | 1 |
| normalise | missing-media | warning | 122 |
| normalise | missing-media | info | 85 |
| normalise | unresolved-link | warning | 31 |
| normalise | unsupported-block | warning | 23 |
| normalise | unresolved-relation | warning | 13 |
| normalise | redirect-chain | info | 15 |
| normalise | unmapped-field | info | 1 |
| schema | schema-mismatch | warning | 22 |
| schema | unmapped-field | warning | 10 |

All the kinds the brief expects are present: unmapped fields, unresolved media and the page
slider (`converter-missing`).

## Notes

1. **page.** One page is published in the dump but absent from the importer's extract. Every
   other page id matches. The extract and the dump look to have been taken at different times.
2. **media.**
   - **Missing files.** 2 attachments have no file, as in the reconstruction.
   - **Name case.** 4 attachments name their file with a different case from the file on disk.
     Porter reports them as missing (missing-media warnings) and does not read the sibling file.
     The importer read these 4 from a separate folder of copies.
   - **Repeated bytes.** Porter stores media by content, so attachments whose bytes repeat
     another's share one item: 2,639 items for 2,861 attachments. Each such group with
     differing alt, title or folder gets one missing-media info issue that lists the
     attachments and the values kept. That accounts for the 85 info issues.
   - **Folders.** Two folders hold only repeats, which is why porter has 130, not 132.
   - **Not done.** Matching one item per attachment would need a change to the store's
     media identity.
3. **redirects.** The reconstruction gives 257 on today's data, not the brief's 248. Porter has
   257 too, and the sources (`from` keys) differ in two rows:
   - The importer keeps two sources that differ only by a doubled slash; porter's URL keys
     treat them as one.
   - Porter keeps one plugin row whose source contains commas, which the importer rejects.
