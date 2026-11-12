# Landmark acceptance

The Landmark site (WordPress with Divi) was moved to Payload by a one-off TypeScript importer
in the Landmark repo (`migration/import-*.ts`). This acceptance run checks that porter, given the
same database dump, produces what that importer produced. The only things committed here are
`porter.toml.example` (the mapping onto the app's collections, globals and blocks) and
`RESULT.md` (counts). `project/` and `snapshot/` are git-ignored and hold client data.

`LANDMARK_DIR` (default `~/Desktop/work/clients/landmark`) is only read. The run uses:

- `wp-source/prodlandmark.sql.gz`: the dump, read by the `wp-db` connector (Docker:
  `mariadb:11` and `wordpress:cli`, on a network with no route out)
- `wp-source/wp`: WordPress, themes and plugins (`wp_root`)
- `wp-source/uploads`: the uploads, copied apart from `wp_root` (`uploads_dir`)
- `src/`: the app's Payload config, read offline by `porter schema check`
- `migration/data/*.json`, `*.tsv`: the importer's own extract, used only to work out the
  expected counts in the table

## Local run (no network, nothing loaded)

```sh
acceptance/run.sh landmark --local-only
```

Runs analyse, extract, normalise and `schema check` with the wp-db connector only, then prints
the table. Re-print the table with `--table`; run single stages with `--stage NAME`.

## Full run

Each of these steps needs an explicit go-ahead first.

1. **Snapshot the old site, once, before launch replaces it.** In a scratch project with only
   a `crawl` connector pointing at the live site, run `porter extract`, then copy its
   `raw/crawl` and `media` folders into `acceptance/landmark/snapshot/` (as `snapshot/crawl/`
   and `snapshot/media/`). From then on the `crawl` connector reads the snapshot through
   `url = "file://.../snapshot"` and never touches the network.
2. **Start the app on a fresh, throwaway database.** Never point porter at Landmark's real
   database. In the Landmark repo: create a new Postgres database (for example
   `landmark_porter`), start the app against it (`DATABASE_URI=.../landmark_porter pnpm dev`),
   create an admin user, and create an API key for that user.
3. **Run.** `PORTER_PAYLOAD_API_KEY=<key> acceptance/run.sh landmark` runs
   `porter run --apply --json` (events go to `project/run.ndjson`) and prints the table.
   `verify.site_url` points at the dev server, `http://localhost:3000`.

Afterwards drop the throwaway database.

## What the table compares

| Row | Landmark side |
| --- | --- |
| documents per WordPress type | `migration/data/<type>.json` as `import-content.ts` reads it (its `TYPES`; a post at a page's URL is skipped) |
| media | attachments whose file exists (`import-media.ts`) |
| media folders | their upload directories, parents included (`import-media-folders.ts`) |
| redirects | `import-redirects.ts` replayed on its data: Redirection rows, `old_site_url`, the hand-written rules, chains collapsed |
| submissions | Ninja Forms submissions with a form plus Gravity Forms entries (`import-submissions.ts`) |

Porter's documents are those a load writes: published and private (as drafts). WordPress
drafts are left out by the mapping (`draft = "skip"`), as the importer never read them.
