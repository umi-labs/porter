# Porter

Porter migrates a client website from one CMS into another and checks the result. It reads a source (WordPress, Umbraco), turns it into a neutral model, loads it into a destination (Payload), and then verifies that what landed matches what was there.

Porter is a CLI first. Every command works the same way for a person at a terminal and for a script or an AI agent: `--json` makes stdout pure NDJSON, nothing prompts unless you ask, and exit codes say what happened.

> **Porter 2.0 is in active development.** The engine is built and tested: the five stages and verify, the WordPress and Umbraco sources, the crawler, and the Payload destination. Not built yet: the terminal UI, the desktop app, the hub, the Sanity, Webflow, Squarespace and Gatsby sources and destinations, and the Elementor and WPBakery converters (porter records a `converter-missing` issue for those builders instead of guessing). Coming from 1.x? Read [UPGRADING.md](UPGRADING.md).

## Install

```bash
brew install umi-labs/tap/porter
```

Or download a binary for your platform from the [releases page](https://github.com/umi-labs/porter/releases), or build from source (Rust 1.89 or newer, 2024 edition):

```bash
cargo build --release   # target/release/porter
```

`porter upgrade` updates a Homebrew install through `brew`, and any other install by replacing the binary from the GitHub release for your platform. See [Upgrading porter](#upgrading-porter).

## A migration is a project folder

One folder per client migration. It is the source of truth.

```
acme-migration/
  porter.toml          connectors, mapping decisions, destination (no secrets)
  project.db           SQLite: items, statuses, issues, runs, the normalised model
  raw/<connector>/     what each connector fetched, untouched
  media/ab/cd/<sha256> content-addressed media files
  reports/             generated HTML reports; schema check/prompt/spec output
  generated/payload/   schema files from `porter schema generate`
```

`porter clean` deletes only `raw/`, `media/`, `reports/` and `project.db` inside the project folder, never the folder itself and never `porter.toml`.

Every command takes `--project <dir>` (default: the current directory).

## The stages

```bash
porter init acme --adapter wordpress     # writes porter.toml, edit it
porter analyse                           # 1
porter extract                           # 2
porter normalise                         # 3
porter schema check                      # 4
porter load                              # 5 (dry run)
porter load --apply                      # 5
porter verify                            # after load
porter run --apply                       # all of the above, in order
```

1. **analyse** (read-only). Uses every configured connector to inventory the site: platform, builders and plugins detected, content types and counts, the URL tree, media, redirects, forms, menus, languages. It writes a coverage section listing what each connector contributed and what is unknown for lack of access, and merges what several connectors saw by URL. Output: the store and `reports/analysis.html`. It also seeds `[mapping]` in `porter.toml` once, with defaults for you to edit.
2. **extract**. Fetches everything into `raw/` and `media/`: content, metadata, media files, menus, forms, redirects, rendered pages. Resumable; it skips what it already has unless you pass `--refresh`, so for a launch-day delta (content or files changed since the first extract), run `porter extract --refresh`. Output: `reports/extract.html`.
3. **normalise**. Converts `raw/` into the neutral model (documents, rich text, layout blocks, media, routes, redirects, menus, taxonomies, forms, settings). Resolves internal links and media refs, collapses redirect chains, builds the route table. No network, so it is fast to re-run; every run rebuilds the model from `raw/`.
4. **schema** (prepare the destination). Works out what the destination has to hold from the model plus your mapping.
   - `porter schema check` reads the destination's existing schema (for Payload, its TypeScript collection and global configs, statically) and reports every mismatch as a `schema-mismatch` issue.
   - `porter schema generate` writes Payload collection and block configs to `generated/payload/`.
   - `porter schema prompt` writes `reports/schema-prompt.md`, a self-contained prompt for an AI agent to create the schema, with the check command to verify against.
   - `porter schema spec` writes a human-readable spec (HTML and Markdown) to `reports/`.
5. **load**. Writes to the destination, keyed by source identity so a re-run updates rather than duplicates. Order: media, taxonomies, documents (parents first; create everything, then fill references), menus, forms, redirects. **It is a dry run unless you pass `--apply`.**
6. **verify**. Checks the loaded result against the model: counts per type; every document's parent; every internal link and reference points where it did in the source; every embedded media ref resolves to an uploaded file; every old URL reaches a 200 directly or through one redirect hop. The URL check needs the new site's frontend: set `[verify] site_url` (a dev or staging server). Without it, porter checks routes inside the model only and records an `access-gap` issue saying the live check was skipped.

`porter run` runs analyse, extract, normalise, schema check, load and verify in order and stops at the first stage that ends with error-severity issues or failed items (or could not run), unless you pass `--continue`. `--apply` is passed on to load. Without `--apply` load is a dry run, so verify is skipped and reported as `verify skipped: dry run` (a `stage-skipped` event under `--json`); it does not affect the exit code.

Other commands: `porter status` (counts per stage, open issues, last runs), `porter issues` (list and filter, resolve), `porter report` (regenerate the HTML reports), `porter export <file>` (write status and issues as JSON), `porter clean`, `porter upgrade`.

### Items and issues

Every entity is an item, and leaves every stage with exactly one status: `done`, `partial` (with a reason), `skipped` (with a reason) or `failed` (with an error). Nothing is dropped silently.

An issue has a kind, a severity (`error`, `warning`, `info`), the item and stage it came from, a message, and a resolution (`open`, `fixed-by-rerun`, `handled-manually` with a note, `wont-fix`). Re-running a stage resolves the issues that no longer occur; "handled manually" survives re-runs. Kinds (kebab-case on the wire): `unresolved-link`, `missing-media`, `unmapped-field`, `unsupported-block`, `unresolved-relation`, `parent-missing`, `redirect-chain`, `redirect-loop`, `write-failed`, `access-gap`, `converter-missing`, `schema-mismatch`.

```bash
porter issues --open --severity error
porter issues --kind unresolved-link --stage normalise
porter issues --resolve <id> --as handled --note "fixed in the CMS"
porter issues --resolve <id> --as wont-fix
```

## Connectors

A source adapter reads through one or more connectors, each declared as a `[[connectors]]` block in `porter.toml`. All are optional; combine as many as you have access to. Credentials never go in `porter.toml`; see [Environment variables](#environment-variables).

### `crawl`: the public site

Reads `sitemap.xml`, `robots.txt` and links within the domain. Records status, redirect hops, title, meta, OG tags, canonical, hreflang and the rendered HTML, and fetches the media the pages reference. Always available.

```toml
[[connectors]]
kind = "crawl"
url = "https://www.example.com"
concurrency = 4               # parallel requests
delay_ms = 250                # politeness delay between requests
max_pages = 10000
include = ["/rooms/*"]        # URL globs; empty = everything
exclude = ["/wp-admin/*", "/cart/*"]
keep_query = ["page"]         # query params kept on links; all others are dropped
max_media_bytes = 209715200   # skip media larger than this (200 MB)
```

### `wp-rest`: WordPress REST API

Reads posts, pages, custom types, taxonomies, media, menus (where exposed) and users from `/wp-json`. Without credentials it sees only what is public, and records `access-gap` issues for the rest.

```toml
[[connectors]]
kind = "wp-rest"
url = "https://www.example.com"
auth = "basic"                # basic | bearer | apikey. Secrets come from PORTER_WP_* env vars
# keyring = "acme-wp"         # or a keyring entry (service "porter", account "acme-wp")
```

### `wxr`: WordPress export file

Reads a WordPress eXtended RSS export (Tools, Export). Needs no access to the live site.

```toml
[[connectors]]
kind = "wxr"
path = "export/acme.xml"      # relative to the project folder
```

### `wp-db`: WordPress database dump

The fullest-fidelity way in. Porter loads the SQL dump into a throwaway MariaDB container and, with `wp_root`, runs a bundled exporter inside WordPress (`wordpress:cli`), so ACF fields come out under their real names, permalinks resolve (including private posts), and form definitions and submissions, Redirection rules and menus are captured. **Needs Docker.** The site's own PHP runs, but only in a disposable container on an internal network with no route out; the dump and `wp_root` are mounted read-only. Without `wp_root` it falls back to reading tables directly with reduced fidelity and records an `access-gap`.

```toml
[[connectors]]
kind = "wp-db"
dump = "export/acme.sql.gz"       # plain or gzip
wp_root = "export/wordpress"      # WordPress core, themes and plugins
table_prefix = "wp_"
site_url = "https://www.example.com"   # overrides the dump's `home` for permalinks
# uploads_dir = "export/uploads"  # when uploads aren't at wp_root/wp-content/uploads
```

### `umbraco-json`: Umbraco JSON export

Reads Umbraco JSON export files, including `_variants` locale handling. Use it with `[source] adapter = "umbraco"`.

```toml
[source]
adapter = "umbraco"

[[connectors]]
kind = "umbraco-json"
paths = ["export/pages.json", "export/hotels.json"]
```

## `porter.toml`

A full annotated example, WordPress to Payload. Every section is optional in the file, but `porter` reports exactly what is missing (`destination.url is missing`) and fails with exit code 1 before doing anything. Unknown keys are errors, so a typo is caught rather than ignored.

```toml
[project]
name = "acme"

[source]
adapter = "wordpress"                 # wordpress | umbraco

# Zero or more connectors; see above.
[[connectors]]
kind = "wp-db"
dump = "export/acme.sql.gz"
wp_root = "export/wordpress"

[[connectors]]
kind = "crawl"
url = "https://www.example.com"

[destination]
adapter = "payload"
url = "https://cms.example.com"       # the Payload site's base URL
api_key_env = "PORTER_PAYLOAD_API_KEY" # NAME of the env var holding the API key, never the key
auth_collection = "users"             # the collection API keys belong to
config_dir = "../acme-site/src"       # where Payload's TS configs live, for schema check/prompt

[verify]
site_url = "http://localhost:3000"    # new site's frontend, for the old-URL check

# Rules that come from no source (members area, payment pages...)
[[redirects]]
from = "/members"
to = "/account"
code = 301                            # any 3xx; default 301

[mapping]
raw_block = "rawHtml"                 # destination block for content with no native shape
locales = { mode = "skip" }           # or { mode = "map", default = "en", others = ["fr"] }
taxonomies = { category = "categories", post_tag = "tags" }  # unlisted taxonomies aren't loaded

[mapping.media]
collection = "media"                  # default
folders = true                        # keep the upload-path folder structure

[mapping.redirects]
collection = "redirects"              # default
from_fields = ["acf.legacy_url"]      # fields holding a document's earlier URL

[mapping.forms]
collection = "forms"
submissions = "form-submissions"      # unset: submissions aren't loaded
retire = ["Test form"]                # form titles not to load

[mapping.menus]
collection = "menus"                  # one doc per menu: title, items
# or: global = "navigation" with fields = { Main = "main", Footer = "footer" }

[mapping.settings]
global = "site-settings"
front_page = "homePage"               # relationship field for the front page
site_name = "siteName"

# Source type -> destination collection. A type with no `to` is dropped (give a reason).
[mapping.types.page]
to = "pages"
path = "/{slug}/"                     # new URL pattern: used for redirect targets and routes
key_field = "legacyId"                # the destination's upsert key (default legacyId)
key_type = "text"                     # text | number
status = { private = "draft", trash = "skip" }  # "skip" leaves those out of the load

[mapping.types.page.fields]
"acf.hero_title" = { to = "hero.title", transform = { op = "trim" } }
"acf.subtitle"   = { to = "hero.subtitle", transform = [{ op = "trim" }, { op = "default", value = "" }] }
"yoast_focuskw"  = { drop = true, reason = "not used on the new site" }

[mapping.types.post]
to = "pages"                          # many types can map to one collection
path = "/news/{slug}/"

[mapping.types.shop_order]
drop_reason = "orders stay in the shop system"

# Builder module / block kind -> a destination block
[[mapping.blocks]]
match = { kind = "rich-text" }
to = "content"
fields = { html = "body" }

[[mapping.blocks]]
match = { module = "et_pb_slider" }   # builder module name
to = "heroSlider"
```

Field transforms (`transform` takes one inline table or a list, applied in order): `trim`, `lower`, `upper`, `slugify`, `number`, `date-parse { format }`, `date-format { format }`, `split { sep }`, `join { sep }`, `url-rewrite { from, to }`, `regex { pattern, replace }`, `default { value }`, `lookup { table }`. There is deliberately no scripting language.

A field mapped with neither `to` nor `drop = true` is an error. Source fields with no mapping raise `unmapped-field` warnings, so decisions are never silent. `schema check` also validates the mapping against the destination schema and reports every unmapped neutral field and every required destination field with no source.

Mapping onto collections that already exist is the normal case: many source types can map to one collection, neutral field paths map to destination field paths, and builder blocks map to your own block slugs.

## Terminal UI

`porter tui` opens an interactive view of a project: stages, issues and items, with a log of the run in progress. Running bare `porter` (no subcommand) does the same when both stdin and stdout are a terminal; in a script or pipe it prints help instead. `--project <dir>` picks the folder. It works at 80x24, honours `NO_COLOR`, and uses your terminal's colours. It uses the engine directly; it never shells out to `porter`.

If the folder has no `porter.toml`, a setup form opens instead: fill in the project, source and destination fields (`ctrl-s` writes `porter.toml` and opens the project; it never overwrites an existing file).

The main keys (`?` shows the full list in the app):

**Anywhere**

| Key | Does |
| --- | --- |
| `tab` | focus the next panel |
| `S-tab` | focus the previous panel |
| `1-4` | focus a panel by number |
| `R` | analyse → dry-run load; skips verify |
| `l` | load, dry run |
| `L` | load, apply (asks first) |
| `v` | verify |
| `ctrl-c` | close prompts, cancel the run; again to quit |
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

**Honesty notes.** The TUI shows the engine's own outcomes: a stage is only shown as clean when the stored run was clean, and engine errors are printed verbatim. Load never applies by default: `l` is a dry run, and `L` opens a prompt that shows the last dry run's counts and applies only after an explicit `y` (any other key cancels). The prompt re-reads the dry run when you press `y` and refuses if it is stale or missing, so run `l` first and again after changing anything. The TUI never asks for secrets: the setup form shows which environment variable a connector reads and whether it is set, and rejects a pasted secret.

**Status.** The TUI is tested against a real project (an end-to-end test drives `analyse` on the WXR fixture) and against a fake terminal, but its author has not yet exercised it in a real terminal. Please report rough edges.

See the [user guide](docs/user-guide.md#terminal-ui) for every key and the setup flow.

## Automation: `--json`

With `--json`, **stdout carries only NDJSON events**, one object per line. Every human message, log line and error goes to stderr. Every event has `"v": 1` (the schema version) and an `"event"` name. Parse stdout line by line and branch on `event`.

Stage events (analyse, extract, normalise, schema, load, verify):

| `event` | Fields |
|---|---|
| `stage-started` | `stage`, `run` (run id) |
| `progress` | `stage`, `done`, `total` (number or null), `label` |
| `item-result` | `stage`, `item` (item id), `kind` (item kind), `status` |
| `issue-opened` | `issue` |
| `stage-finished` | `stage`, `run`, `summary` |

- `stage` is one of `analyse`, `extract`, `normalise`, `schema`, `load`, `verify`.
- `status` is `"done"`, or an object `{"partial": {"reason": "..."}}`, `{"skipped": {"reason": "..."}}`, `{"failed": {"error": "..."}}`.
- `issue` is `{id, kind, severity, item, stage, message, details, resolution}`; `kind` is one of the twelve kinds above, `severity` is `error|warning|info`, `item` may be null, `resolution` is `"open"`, `"fixed-by-rerun"`, `"wont-fix"` or `{"handled-manually": {"note": "..."}}`.
- `summary` is `{done, partial, skipped, failed, errors, warnings, infos, interrupted}`: counts of items by status and of issues opened by severity.

Example:

```json
{"v":1,"event":"stage-started","stage":"normalise","run":"0192f3a0-..."}
{"v":1,"event":"progress","stage":"normalise","done":120,"total":480,"label":"documents"}
{"v":1,"event":"issue-opened","issue":{"id":"...","kind":"unresolved-link","severity":"warning","item":"...","stage":"normalise","message":"...","details":{},"resolution":"open"}}
{"v":1,"event":"stage-finished","stage":"normalise","run":"0192f3a0-...","summary":{"done":478,"partial":2,"skipped":0,"failed":0,"errors":0,"warnings":3,"infos":0,"interrupted":false}}
```

Other commands print one event each: `init` (`dir`, `name`), `status` (`stages`, `open_issues`, `last_runs`), `issue` (`issue`, one per line from `porter issues`), `issue-resolved` (`id`, `resolution`), `clean` (`dir`), `export` (`path`), `report` (`paths`: the files written), `mapping-seeded` (no fields; emitted after analyse when `[mapping]` was seeded in `porter.toml`), `upgrade-check` (`current`, `latest`, `update_available`, `install`) and `upgrade` (`from`, `to`, `upgraded`: whether the binary was replaced).

Porter never prompts unless stdin is a terminal and you passed `--interactive`; missing configuration is an error that names exactly what is missing.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Finished, and this run opened no error-severity issues |
| 1 | Could not run: bad config, missing credentials, unreachable source, internal error |
| 2 | Finished with error-severity issues or failed items (also what a command-line usage error exits with, before any stage runs) |
| 3 | Interrupted (Ctrl-C); resumable, run the same command again |

## Environment variables

Credentials come from the environment (or a keyring entry named in config), never from `porter.toml`.

| Variable | Used by | Meaning |
|---|---|---|
| `PORTER_WP_AUTH_TYPE` | `wp-rest` | `basic`, `bearer` or `apikey` (overrides `auth` in config) |
| `PORTER_WP_USERNAME`, `PORTER_WP_PASSWORD` | `wp-rest`, basic | WordPress user and an application password |
| `PORTER_WP_BEARER_TOKEN` | `wp-rest`, bearer | Token sent as `Authorization: Bearer` |
| `PORTER_WP_API_KEY` | `wp-rest`, apikey | Key sent in a header |
| `PORTER_WP_API_KEY_HEADER` | `wp-rest`, apikey | Header name for the key (default `X-API-Key`) |
| `PORTER_PAYLOAD_API_KEY` | Payload destination | Conventional name for the Payload API key; `destination.api_key_env` names whichever variable you actually use |

`-v`, `-vv`, `-vvv` raise log detail on stderr (info, debug, trace).

## Upgrading porter

```bash
porter upgrade --check        # report the latest version, change nothing
porter upgrade --yes          # upgrade: via brew for Homebrew installs, else replace the binary
porter upgrade --interactive  # on a terminal, ask before replacing
porter upgrade --yes --force  # reinstall even when already current
```

Porter never replaces itself without `--yes` (or an answer at an `--interactive` prompt), so a script can't self-update by accident. Non-Homebrew installs download the release asset for their target triple from [umi-labs/porter](https://github.com/umi-labs/porter/releases).

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

Crates: `porter-model` (the neutral model, no platform knowledge), `porter-engine` (stages, store, events, mapping, reports, plus `sources/<name>` and `destinations/<name>`), `porter-cli` (the `porter` binary). Releases go through `scripts/release.sh`. More in [docs/](docs/README.md); the design lives in `docs/superpowers/specs/`.

## License

MIT
