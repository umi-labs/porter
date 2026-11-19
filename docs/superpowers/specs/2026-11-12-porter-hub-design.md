# Porter hub design

Status: draft written overnight by Claude under Isaac's instruction to keep building through to the hub. What Isaac decided:
- **deployed with Docker on Dokploy**
- **authentication with Better Auth**
- a central server local projects connect to
- shareable links to view results
- clients can upload data dumps
- stages can run on the server

Everything else is Claude's call, flagged for review.

Depends on: the engine design (project folder, `--json` event stream, store rows ready for sync).

## Why

A migration involves people who don't run porter: PMs, other developers, and the client who has to hand over a database dump or check what's being moved. Today results live in one developer's project folder. The hub puts them on a server people can open in a browser. It takes client uploads without emailing large files around, and it can run long stages on a server instead of a laptop.

## Goals

- **Projects on the hub** mirror local porter projects. Local stays the source of truth; porter works fully offline without a hub.
- **Browser views** of a project:
  - overview and pipeline state
  - analysis and coverage
  - the issues list
  - the URL table
  - the HTML reports
- **Shareable read-only links** with an expiry, for people without an account.
- **Client uploads** of DB dumps, WXR exports and media archives into a project, resumable, for files of several GB.
- **Remote runs** of porter stages on the server, streaming live progress to the browser and syncing results back to the local project.
- **Deployable on Dokploy** from a Docker Compose file.

## Non-goals (v1)

- Editing the mapping in the browser. Edits happen locally (TUI, desktop app or editor) and are pushed.
- Row-level merge of concurrent edits. Sync is whole-snapshot with a version check (below).
- Running the WordPress DB-dump connector on the hub. It needs Docker, and Docker-in-Docker or mounting the host's Docker socket would hand the client's PHP code control of the server. Remote runs exclude `wp-db` and say so (`access-gap`). Developers run `wp-db` locally and push.

## Architecture

The hub is **TypeScript**, because Better Auth is a TypeScript library. It drives porter through the binary's headless interface: `--json` NDJSON events, exit codes, and the project folder. That interface was built for exactly this. *(Claude's call: this replaces the engine design's "porter-hub Rust crate" idea; Isaac's Better Auth choice settles it)*

**Services** (one `docker-compose.yml`):

| Service | Role |
|---|---|
| `web` | Next.js 15 (App Router) app: UI, REST API for the CLI, Better Auth routes, server-sent events for live runs |
| `worker` | Node process that claims queued runs, executes the `porter` binary, streams events into Postgres, and stores results |
| `db` | Postgres 17: Better Auth tables, projects, versions, runs, run events, share links, uploads (Drizzle ORM) |
| `storage` | S3-compatible object storage: MinIO in compose, or any S3 (Cloudflare R2, AWS) via env vars. Holds project snapshots, uploads, reports |

- The `porter` binary is built in the image's Rust stage from this repo, and copied into the `web` and `worker` images.
- **Project files on the server:** the worker unpacks a project snapshot into a scratch volume per run, runs porter there, then uploads the resulting snapshot as a new version.

## Data model (Postgres)

- **Better Auth tables:** user, session, account, verification, plus its organization plugin tables (organization, member, invitation).
- **`project`:** id, org id, name, slug, source → destination summary, created by, archived at, `retention_days` (default 90 after archive).
- **`project_version`:**
  - id, project id, number
  - snapshot object key: a tar.zst of `porter.toml`, `project.db`, `raw/` and `reports/`; **media is stored separately**
  - porter version, created by, created at, size
  - summary JSON from `porter status --json`
- **`media_object`:** project id, sha256, size, content type. Media is content-addressed, so pushes upload only hashes the hub doesn't have.
- **`run`:**
  - id, project id, base version, stage list, apply flag
  - state: queued / running / finished / failed / cancelled
  - exit code, result version id, requested by, timestamps
- **`run_event`:** run id, seq, the NDJSON event as JSON. Live progress is served from this.
- **`share_link`:** id, project id, token hash, scope (`report`, or `project-readonly`), expires at, created by, revoked at.
- **`upload`:** id, project id, uploader, filename, size, sha256, object key, state, created at, purpose (dump / wxr / media-archive / other).

## Access

- **Staff:** Google sign-in through Better Auth, restricted to Umi's Google Workspace domain *(Claude's call)*, plus email and password as a fallback. Staff belong to the Umi organization.
- **Clients:** invited per project by email. They sign in with a **magic link** (Better Auth's magic-link plugin), and can only see the projects they were invited to. Client role: view the project's overview, coverage and reports, and upload files. They can't see raw data, issues detail or other projects.
- **Roles per project:** `owner`, `developer` (push, pull, run, share), `viewer`, `client`.
- **Share links:**
  - Read-only, expiring (default 14 days), revocable.
  - The token is shown once and stored hashed.
  - Scope `report` serves only the HTML reports.
  - **No share link ever exposes raw data or submissions.**
- **CLI auth:** `porter hub login` uses Better Auth's **device authorization** flow. The CLI shows a code, you approve it in the browser, and the token goes into the OS keychain. `PORTER_HUB_TOKEN` works for CI and servers.

## CLI integration (new porter commands)

| Command | What it does |
|---|---|
| `porter hub login [--url]` / `logout` / `whoami` | Sign in, sign out, show the signed-in user |
| `porter hub link <project-slug>` | Writes `[hub] url, project` into `porter.toml` (no secrets) |
| `porter hub push [--message]` | Snapshots the project without media, uploads missing media by hash, creates a new version. Refuses if the hub has a newer version than the one last pulled or pushed, unless `--force` (an optimistic version check). |
| `porter hub pull [--version N]` | Downloads a version and its missing media into the local project. Refuses to overwrite local changes newer than the last sync unless `--force`. |
| `porter hub run <stages…> [--apply]` | Queues a remote run on the hub's current version, streams its events to the terminal (same output as a local run, including `--json`), then pulls the result version |
| `porter hub uploads` | Lists client uploads; `porter hub uploads get <id>` downloads one into the project (e.g. as the `wp-db` dump path) |

All hub commands follow the CLI contract: `--json`, exit codes, and no prompts without `--interactive`.

## Uploads

- **Mechanism:** S3 multipart uploads with presigned part URLs, so multi-GB dumps resume after a dropped connection. The browser uploader uses 16 MB parts and retries each part. Each part is hashed, and the whole file is hashed and verified on completion.
- **Limits and audit:** server-side size limits are configurable, 20 GB by default. Allowed types are by purpose. Every upload is logged in an audit table.
- **Personal data:** uploads and project data are treated as containing personal data (dumps include form submissions).
  - Encryption at rest is the object store's.
  - Retention: archived projects and their uploads are deleted after `retention_days`, by a daily job. Deletion is logged.

## Remote runs

- **Worker loop:**
  - It claims a queued run with `SELECT … FOR UPDATE SKIP LOCKED`, downloads the base version and its media into a scratch dir, writes `porter.toml`'s secrets from the project's **encrypted run secrets** (below), and runs `porter --json --project <dir> run …` or the individual stages.
  - It reads stdout line by line into `run_event` and notifies listeners through Postgres `LISTEN/NOTIFY`, which the web service relays as server-sent events.
  - On exit it uploads the result as a new version and records the exit code.
- **Concurrency:** one run per project at a time; the worker's own concurrency is set by env.
- **Cancel:** sends SIGINT to the porter process, which exits 3 and is resumable. The result is still saved as a version.
- **Run secrets:** destination API keys and WordPress credentials needed by remote runs.
  - Stored per project, encrypted with a server key (`HUB_SECRET_KEY`), entered in the UI by developers.
  - Never shown again and never written into snapshots.
  - Passed to porter as environment variables only.
- **Not on the hub:** `wp-db` (above). `crawl` is allowed, with the engine's existing crawl bounds.

## Web UI pages

- `/`: projects you can see.
- `/p/[slug]`: overview, with the pipeline state, the latest version, issue counts by kind, coverage, and a Run button for developers.
- `/p/[slug]/issues`, `/p/[slug]/urls`, `/p/[slug]/reports/[name]` (serves the stored HTML report in a sandboxed iframe), `/p/[slug]/runs/[id]` (live log over SSE), `/p/[slug]/uploads` (client upload page), `/p/[slug]/settings` (members, share links, run secrets, retention).
- `/s/[token]`: share-link viewer.

The style is plain and readable, with no Umi branding requirement in v1.

## Deployment (Dokploy)

- **Images:**
  - A multi-stage `Dockerfile` with three stages: `rust:1.97` builds porter; `node:22` builds Next (standalone); a slim runtime copies both.
  - A separate `worker` target in the same Dockerfile.
- **`docker-compose.yml`:** `web`, `worker`, `db` (postgres:17 with a volume), and `storage` (minio, optional; for an external S3 the storage service is dropped and env vars are set). Health checks are on every service. Migrations run with Drizzle on `web` start.
- **`.env.example`:**
  - `DATABASE_URL`
  - `BETTER_AUTH_SECRET`, `BETTER_AUTH_URL`
  - `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `STAFF_EMAIL_DOMAIN`
  - SMTP settings for magic links
  - `S3_ENDPOINT`, `S3_BUCKET`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`
  - `HUB_SECRET_KEY`
  - `WORKER_CONCURRENCY`
- **`hub/DEPLOY.md`:** step by step for Dokploy (create a compose app from the repo path `hub/`, set env, attach a domain with Let's Encrypt, back up the Postgres volume).
- **Nothing is deployed by Claude.** Isaac deploys.

## Code layout

`hub/` at the repo root: a pnpm workspace with `apps/web`, `apps/worker` and `packages/db` (the Drizzle schema), plus a shared `packages/porter` (spawn porter, parse NDJSON events with zod schemas matching the README's event schema v1).

## Testing

- **Unit:** the event parser against recorded NDJSON from the engine's tests; permission checks per role; share-token hashing and expiry.
- **Integration:** `docker compose` with Postgres and MinIO in CI. Push a fixture project from the CLI, run analyse remotely against the WXR fixture, pull, assert the versions and events.
- **Auth flows:** Better Auth's test utilities; the device flow is driven end to end with a fake browser approval.
- **Playwright smoke:** sign in, open a project, see the report, create a share link, open it in a fresh context.

## Calls made without Isaac (to check)

- **Stack:** TypeScript hub (Next.js 15 + Drizzle + Postgres + S3/MinIO) driving the porter binary, not a Rust server.
- **Sync model:** whole-snapshot versions with an optimistic version check, and content-addressed media. No row-level sync in v1.
- **No `wp-db` on the hub,** for server safety.
- **Auth:** staff via Google restricted to Umi's domain (plus email/password), clients via magic link, CLI via the device flow, CI via a token.
- **Access:** share links are read-only and expiring, and never expose raw data.
- **Retention:** 90 days after archive by default.
- **Run secrets:** stored encrypted per project, passed as env only.
