# Hub: rulings, open decisions and backlog

These are taken from the build ledger on 2026-11-12. They record the calls Claude made while building the hub, what each would cost if wrong, the decisions that are Isaac's to make, and everything parked for later.

The final whole-branch review found 0 Critical and 5 Important issues. All of them were fixed in 7bbc289, 2124037, 34af8bd and 68ce82b, then re-reviewed.

## Decisions for Isaac

Two of these I've already decided provisionally. Reverse them if you disagree:
- **Push during a run (D1):** a push is refused while a remote run is queued or running on the project. Without that, the run's result would land on top of the newer push.
- **Who can download client dumps (D3):** client uploads need `push` to download, so only owners and developers can. Viewers can't, because the dumps hold personal data.

These are still open:
- **D2, who gets developer or owner:** developers can store run secrets and repoint the destination, which is a documented way to exfiltrate a key. Give those roles to trusted staff only.
- **D4, report-only share links:** they serve porter's HTML reports, and those include issue text, which can contain error bodies from the destination. Keep that, or strip the issue text?
- **D5, the audit log:** it keeps personal data (emails, filenames) after a project is deleted. Pick a retention period or scrub it. Keep the `retention.delete` rows either way, because the orphan sweep depends on them.
- **D6, worker hardening:**
  - porter runs as the worker's uid, so it can read the worker's environment, including HUB_SECRET_KEY;
  - the worker has unrestricted network egress (SSRF to cloud metadata, `web`, `storage`).

  See DEPLOY §12, and decide before the first client project.
- **D7, live checks nobody has done:**
  - Dokploy compose details (`.env`, the optional MinIO profile)
  - Google OAuth
  - `porter hub login` and `logout` through the device flow
  - MinIO behind Traefik with presigned URLs
  - R2's checksum, If-None-Match and CORS behaviour
- **D8, CLI and CI tokens:** they are 7-day rolling sessions with no UI to list or revoke them. Offboarding means removing the user.
- **D9, old versions:** they are never pruned until the project is archived and retention removes it.
- **D10, storage:** launch on MinIO (a pinned Chainguard digest that can disappear) or R2 (recommended).

Other things to know:
- There is no archive button. Archiving is done in SQL for now, so retention only acts on projects someone has archived by hand.
- About 6.5 GB of Docker build cache from the production smoke run is still on disk. Free it with `docker builder prune`.

## Rulings
- | Tasks | Produces → consumes | Finding | Ruling |
- | T1 → all | Drizzle schema incl. Better Auth tables | Better Auth CLI generates its own schema | Ruling: generate Better Auth tables with its CLI into packages/db, hand-write porter tables alongside — cost: none |
- | T2 ↔ T3 | roles | Better Auth organization roles vs per-project roles | Ruling: org membership = staff; per-project roles in our own project_member table — cost: none |
- | T4/T5 | snapshot reader | reading project.db in Node | Ruling: better-sqlite3 read-only on a cached download |
- | T9 | docker compose build + Playwright | disk ~11 GB, Docker VM | Ruling: build images only if host disk ≥ 10 GB free at that moment; else skip and record (Isaac to run) — cost: unverified images |
- | env | no external services | Google OAuth / SMTP unavailable | Ruling: tests use Better Auth test utilities and a fake SMTP (log transport); real OAuth documented only |
- Task 1: Ruling: better-auth pinned to 1.7.21 (1.7.22 broken on npm); dev ports 55439/59100/59101 to avoid local clashes; MinIO bucket creation deferred to T4
- Task 1: Ruling (carry to T4): MinIO healthcheck `mc ready local` unverified + image tag unpinned — fix when storage is first used
- Ruling: typescript@^6 devDep in apps/web for Next 15 — accepted; Next 16 upgrade later — cost if wrong: one extra devDep.
- Ruling: allowUserToCreateOrganization=false; single staff org slug `umi` (STAFF_ORG_SLUG), created server-side, Google verified staff-domain users auto-added as member; canApproveDevice checks that org; approval hook fails closed — why: closes Critical, satisfies spec "staff belong to Umi org" — cost if wrong: org slug rename.
- Ruling: matrix owner=all; developer=view,view_raw,push,pull,run,share,upload,manage_secrets; viewer=view,view_raw,pull; client=view,upload; staff org gives no implicit project access; only staff create projects — why: spec Access section (devs enter secrets; clients no raw) — cost if wrong: edit one table.
- Ruling T4: per-project key prefixes; S3 checksum-verified media PUTs; commit verifies objects then version check in one tx with unique(project_id,number); presign 15 min; MinIO pinned+healthcheck+auto bucket — why: integrity + retention by prefix — cost if wrong: key layout migration.
- Ruling: snapshot_sha256 at init + signed checksum; S3_PUBLIC_ENDPOINT for presigning; media-urls batch endpoint ≤1000; init cap 200k hashes — cost if wrong: one extra CLI round trip per 1000 media.
- Ruling T5: issues/urls need view_raw (client 403); project-readonly share = overview+issue list (no field data)+urls+reports, never items/raw/submissions/media; report scope = reports only; tokens 32B base64url sha256-stored, 14d default/90d max; safe tar extraction (no links, no .., 2GB cap) into HUB_CACHE_DIR — cost if wrong: share scope tweak.
- Ruling: streaming extraction + single-flight; only no-such-table/column -> 'not available'; share links never show issue messages (spec deviation: tighter than 'issue list' — cost: share viewers see less detail).
- Ruling T6: upload perm = owner/developer/client; download needs pull (clients never download); clients list only own uploads; key projects/<id>/uploads/<uploadId>; per-purpose extension allowlist; verifyUpload(id) in shared package, web fires it after complete, Task 7 worker sweeps stuck 'verifying'; UPLOAD_MAX_BYTES default 20GB checked at create and at completion — cost if wrong: perms table tweak.
- Ruling T7: apply needs confirm_host == destination host; one run per project (partial unique index); spawned env = allowlist + secrets only; wp-db stripped via TOML parser with access-gap note; result version always appended (latest+1) with warning if base is stale; exit map 0/2 finished, 3 cancelled|failed, 1/crash failed; SSE needs view_raw, shared LISTEN per process; worker sweeps verifying uploads (>10 min) and stale uploading (>7 d) — cost if wrong: stale-base versions need manual pick.
- Task 7: implemented 1f15b63. Ruling: accepted result pack keeping all top-level snapshot files except media/ (needed for WXR source files) — cost: snapshot carries source exports. Review dispatched.
- Ruling: keep env-var secrets (api_key_env arbitrary) with extended denylist; per-run uid -> T9 note.
- Ruling T8: auth header only to hub origin, no cross-origin redirects; snapshot excludes media/, .porter-hub.json, dot-entries; push --force = re-init on hub latest; pull refuses on project.db/porter.toml hash drift; run --apply needs --confirm-host; token keyring service porter-hub or PORTER_HUB_TOKEN, never written to file — cost if wrong: flag tweaks.
- Task 8: implemented 8edd394, 8f72260; live check vs demo passed (whoami, link, pull, push v3, run analyse -> v4, uploads get). Ruling: exclude uploads/ from push snapshots (client dumps must not be re-pushed) — fold into fix round. Review dispatched.
- Ruling: VACUUM INTO for snapshot + consistent-copy hash; .tmp/.bak staged swap with crash recovery; push refuses symlinks/special files.
- Task 9: dispatched (base 34dac71, opus). Ruling: prod compose = docker-compose.yml, dev services -> docker-compose.dev.yml; migrations on web start under advisory lock; /api/health; admin:invite-staff script for first password user; build+smoke on project porter-hub-prod-test port 3200 if >=10GB free, then down -v and remove images.
- Task 9 review: spec ✅, changes needed (0C/2I/10M: retention not transactional/locked; staff-invite email squatting -> staff). Fix round 1 dispatched. Ruling: invite id required for staff password sign-up; retention select-for-update re-check per project.
- Ruling: D1 adopted (refuse push while run active); D3 adopted conservatively (upload downloads need push, not pull) — flag to Isaac. D2,D4-D10 for Isaac.

## Parked (backlog)

- Parked -> Task 3: unique (organization_id,user_id) index on member (check-then-insert race); M1 invitation-enumeration on sign-up accepted (rate-limited in prod); M2 unverified-squatter on invited email accepted (admin deletes); OAuth callback path untested (id-token path tested).
- Task 3 re-review: all addressed. Parked minors M3/M4/M5/M8/M9 (see task-3-report.md Fix round 1) -> rulings doc deferred list.
- Task 4 re-review: all addressed. Parked minors: media-urls ignores upload TTL; stray-hash 400 echoes hashes; CLI must treat 412 as uploaded (added to task-4-report for T8); orphan S3 sweep -> T9 retention; chainguard digest fragility -> DEPLOY.md note for Isaac; media_object.content_type null.
- Task 5 re-review: all addressed. Parked minors: readFileSync in lib/report.ts (carried into T6 dispatch); temp-dir rm can race in-flight open (stray empty file); no committed tests for pax '..', S3-error cleanup, single-flight clear, checksum mismatch; M4 (failed-load memo, stale .tmp sweep) -> T9; M7 page-module tests -> smoke.
- Carry to T7: worker sweeps 'verifying' uploads via @porter-hub/storage verifyUpload; expire abandoned 'uploading'. Carry to T9: S3 CORS (ETag exposed) + abort-incomplete-multipart lifecycle in DEPLOY.md.
- Task 6 fix round 1: 72739a4; re-review dispatched. Carry to T9: CORS must allow x-amz-checksum-sha256 + content-length headers.
- Task 6 re-review: all addressed. Parked minors: N1 idle timer fires under consumer back-pressure (no stalling consumer today); N2 open-upload cap check-then-insert overshoot; N3 no idle-timeout test.
- Task 7 fix round 1: e43c2b3 (migration 0007 run.worker_id; guard uses FOR NO KEY UPDATE to avoid FK deadlock). Carry to T9 DEPLOY: per-worker WORKER_SCRATCH_DIR; M7 repointed destination can exfiltrate secret -> spec/docs note; porter under own uid. Re-review dispatched.
- Task 7 fix round 2: cf5f2a9. Carry to T9: Docker image needs procps; porter binary name short (<15 chars). Re-review dispatched.
- Task 8 re-review 1: C1/I1/I2/I4 fixed (VACUUM INTO hash verified stable); open I3 (.bak partial delete -> bad recovery), minors I5/M8/I6. Open-db heuristic parked with code note asking for Store::open shared lock. Fix round 2 dispatched.
- Task 8 re-review 2: all addressed. Parked nits: interrupt flag read once; uploads id/state/purpose + whoami fields unclean; no interrupt-flag test; device-flow login + hub logout untried live (Isaac).
- Task 9 re-review: all addressed. Parked: cancel blocked on archived project (carry to hub final fix wave — cancelling must always be possible); stale signed-up invitation rows linger (not exploitable); members/share/secrets writes allowed on archived; M10 worker image size.
- Hub final re-review: all fixed except N1 (Important: orphan sweep could empty bucket if worker runs against wrong/restored DB) + N2 minor. Adjudicated N1 load-bearing (destructive) -> residual fix: sweep only ids with retention.delete audit row; plus N2, D1 recheck at commit, run-commit statement_timeout. Concern 1 parked.
