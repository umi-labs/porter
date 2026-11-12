# Deploying the Porter hub on Dokploy

The hub is one Docker Compose app, `hub/docker-compose.yml`:

| Service   | What it is                                                                                 | Image                               |
| --------- | ------------------------------------------------------------------------------------------ | ----------------------------------- |
| `web`     | Next.js server (UI + API). Applies database migrations on start, before it serves.         | `hub/Dockerfile`, target `web`      |
| `worker`  | Claims remote runs and spawns the `porter` CLI; sweeps uploads; daily retention.           | `hub/Dockerfile`, target `worker`   |
| `db`      | Postgres 17                                                                                | `postgres:17`                       |
| `storage` | MinIO, **optional** (compose profile `minio`). Use R2/S3 instead in production if you can. | `cgr.dev/chainguard/minio@sha256:…` |

Both app images are built on the server from the repo (Rust builds `porter-cli` in release; porter-desktop is never
built). Nothing is pushed to a registry. The first build takes 10–15 minutes, most of it the Rust compile. Allow
about 8 GB of disk for the build: the images plus about 6.5 GB of build cache (base images, cargo and pnpm caches).
The cache makes later builds fast. Reclaim it with `docker builder prune` when disk is tight.

Everything below uses `hub.example.com` for the hub and `s3.example.com` for storage. Swap in your domains.

---

## 1. Before you start

You need:

- A domain for the hub (`hub.example.com`) pointing at the Dokploy server.
- Storage, one of:
  - **R2 or AWS S3 (recommended):** a bucket, an access key that can read, write, list and delete in it, and its
    endpoint. The bucket's CORS and lifecycle are set up in [§5](#5-storage).
  - **The bundled MinIO:** a second domain (`s3.example.com`) pointing at the same server. Browsers and the
    `porter` CLI upload and download straight to storage with presigned URLs, so it must be public.
- An SMTP account for magic links and email verification ([§8](#8-smtp)).
- A Google OAuth client for staff sign-in ([§7](#7-google-oauth-staff-sign-in)). You can add it later; the password
  route works without it.

Generate the secrets now:

```sh
openssl rand -base64 32   # BETTER_AUTH_SECRET
openssl rand -base64 32   # HUB_SECRET_KEY: keep a copy somewhere safe (see below)
openssl rand -hex 24      # POSTGRES_PASSWORD (hex: it goes into a URL)
openssl rand -hex 24      # MINIO_ROOT_PASSWORD, if you use the bundled MinIO
```

`HUB_SECRET_KEY` encrypts the run secrets (destination API keys) stored in the database. If you lose it, those
secrets can't be decrypted and have to be entered again. If you change it, the same happens. Back it up along
with the database.

## 2. Create the compose app

1. In Dokploy, open (or create) a project, then **Create Service → Compose**. Name it e.g. `porter-hub`.
   Compose type: **Docker Compose**.
2. **General → Provider:** your Git provider (or plain Git). Repository: the porter repo. Branch: the one you deploy
   (e.g. `main`).
3. **Compose Path:** `./hub/docker-compose.yml`.
   The compose file builds with the repo root as the context (`context: ..`), because the worker image compiles
   `porter` from `crates/`. Dokploy clones the whole repo, so this just works. Keep the path pointing at `hub/`.
4. **Environment:** paste the variables from [§3](#3-environment-variables). Dokploy writes them to a `.env` next to
   the compose file, which compose reads.
5. **Domains:** add them as in [§4](#4-domains-and-tls).
6. **Deploy.** Watch the logs: `db` (and `storage`) turn healthy, then `web` applies migrations
   (silently; a failure exits the container with the error), turns healthy, and
   `worker` starts with `porter worker: concurrency 1, porter porter, scratch /var/lib/porter-worker`.
7. Open `https://hub.example.com/api/health`. It should say `{"ok":true}`.
8. Create the first admin ([§6](#6-the-first-admin)).

## 3. Environment variables

Required ones have no default; compose refuses to start without them (`required variable … is missing a value`).

| Variable                     | Required        | Example / default                                                                                                          | What it does                                                                                                   |
| ---------------------------- | --------------- | -------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `BETTER_AUTH_URL`            | **yes**         | `https://hub.example.com`                                                                                                  | The hub's public origin. Auth callbacks, cookies, CORS for MinIO.                                              |
| `BETTER_AUTH_SECRET`         | **yes**         | `openssl rand -base64 32`                                                                                                  | Signs sessions. At least 32 characters; the web refuses to start otherwise.                                    |
| `HUB_SECRET_KEY`             | **yes**         | `openssl rand -base64 32`                                                                                                  | 32 bytes, base64. Encrypts run secrets (AES-256-GCM). Web and worker refuse to start without a valid one.      |
| `POSTGRES_PASSWORD`          | **yes**         | `openssl rand -hex 24`                                                                                                     | Password for the bundled Postgres. Keep it URL-safe (hex).                                                     |
| `S3_PUBLIC_ENDPOINT`         | **yes**         | `https://s3.example.com` (MinIO), or the same as `S3_ENDPOINT` for R2/S3                                                   | Storage URL as browsers and the CLI reach it. Presigned URLs are signed for this host.                         |
| `COMPOSE_PROFILES`           | for MinIO       | `minio`                                                                                                                    | Includes the bundled MinIO. Leave unset for R2/S3.                                                             |
| `MINIO_ROOT_USER`            | for MinIO       | `porter`                                                                                                                   | MinIO admin user; also the default `S3_ACCESS_KEY`.                                                            |
| `MINIO_ROOT_PASSWORD`        | for MinIO       | `openssl rand -hex 24`                                                                                                     | MinIO admin password (8+ chars); also the default `S3_SECRET_KEY`.                                             |
| `S3_ENDPOINT`                | for R2/S3       | default `http://storage:9000`; R2: `https://<account>.r2.cloudflarestorage.com`; AWS: `https://s3.eu-west-2.amazonaws.com` | Storage URL as the hub's containers reach it.                                                                  |
| `S3_ACCESS_KEY`              | for R2/S3       | default `$MINIO_ROOT_USER`                                                                                                 |                                                                                                                |
| `S3_SECRET_KEY`              | for R2/S3       | default `$MINIO_ROOT_PASSWORD`                                                                                             |                                                                                                                |
| `S3_BUCKET`                  | no              | `porter-hub`                                                                                                               | Created on first use if missing (if the key may create buckets).                                               |
| `S3_REGION`                  | no              | `us-east-1`; R2: `auto`; AWS: the bucket's region                                                                          |                                                                                                                |
| `DATABASE_URL`               | no              | `postgres://porter:$POSTGRES_PASSWORD@db:5432/porter`                                                                      | Set only to use an external Postgres 17 (see the PgBouncer caveat in [§12](#12-caveats)).                      |
| `GOOGLE_CLIENT_ID`           | no              | `…apps.googleusercontent.com`                                                                                              | Turns on "Continue with Google" for staff.                                                                     |
| `GOOGLE_CLIENT_SECRET`       | with Google     |                                                                                                                            |                                                                                                                |
| `STAFF_EMAIL_DOMAIN`         | with Google     | `umidigital.co.uk`                                                                                                         | The Workspace domain. Only verified `@<domain>` Google accounts can sign in, and they become staff.            |
| `STAFF_ORG_SLUG`             | no              | `umi`                                                                                                                      | Slug of the one staff organization. Don't change it after the first sign-in.                                   |
| `SMTP_HOST`                  | yes in practice | `smtp.postmarkapp.com`                                                                                                     | Without mail, magic links and email verification can't be sent.                                                |
| `SMTP_PORT`                  | no              | `587`                                                                                                                      | 587 = STARTTLS, 465 = TLS.                                                                                     |
| `SMTP_USER`, `SMTP_PASSWORD` | usually         |                                                                                                                            |                                                                                                                |
| `SMTP_FROM`                  | yes with SMTP   | `Porter <porter@example.com>`                                                                                              | Must be a sender your SMTP provider allows.                                                                    |
| `WORKER_CONCURRENCY`         | no              | `1`                                                                                                                        | Remote runs at once (never two on the same project). Each run is a `porter` process; size the server to match. |
| `UPLOAD_MAX_BYTES`           | no              | `21474836480` (20 GiB)                                                                                                     | Largest client upload.                                                                                         |
| `HUB_SNAPSHOT_MAX_BYTES`     | no              | `2147483648` (2 GiB)                                                                                                       | Largest extracted snapshot the hub will open.                                                                  |

Set inside the images (don't set these yourself): `NODE_ENV=production`, `HUB_CACHE_DIR`, `WORKER_SCRATCH_DIR`,
`PORTER_BIN`, `MIGRATIONS_DIR`, `HOSTNAME`, `PORT`.

## 4. Domains and TLS

In the compose app's **Domains** tab:

| Host                                  | Service   | Container port | HTTPS                             |
| ------------------------------------- | --------- | -------------- | --------------------------------- |
| `hub.example.com`                     | `web`     | `3000`         | on, certificate **Let's Encrypt** |
| `s3.example.com` (bundled MinIO only) | `storage` | `9000`         | on, certificate **Let's Encrypt** |

Path `/`, no strip-path. Redeploy after adding domains so Traefik picks up the labels. Dokploy issues and renews the
certificates; the DNS A/AAAA records must point at the server first.

Nothing publishes host ports; Traefik reaches the containers over Dokploy's network. Don't add a domain for `db`.
The MinIO console (port 9001) isn't exposed; use `mc` from inside the container if you need it.

Uploads can be large (client database dumps, up to 20 GiB by default). They go browser → storage in 16 MiB parts,
not through the hub, so no proxy body-size settings are needed for the hub's domain.

## 5. Storage

The hub stores everything under `projects/<project-id>/` in one bucket: snapshots, media and client uploads.
**The bucket must be this hub's alone.** Retention empties any `projects/<id>/` prefix whose project isn't in this
hub's database, so a second hub (or a staging copy) sharing the bucket would lose its data.

Browsers PUT upload parts straight to storage, from the hub's origin. Storage therefore needs:

- **CORS** for the origin `https://hub.example.com`:
  - methods `PUT` and `GET`
  - allowed request headers `x-amz-checksum-sha256`, `content-length`, `if-none-match` and `content-type`
  - exposed header `ETag`
- A **lifecycle rule** that aborts incomplete multipart uploads after 7 days.

### Bundled MinIO

`docker-compose.yml` sets both:

- `MINIO_API_CORS_ALLOW_ORIGIN=$BETTER_AUTH_URL`. MinIO allows any request header and exposes `ETag` for the origins
  listed.
- `MINIO_API_STALE_UPLOADS_EXPIRY=168h`, which makes MinIO abort multipart uploads after 7 days.

Then:

- Give it a domain ([§4](#4-domains-and-tls)) and set `S3_PUBLIC_ENDPOINT=https://s3.example.com`.
- `S3_ENDPOINT` stays `http://storage:9000`. The containers talk to MinIO directly; only presigned URLs use the
  public host.

**The MinIO image.** `minio/minio` is no longer on Docker Hub. The compose file pins `cgr.dev/chainguard/minio` by
digest. Chainguard's free tier only guarantees the `latest` tag, so an old digest can disappear and the deploy then
fails to pull. To re-pin:

```sh
docker pull cgr.dev/chainguard/minio:latest
docker inspect --format '{{index .RepoDigests 0}}' cgr.dev/chainguard/minio:latest
# put the printed cgr.dev/chainguard/minio@sha256:… into docker-compose.yml (storage.image) and in
# docker-compose.dev.yml, commit, redeploy
```

MinIO keeps its data on the `miniodata` volume, so a new image version reads the same data.

### Cloudflare R2 or AWS S3 (recommended)

Leave `COMPOSE_PROFILES` unset (no `storage` service), and set:

```sh
S3_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com   # AWS: https://s3.<region>.amazonaws.com
S3_PUBLIC_ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
S3_REGION=auto                                             # AWS: the bucket's region
S3_BUCKET=porter-hub
S3_ACCESS_KEY=…
S3_SECRET_KEY=…
```

Create the bucket yourself. An R2 "Object Read & Write" token can't create buckets. Then add its CORS policy (R2:
bucket → Settings → CORS policy; AWS: bucket → Permissions → CORS):

```json
[
  {
    "AllowedOrigins": ["https://hub.example.com"],
    "AllowedMethods": ["PUT", "GET"],
    "AllowedHeaders": ["x-amz-checksum-sha256", "content-length", "if-none-match", "content-type"],
    "ExposeHeaders": ["ETag"],
    "MaxAgeSeconds": 3600
  }
]
```

And the lifecycle rule. R2: bucket → Settings → Object lifecycle rules → "Abort incomplete multipart uploads", 7
days. AWS (`aws s3api put-bucket-lifecycle-configuration --bucket porter-hub --lifecycle-configuration file://l.json`):

```json
{
  "Rules": [
    {
      "ID": "abort-incomplete-multipart",
      "Status": "Enabled",
      "Filter": {},
      "AbortIncompleteMultipartUpload": { "DaysAfterInitiation": 7 }
    }
  ]
}
```

The hub relies on S3 checksums (`x-amz-checksum-sha256`) and conditional writes (`If-None-Match: *`). MinIO, R2 and
AWS S3 support both. Other S3-compatible stores may not.

## 6. The first admin

Staff are the members of the staff organization (`STAFF_ORG_SLUG`). Only staff can create projects. There are two
ways in.

**Google (staff domain).** Once Google is set up ([§7](#7-google-oauth-staff-sign-in)), anyone with a verified
`@STAFF_EMAIL_DOMAIN` Google account becomes staff on their first "Continue with Google". Nothing else is needed.

**Email and password (fallback, or before Google is set up).** Sign-up is by staff invitation only. Make one from
the worker container. In Dokploy, open the compose app → **Docker** tab (or a shell on the server):

```sh
docker compose -p <dokploy-app-name> exec worker \
  node_modules/.bin/tsx apps/worker/src/invite-staff.ts you@example.com
# invited you@example.com to the staff org until …
```

It prints the invitation id and a sign-up link:

```text
invited you@example.com to the staff org until …
invitation id: 6f1c…  (the sign-up secret: send it to them privately)
sign up at: https://hub.example.com/sign-up?invitation=6f1c…
or: curl -sS https://hub.example.com/api/auth/sign-up/email -H 'content-type: application/json' \
       -H 'origin: https://hub.example.com' -d '{"email":"you@example.com","password":"<12+ characters>","name":"<name>","invitationId":"6f1c…"}'
```

(In a local checkout with `DATABASE_URL` in `hub/.env`: `pnpm -C hub admin:invite-staff you@example.com`.) The
invitation lasts 7 days. Running the command again replaces it, with a new id. It also creates the staff
organization if it doesn't exist yet.

**The invitation id is the secret.** Sign-up needs it, together with exactly the invited email, so knowing an
invited address isn't enough. Pass the link (or the id) to the invitee privately; don't post it in a shared channel.
The invitee opens the link and chooses a password (12+ characters), or uses the curl form.

The hub then emails a verification link (SMTP must work). Click it, then sign in at
`https://hub.example.com/sign-in` → "Use a password instead". The first sign-in with the verified email accepts
the invitation the account signed up with, and you're staff.

**If you get a verification email you didn't ask for, don't click it.** The same applies if sign-up says the
account already exists. Someone else may have created an account on your address. Ask an admin to delete that user
(`delete from "user" where email = '…'`) and run `invite-staff` again.

No working SMTP yet? As a one-off, you can mark the address verified in the database instead:

```sh
docker compose -p <app> exec db psql -U porter -d porter \
  -c "update \"user\" set email_verified = true where email = 'you@example.com'"
```

**The first project.** Signed in as staff, the home page has a **New project** form (name, and an optional slug
made from the name). You become its owner and land on its settings. Or from a porter project directory:
`porter hub link <slug> --url https://hub.example.com --create --name "Acme"`, which creates it and writes `[hub]`
into porter.toml. Then `porter hub push`.

Staff can then invite clients per project (Settings → Members). Clients sign in with a magic link.

**Roles** (per project):

| Role        | View | Raw data, issues, runs | Push, pull, run | Pull only | Upload | Download uploads | Share | Members | Secrets |
| ----------- | ---- | ---------------------- | --------------- | --------- | ------ | ---------------- | ----- | ------- | ------- |
| `owner`     | yes  | yes                    | yes             |           | yes    | yes              | yes   | yes     | yes     |
| `developer` | yes  | yes                    | yes             |           | yes    | yes              | yes   |         | yes     |
| `viewer`    | yes  | yes                    |                 | yes       |        |                  |       |         |         |
| `client`    | yes  |                        |                 |           | yes    |                  |       |         |         |

Downloading client uploads needs push (owner, developer): dumps carry personal data, so viewers can't fetch them.
Clients see only their own uploads.

## 7. Google OAuth (staff sign-in)

In the Google Cloud console, under a project owned by the Workspace:

1. **APIs & Services → OAuth consent screen:** User type **Internal**. Then only accounts in your Workspace can
   sign in at all. App name "Porter", support email, and the authorised domain `example.com`. Scopes: the defaults
   (`openid`, `email`, `profile`).
2. **Credentials → Create credentials → OAuth client ID:** type **Web application**.
   - Authorised JavaScript origins: `https://hub.example.com`
   - Authorised redirect URI: `https://hub.example.com/api/auth/callback/google`
3. Set `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` and `STAFF_EMAIL_DOMAIN` (e.g. `umidigital.co.uk`) and redeploy.

The hub checks the domain too. Google sends it as the `hd` claim, and the hub also checks that the email ends in
exactly `@STAFF_EMAIL_DOMAIN` and is verified. Google accounts outside the domain are refused. Clients never use
Google; they use magic links.

## 8. SMTP

The web sends two kinds of mail: client magic links (valid for 5 minutes) and email verification for password
sign-ups. Set `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `SMTP_PASSWORD` and `SMTP_FROM`. Any SMTP
provider works (Postmark, SES, Mailgun, Google Workspace SMTP relay). Port 587 uses STARTTLS, port 465 implicit TLS.

To check it, invite a client to a project, then ask for a magic link with that email on the sign-in page. A failed
send is logged by `web` as `magic link mail failed: …`. The page doesn't say, on purpose, so it can't reveal which
emails are invited.

## 9. Backups and restore

**What to back up:**

- the Postgres database (everything except file contents)
- storage (the `miniodata` volume, or your R2/S3 bucket)
- `HUB_SECRET_KEY`

The `cache` and `scratch` volumes are disposable.

**Postgres, with Dokploy.** Dokploy's scheduled backups target its own database services. For a compose app, use
**Schedules** (compose app → Schedules → add a job on the `db` service), e.g. daily at 03:00:

```sh
pg_dump -U porter -d porter -Fc > /var/lib/postgresql/data/backup-latest.dump
```

then copy it off the server (Dokploy **Volume Backups** of `dbdata` to an S3 destination, or your own job). Or run
it from the server's crontab:

```sh
# /etc/cron.d/porter-hub-backup — daily dump, kept 14 days
0 3 * * * root docker exec $(docker ps -qf name=porter-hub.*-db-1) pg_dump -U porter -d porter -Fc \
  > /var/backups/porter-hub/$(date +\%F).dump && find /var/backups/porter-hub -mtime +14 -delete
```

Ship `/var/backups/porter-hub` off the box (rclone/restic to R2/S3).

**Restore** (into an empty database, with the app stopped so nothing writes meanwhile):

```sh
docker compose -p <app> stop web worker
docker compose -p <app> exec -T db psql -U porter -d postgres \
  -c 'drop database porter with (force)' -c 'create database porter owner porter'
docker compose -p <app> exec -T db pg_restore -U porter -d porter --no-owner < 2026-11-12.dump
docker compose -p <app> start web worker   # web applies any newer migrations on start
```

**MinIO data:** back up the `miniodata` volume (Dokploy Volume Backups), or mirror the bucket:
`mc mirror local/porter-hub <remote>/porter-hub` from inside the container. With R2/S3, use the provider's
versioning or replication. With versioning on, retention's deletes only add delete markers. Add a noncurrent-version
expiration rule (e.g. 30 days), or deleted projects' data stays in the bucket.

A database restored without the matching objects shows versions whose snapshots fail to load. Objects without
database rows are harmless but take space, with one exception: retention empties the storage of a project this
database has retired, which it knows from that project's `retention.delete` row in `audit_log`. Keep those audit
rows. Objects of projects the database has never heard of are left alone. Restore both from about the same time.

**Stop the worker before restoring, or before pointing it at an empty or different database** (`docker compose -p
<app> stop worker`), and start it only once the restore is complete. Retention runs when the worker starts; against
the wrong database it can delete projects whose rows say they're archived and past retention.

## 10. Volumes

| Volume      | Service      | Contents                                                     | Back up? |
| ----------- | ------------ | ------------------------------------------------------------ | -------- |
| `dbdata`    | db           | Postgres data                                                | **yes**  |
| `miniodata` | storage      | bucket contents (bundled MinIO only)                         | **yes**  |
| `cache`     | web + worker | extracted snapshots for the project views; rebuilt on demand | no       |
| `scratch`   | worker       | per-run working dirs, deleted after each run                 | no       |

`cache` is shared by web and worker: the worker's daily retention clears a deleted project's extracted snapshots
from it. Each worker needs its **own** `scratch`. Never point workers on different databases at the same
scratch dir: at startup each worker clears scratch dirs of runs that aren't running in _its_ database.

**Scaling:** raise `WORKER_CONCURRENCY` on the one worker rather than adding compose replicas. Replicas would share
the `scratch` volume, and a worker cleaning up after a lost run could then signal another container's process by
pid. A second worker needs its own scratch volume (mounted at the same path).

## 11. Upgrading and rollback

**Upgrade:** merge to the deployed branch and press **Deploy** (or turn on auto-deploy). Dokploy rebuilds both
images and recreates the containers. `web` applies new migrations before serving. They're advisory-locked, so
several replicas starting at once take turns. `worker` gets SIGTERM, interrupts running porter processes and saves
their results as versions (runs end `failed: interrupted`), with up to 60 s to finish (`stop_grace_period`).
Re-run them after the upgrade.

**Keep the worker's porter current.** A project opened by a newer porter can't be read by an older one (project
schema v6 onwards). Keep the worker's porter at least as new as any client porter that pushes, and upgrade
clients' porter before they pull a worker result.

**Rollback:** redeploy the previous commit (Dokploy → Deployments, or reset the branch). Migrations only go
forward. An older image runs against a newer schema as long as the migrations it skips only _added_ things, which
is the rule for this repo. If a release notes a destructive migration, take a database backup first and restore it
to roll back.

Images are rebuilt on the server for each deploy. Prune old ones now and then (`docker image prune`); Dokploy has a
cleanup setting for this.

## 12. Caveats

- **Run secrets and the developer role.** Developers can store destination API keys as run secrets, which the hub
  never shows again. But a developer can also edit the project's `porter.toml` (push a version) and point the
  destination URL at a server they control. The next remote run then sends the API key there. Only give the
  `developer` and `owner` roles to trusted staff. Clients and viewers can't do this.
- **porter runs as the worker's user.** Inside the worker container, `porter` runs as the same unprivileged user
  (`node`, uid 1000) as the worker. The hub passes it only `PATH`, `HOME` and `LANG` plus that run's secrets, but
  as the same uid it _could_ read the worker's own environment (`/proc/<pid>/environ`: `HUB_SECRET_KEY`,
  `DATABASE_URL`, the S3 keys). It only matters if porter itself were made to run hostile code. Running porter
  under its own uid would need the worker to hold `CAP_SETUID`/`CAP_SETGID`, so it isn't done. Hardening options,
  if you want them:
  - a sudo-less setuid helper, or the worker as root with porter dropped to a second uid;
  - `read_only: true` with `tmpfs: [/tmp]` (the volumes stay writable), `cap_drop: [ALL]` and
    `security_opt: [no-new-privileges:true]` on the worker service;
  - a separate hub for projects with especially sensitive destination keys.
- **PgBouncer.** The worker sets `statement_timeout` as a connection startup parameter. PgBouncer in transaction
  pooling mode rejects unknown startup parameters unless `ignore_startup_parameters = statement_timeout` is set.
  The bundled Postgres has no PgBouncer; this only matters with an external `DATABASE_URL`. `LISTEN/NOTIFY` (live
  run logs) also needs session pooling or a direct connection for the web. The same goes for migrations: the web
  takes a session advisory lock while it migrates, which a transaction pooler can't hold. So web's `DATABASE_URL`
  must be a direct or session-pooled connection.
- **The MinIO digest** can disappear from Chainguard ([§5](#5-storage)). R2/S3 avoids that.
- **Share tokens are in the link's path** (`/s/<token>`), so they appear in Traefik/Dokploy request logs (the
  "Requests" view). The database only stores their hash. Treat those logs as sensitive, or turn off access logging
  for `/s/`.
- **Report share links** serve the stored HTML reports as they are. Those can include issue text, which can include
  error bodies returned by the destination. Don't share report links for projects whose destination errors may
  carry sensitive data.
- **Retention:** archived projects are deleted for good `retention_days` (default 90) after archiving, by the
  worker's daily job: storage, database rows and cached snapshots. Each deletion is written to `audit_log`
  (`action = 'retention.delete'`, project id in `target_id`). An archived project takes no new pushes, runs or
  uploads (409); it can still be viewed and pulled until it's deleted, and a running run or an open upload on it can
  still be cancelled or aborted. There's no archive button yet. To archive or
  unarchive, use SQL: `update project set archived_at = now() where slug = '…'` (or `= null`). To keep a project
  longer, raise its `retention_days`. Retention deletes the rows first and storage after: if storage fails, the
  files are swept on a later pass (any prefix whose project no longer exists is emptied), and a failed row delete
  leaves the project and its files whole for the next pass.
- **Pushing while a run is going is refused** (409, naming the run): the run's result lands as the next version,
  built from its own base, and would bury the push on the next pull. Wait for the run (or cancel it), pull, then
  push.
- **A remote apply needs a dry run on record.** The hub refuses `load --apply` unless the version's project has a
  finished dry-run load ("Run a dry-run load first"); porter then checks that dry run still matches.
- **WXR files from client uploads.** `porter hub uploads get` saves into `<project>/uploads/`, which push leaves
  out. For a remote run, move the file into the project (outside `uploads/`) and point `[source] path` at it before
  pushing.
