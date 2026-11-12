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

## Terminal UI

`porter tui` is the same engine as the commands above, in a full-screen view. Four panels (Project, Stages, Issues, Items) sit on the left, a Detail pane on the right and the log along the bottom. Running `porter` with no subcommand opens it when stdin and stdout are both a terminal; otherwise it prints help, so scripts are unaffected. It works at 80x24, honours `NO_COLOR` and uses your terminal's own colours. The terminal is restored when you quit, on a panic and on SIGTERM.

### Setup
In a folder without `porter.toml` the TUI opens a setup form instead of the project. Move between fields with `j`/`k`, tab or the arrow keys, press `enter` to type into a field (`enter` or `esc` finishes), and `space` or the arrow keys to choose among options. Optional connector sections open from their `use` field. `ctrl-s` writes `porter.toml` and opens the project straight away. Setup never overwrites an existing `porter.toml`, round-trips every connector, and shows validation problems next to the field that caused them. There is no field for secrets: the form shows the environment variable a connector reads and whether it is set, and a pasted secret is rejected.

### Running and reading
Focus the Stages panel, move to a stage and press `space` to run it; `R` runs all stages, `v` verifies, `c` cancels the running stage. Progress and counts update live, then settle to the stored result of the run (clean, warnings, errors, interrupted, or could not run). Issues are grouped by kind; `enter` drills into a group and then an issue, `x` resolves it with a note, `g` jumps to its item, `/` filters, `f` cycles all, open and errors only. `o` opens the HTML report and `e` suspends the TUI to edit `porter.toml` in `$VISUAL`/`$EDITOR`, then re-checks it. `y` on an item or issue copies its id to the clipboard (OSC 52, so it needs a terminal that supports it).

### Loading
`l` runs a load dry run. `L` opens an apply prompt that shows the item counts from the last dry run and writes to the live CMS only after you press `y`; any other key cancels. The prompt re-reads the dry run when you press `y` and refuses to apply from a missing or stale one, so a fresh dry run is always required first. Quitting while a stage runs asks first and then cancels the run cleanly, so it can be resumed.

### Every key

**Anywhere**

| Key | Does |
| --- | --- |
| `tab` | focus the next panel |
| `S-tab` | focus the previous panel |
| `1-4` | focus a panel by number |
| `R` | run all stages |
| `l` | load, dry run |
| `L` | load, apply (asks first) |
| `v` | verify |
| `ctrl-c` | cancel the run; again to quit |
| `c` | cancel the running stage |
| `o` | open the HTML report |
| `e` | edit porter.toml |
| `@` | show or hide the log |
| `?` | this help |
| `q` | quit (asks first if a stage runs) |

**Stages**

| Key | Does |
| --- | --- |
| `j/↓` | move down |
| `k/↑` | move up |
| `space` | run the selected stage |

**Issues, Items**

| Key | Does |
| --- | --- |
| `j/↓` | move down |
| `k/↑` | move up |
| `pgdn` | move a page down |
| `pgup` | move a page up |
| `enter` | open in Detail, drill in |
| `/` | filter (fuzzy, incremental) |
| `f` | cycle all, open, errors only |
| `esc` | clear the filter |
| `y` | copy the item or issue id |

**Open issue**

| Key | Does |
| --- | --- |
| `j/↓` | move in the open issue list |
| `k/↑` | move up in the open issue list |
| `pgdn` | page down in the open list |
| `pgup` | page up in the open list |
| `enter` | open the issue, or its item |
| `x` | resolve the issue |
| `g` | jump to the issue's item |

**Detail**

| Key | Does |
| --- | --- |
| `esc` | close Detail, go back |

**Filter box**

| Key | Does |
| --- | --- |
| `enter` | apply the filter |
| `esc` | clear the filter |
| `backspace` | delete a character |

**Resolve box**

| Key | Does |
| --- | --- |
| `↓/tab` | next choice |
| `↑/S-tab` | previous choice |
| `enter` | confirm the resolution |
| `backspace` | delete a note character |
| `esc` | cancel |

**Apply prompt**

| Key | Does |
| --- | --- |
| `y` | write to the live CMS |
| `n/esc` | cancel (any other key too) |

**Quit prompt**

| Key | Does |
| --- | --- |
| `y` | cancel the run and quit |
| `n/esc` | keep running |

**Help**

| Key | Does |
| --- | --- |
| `j/↓` | scroll down |
| `k/↑` | scroll up |
| `?/esc` | close help |
| `ctrl-c` | close help, cancel the run; again to quit |

**Setup**

| Key | Does |
| --- | --- |
| `j/↓/tab` | next field |
| `k/↑/S-tab` | previous field |
| `enter` | type into the field, or pick the next choice |
| `space/→` | next choice |
| `←` | previous choice |
| `ctrl-s` | write porter.toml and open the project |
| `ctrl-c` | quit |
| `q` | quit |

**Setup field**

| Key | Does |
| --- | --- |
| `enter/esc` | finish typing in the field |
| `backspace` | delete a character |
| `ctrl-s` | write porter.toml and open the project |
| `ctrl-c` | quit |

**Honesty notes.** The TUI shows the engine's own outcomes: a stage is only shown as clean when the stored run was clean, and engine errors are printed verbatim. Load never applies by default: `l` is a dry run, and `L` opens a prompt that shows the last dry run's counts and applies only after an explicit `y` (any other key cancels). The prompt re-reads the dry run when you press `y` and refuses if it is stale or missing, so run `l` first and again after changing anything. The TUI never asks for secrets: the setup form shows which environment variable a connector reads and whether it is set, and rejects a pasted secret.

**Status.** The TUI is tested against a real project (an end-to-end test drives `analyse` on the WXR fixture) and against a fake terminal, but its author has not yet exercised it in a real terminal. Please report rough edges.

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
