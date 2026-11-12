# Porter reference

## CLI

```
porter [--project <dir>] [--json] [--interactive] [-v...] <command>
```

Global options: `--project <dir>` (folder holding `porter.toml`, default `.`), `--json` (NDJSON on stdout, all human text on stderr), `--interactive` (allow prompts, only when stdin is a TTY), `-v/-vv/-vvv` (log detail on stderr).

| Command | Options | Does |
|---|---|---|
| `init <name> --adapter <a>` | | Create `porter.toml` and an empty store. Adapters: `wordpress`, `umbraco`. Refuses to overwrite |
| `analyse` | `--refresh` | Inventory the source; seed `[mapping]`; write `reports/analysis.html` |
| `extract` | `--refresh` | Fetch into `raw/` and `media/`; resumable |
| `normalise` | `--refresh` | `raw/` to the neutral model; no network |
| `schema <mode>` | `check`, `generate`, `prompt`, `spec` | Prepare the destination |
| `load` | `--apply` | Write to the destination; dry run without `--apply` |
| `verify` | | Check the loaded result against the model |
| `run` | `--continue`, `--apply` | analyse, extract, normalise, schema check, load, verify; stops at the first stage with error issues unless `--continue` |
| `status` | | Counts per stage, open issues, last runs |
| `issues` | `--stage`, `--kind`, `--severity`, `--open`, `--resolve <id> --as handled\|wont-fix [--note]` | List or resolve issues. `--as handled` needs `--note` |
| `report` | | Regenerate `reports/*.html` |
| `export <file>` | | Write status and issues to a JSON file |
| `clean` | | Delete `raw/`, `media/`, `reports/`, `project.db` |
| `upgrade` | `--check`, `--yes`, `--force` | See below |

Exit codes: `0` finished and this run opened no error issues; `1` could not run; `2` finished with error issues or failed items; `3` interrupted, resumable.

### `porter upgrade`

Detects how porter was installed from the path of the running binary. Under a Homebrew prefix or `Cellar` it runs `brew update` then `brew upgrade` (trying `umi-labs/tap/porter`, then `porter`). Anywhere else it downloads the [umi-labs/porter](https://github.com/umi-labs/porter/releases) release asset whose name contains this platform's target triple and verifies it against the release's `<archive>.sha256` (a mismatch aborts and leaves the binary untouched), and replaces the running binary. With `--json` a final `upgrade` event reports `{from, to, upgraded}`.

- `--check`: print current and latest version (and, with `--json`, one `upgrade-check` event); change nothing; exit 0.
- `--yes`: replace without asking. Without `--yes`, porter refuses (exit 1), unless `--interactive` is set on a terminal, where it asks `[y/N]`.
- `--force`: upgrade even when already current.

## Event schema

See the README, section "Automation: `--json`".

## Configuration

See the README, sections "Connectors" and "`porter.toml`". The Rust types are `porter_engine::config::ProjectConfig` and `porter_engine::mapping::MappingConfig`; `ProjectConfig::validate()` returns the problems, each starting with the exact key at fault.

## Crates

Porter is a Cargo workspace; the engine is usable as a library, and the CLI, and later the TUI and desktop app, are thin shells over it.

**`porter-model`**: the neutral model. Everything serialises to JSON.
- `Document`, `FieldValue`, rich text (`richtext`), layout blocks (`layout`: `Block`, `BlockKind`), `Media`, `Route`, `Redirect`, site-wide data (`site`: menus, taxonomies, forms, settings, authors).
- `ItemId` (UUIDv5 over `"{kind}|{connector}|{source_id}"`, stable across runs), `ItemKind`, `ItemStatus` (`Done`, `Partial`, `Skipped`, `Failed`), `Stage`.
- `Issue`, `IssueKind`, `Severity`, `Resolution`.
- No platform knowledge lives here.

**`porter-engine`**: the stages and everything they share.
- `project::Project`: open or init a project folder; `clean`.
- `store::Store`: the SQLite store (items, statuses, issues, runs). Every table has `updated_at`; mutable ones `deleted_at`.
- `stage`: `run_stage` and the adapter traits `Connector`, `Source`, `Destination` (with `Capabilities`), `SchemaMode`, `StageCtx`.
- `events`: `Event` (serialises with `"v": 1`), `EventSink`, `Summary`, `exit_code`.
- `mapping`: `MappingConfig` and the transforms; `verify`; `report` (HTML reports); `url`, `html`, `http` helpers.
- `connectors`: the generic `crawl` connector. `sources/wordpress`, `sources/umbraco`, `destinations/payload`: the platform code. Core modules never name a platform.
- `registry`: maps adapter names (`wordpress`, `umbraco`, `payload`) to implementations.

**`porter-cli`**: the `porter` binary: argument parsing, `--json` and human rendering of events, exit codes.

### Adding an adapter

Implement `Source` (and `Connector`s) or `Destination` under `sources/<name>/` or `destinations/<name>/`, declare its `Capabilities` so analyse and schema can raise `converter-missing` and `access-gap` before anything is attempted, and register it in `registry.rs`. Rules the engine holds you to: every item leaves every stage with exactly one `ItemStatus`; issue kinds come from the fixed list; fixtures in tests are synthetic.
