# Upgrading from porter 1.x to 2.0

Porter 2.0 is a rebuild, not an update. The 1.x commands and flags are gone and are not kept working. A removed flag (`--source`, `--target`, `--collection`...) is a command-line usage error: porter prints the usage error and a note pointing here, and exits with code 2 before any stage runs. (In the README's exit codes, 2 means a stage finished with error issues; a usage error is clap's own exit 2.) Plan to redo your setup rather than convert it in place.

## What changed

**The model.** 1.x ran one pass, `porter migrate --source X --target Y`, from a source straight to seed files. 2.0 works on a project folder (one per client migration) and runs stages you can re-run and inspect: `analyse`, `extract`, `normalise`, `schema`, `load`, `verify`. Every item gets a status per stage and every problem becomes an issue. See the README.

**Removed flags.** These no longer exist: `--source`, `--target`, `--collection`, `--output`, `--plugin-dir`, `--list-adapters`, `--dry-run`, `--debug`, `--fixtures`, and `-c/--config <file>` on subcommands. Their replacements:

| 1.x | 2.0 |
|---|---|
| `--source`, `--target` | `[source] adapter` and `[destination] adapter` in `porter.toml` |
| `--collection` | `[mapping.types.<type>] to = "<collection>"` |
| `--output ./seed` | gone; load writes straight to the destination (see below) |
| `--dry-run` | the default for `porter load`; add `--apply` to write |
| `--debug`, `--verbose` | `-v`, `-vv`, `-vvv` (log detail on stderr) |
| `--config <file>` | `--project <dir>` (the folder holding `porter.toml`) |
| `--list-adapters` | gone; `porter init --adapter nope` exits 1 with `unknown adapter "nope" (available: wordpress, umbraco)` and writes nothing |
| `--plugin-dir` | gone; there is no plugin system |

**Removed commands.** `config`, `generate`, `template`, `migrate`, `explain`. Use `analyse`, `extract`, `normalise`, `schema generate|check|prompt|spec`, `load`. `init`, `clean` and `upgrade` remain but work differently (below).

**Config format is new.** 1.x read `porter.config.toml` (or JSON). 2.0 reads `porter.toml` in the project folder, with `[project]`, `[source]`, `[[connectors]]`, `[destination]`, `[mapping]`, `[[redirects]]` and `[verify]`. Old files are not read and cannot be converted automatically; the mapping decisions in them (field renames, transforms, collections) go into `[mapping.types.<type>.fields]`. Unknown keys are errors, so a typo is caught. Credentials never go in the file: it names the environment variable that holds them (`api_key_env`).

**Seed-file output is replaced by the Payload REST load.** 1.x wrote seed files for you to run. 2.0 loads into Payload over its REST API with an API key: media is uploaded, documents are upserted by a hidden `legacyId` field (so re-runs update rather than duplicate), references are filled in a second pass, and redirects go to the redirects collection. `porter load` is a dry run until you pass `--apply`. Create an API key for a user in your auth collection, put it in an environment variable, and set `destination.api_key_env` to its name. `porter schema check` flags a missing `legacyId`; `porter schema generate` adds it.

**Umbraco config now lives under `[[connectors]] kind = "umbraco-json"`.** What was source options in 1.x is a connector:

```toml
[source]
adapter = "umbraco"

[[connectors]]
kind = "umbraco-json"
paths = ["export/pages.json"]
```

The `_variants` locale handling is kept. WordPress is now reached through the `crawl`, `wp-rest`, `wxr` and `wp-db` connectors; its credentials come from `PORTER_WP_*` environment variables.

**`porter clean` is safer.** It deletes only `raw/`, `media/`, `reports/` and `project.db` inside the project folder, never the folder itself, never `porter.toml`, and never a seed directory. 1.x could delete the working directory.

**`porter upgrade` changed.** Homebrew installs still upgrade through `brew`. Other installs now replace the binary from the GitHub release for their platform. Neither replaces anything without `--yes` (or an answer at an `--interactive` prompt on a terminal). `porter upgrade --check` only reports the latest version. `--force` reinstalls when already current.

**Scripting.** `--json` makes stdout pure NDJSON events (each with `"v": 1`); all human text goes to stderr. Exit codes: 0 clean, 1 could not run, 2 finished with errors, 3 interrupted and resumable. Nothing prompts unless stdin is a terminal and you pass `--interactive`.

## Moving a 1.x migration to 2.0

1. `brew upgrade umi-labs/tap/porter` (or `porter upgrade --yes` on a 1.x that has it), then `porter --version`.
2. `porter init <name> --adapter wordpress` (or `umbraco`) in a new folder.
3. Add your connectors and `[destination]` to the new `porter.toml`.
4. `porter analyse`; it seeds `[mapping]` with defaults. Carry your old decisions across by hand.
5. `porter run`: load is a dry run (would-be writes go to `raw/load-preview/`) and verify is skipped, since nothing reached the destination. It stops at the first stage with error issues or failed items; `--continue` runs on. Read `porter issues --open`, then `porter run --apply` to load and verify.

## Not built yet

2.0 ships the engine and CLI. The terminal UI, desktop app, hub, the Sanity, Webflow, Squarespace and Gatsby adapters, and the Elementor and WPBakery converters are not part of it. Targets other than Payload that 1.x listed (such as Strapi) are not supported.
