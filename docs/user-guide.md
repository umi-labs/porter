# Porter user guide

This walks through a migration from a WordPress site to Payload, then covers everyday use. For the full reference (every connector option, the whole `porter.toml`, the event schema) see the [README](../README.md).

Porter 2.0 is in active development: see the README for what is not built yet.

## Before you start

- porter installed (`brew install umi-labs/tap/porter`; check with `porter --version`).
- Something to read from. Best is a database dump plus the site's `wp-content` (the `wp-db` connector, needs Docker). Otherwise a WXR export file, the WordPress REST API with an application password, or just the public site (the crawler).
- A Payload site to load into, with an API key for a user in its auth collection.

## 1. Create the project

```bash
mkdir acme-migration && cd acme-migration
porter init acme --adapter wordpress
```

Edit `porter.toml`: add the connectors you have and the destination.

```toml
[[connectors]]
kind = "wp-db"
dump = "export/acme.sql.gz"
wp_root = "export/wordpress"

[[connectors]]
kind = "crawl"
url = "https://www.example.com"

[destination]
adapter = "payload"
url = "http://localhost:3000"
api_key_env = "PORTER_PAYLOAD_API_KEY"
config_dir = "../acme-site/src"

[verify]
site_url = "http://localhost:3000"
```

Export the key rather than writing it in the file: `export PORTER_PAYLOAD_API_KEY=...`. If something is missing, porter exits 1 and names the key (`destination.url is missing`).

## 2. Look at the site

```bash
porter analyse
```

Open `reports/analysis.html`: platform, builders and plugins, content types and counts, URL tree, media, forms, and a coverage section showing what each connector could and could not see. `converter-missing` issues name page builders porter can't convert yet. analyse also writes a starting `[mapping]` into `porter.toml` (once). Read it and edit: which source types go to which collections (or are dropped, with a reason), field renames, block mappings, URL patterns.

## 3. Fetch and convert

```bash
porter extract       # downloads into raw/ and media/; resumable, --refresh to refetch
porter normalise     # raw/ -> the neutral model; no network, safe to re-run
porter issues --open
```

Fix what you can in the mapping and re-run `porter normalise`; issues that no longer occur resolve themselves. Mark the ones you've dealt with elsewhere: `porter issues --resolve <id> --as handled --note "..."`.

## 4. Get the schema ready

```bash
porter schema check      # compare the model + mapping to the Payload site's configs
porter schema generate   # write collection/block configs to generated/payload/
porter schema prompt     # write reports/schema-prompt.md for an AI agent to build the schema
```

Run `check` until it reports no `schema-mismatch` errors.

## 5. Load and verify

```bash
porter load              # dry run: reports what would be written
porter load --apply      # writes to Payload
porter verify            # counts, parents, links, media, and every old URL
```

Loading is keyed by source identity: running `load --apply` again updates documents instead of duplicating them. Each verify failure is an issue.

Or do it all: `porter run --apply` (add `--continue` to keep going past a stage with errors).

## Everyday use

- `porter status`: counts per stage, open issues, last runs.
- `porter issues --kind unresolved-link --stage normalise`: filter issues. Kinds: `unresolved-link`, `missing-media`, `unmapped-field`, `unsupported-block`, `unresolved-relation`, `parent-missing`, `redirect-chain`, `redirect-loop`, `write-failed`, `access-gap`, `converter-missing`, `schema-mismatch`.
- `porter report`: regenerate the HTML reports in `reports/`. They are self-contained single files, easy to share.
- `porter export status.json`: status and issues as JSON.
- `porter clean`: delete `raw/`, `media/`, `reports/` and `project.db` (never `porter.toml`).
- Ctrl-C stops cleanly with exit code 3; run the same command to resume.

## Scripts and CI

```bash
porter --json run --apply > events.ndjson 2> porter.log
echo $?
```

With `--json` stdout is only NDJSON events (each with `"v": 1`), so `jq -c 'select(.event=="issue-opened")' events.ndjson` just works. Exit codes: 0 clean, 1 could not run, 2 finished with errors, 3 interrupted. Nothing prompts unless stdin is a terminal and `--interactive` is set. Event fields are in the README.

## Troubleshooting

| Symptom | Cause |
|---|---|
| Exit 1, `... is missing` | A required `porter.toml` key; the message names it |
| Exit 1, `unknown field` | A typo in `porter.toml`; unknown keys are errors |
| `access-gap` issues | A connector lacked access (no `wp_root`, no credentials, no `verify.site_url`); the issue says what |
| `converter-missing` | A page builder porter has no converter for yet (Elementor, WPBakery); content comes through as raw HTML blocks where `mapping.raw_block` is set |
| `wp-db` fails to start | Docker isn't running or can't pull `mariadb:11` / `wordpress:cli` |
| 1.x flag rejected | See [UPGRADING.md](../UPGRADING.md) |

Use `-v`, `-vv` or `-vvv` for more detail on stderr.
