# Porter ↔ Foundry hub integration

Foundry hub creates and runs a client's Payload site. Porter fills it with the old site's content. This connects them in three steps:

1. Porter becomes a Foundry integration. "Migrate content with Porter" on a Foundry site creates or links a Porter project, with the destination filled in.
2. Foundry shows the migration's status on the site page and posts Slack notices when a Porter run fails or an apply finishes.
3. Two follow-ups:
   - Porter staff can sign in with Foundry hub.
   - Foundry's MCP server gets Porter tools.

## Contract (Porter side)

### Service keys

- **Key format:** `phk_<32 random bytes, base64url>`. Porter stores only its sha256, plus a display prefix.
- **Table:** `service_key`, with columns `id`, `name`, `prefix`, `key_hash` (unique), `user_id`, `created_by`, `last_used_at`, `revoked_at` and `created_at`.
- **Bot user:** each key acts as its own bot user, such as "Foundry hub". The bot is a member of the staff org, so the existing permission checks apply unchanged: it is staff, and it owns the projects it creates.
- **Minting:** `pnpm admin:service-key <name>` creates the bot user and the key, and prints the key once.
- **Auth:** `Authorization: Bearer phk_…` on any `/api/*` route resolves to the bot user. Better Auth session bearers are unaffected, because they never start with `phk_`.

### `POST /api/foundry/projects`: create or link

This is idempotent on `ref`.

```json
{
  "ref": "turquoise/main-site",
  "name": "Turquoise",
  "foundry_url": "https://hub.example.com/sites/turquoise/main-site",
  "members": [{ "email": "dev@umi.digital", "role": "owner" }],
  "destination": { "url": "https://www.turquoise.com", "api_key": "…" }
}
```

- **Access:** staff only (a service key, or a signed-in staff user).
- **Project:** creates the Porter project, or finds it by `foundry_ref`, and stores `foundry_ref` and `foundry_url`.
- **Members:** each `members` entry gets `addMember` if the person has a Porter account, or `createInvitation` if not. Existing roles are left alone.
- **Destination:** commits a new version whose `porter.toml` sets `[destination]` (Payload, that URL, `api_key_env = "PAYLOAD_API_KEY"`). The key is stored as the `PAYLOAD_API_KEY` run secret. Nothing else in `porter.toml` changes, and the source is left for the Setup tab.
- **Response:** `{ slug, url, created }`, where `url` is the Porter project page.

### `GET /api/foundry/status?ref=<org>/<project>`

The caller needs `view` on the linked project (a 404 otherwise). Porter returns:

```json
{
  "slug": "turquoise", "url": "https://porter…/p/turquoise",
  "source": { "adapter": "wordpress", "kinds": ["wp-rest", "crawl"] },
  "destination_host": "www.turquoise.com",
  "version": { "number": 5, "created_at": "…" },
  "stages": [{ "stage": "analyse", "state": "clean", "stale": null }],
  "issues": { "error": 0, "warning": 3 },
  "dry_run": true, "ever_written": false,
  "last_run": { "id": "…", "state": "finished", "stages": "all", "apply": false, "exit_code": 0, "finished_at": "…" }
}
```

It returns `{ linked: false }` when no project has that ref.

### Run webhook (Porter → Foundry)

- **Config:** Porter has two settings, `FOUNDRY_WEBHOOK_URL` (Foundry's `/api/webhooks/porter?c=<connectionId>`) and `FOUNDRY_WEBHOOK_TOKEN`.
- **When:** when a run ends on a project with a `foundry_ref`, the worker POSTs the payload below with `Authorization: Bearer <token>`.
- **Retries:** none. It uses a 10 s timeout and logs failures; the status endpoint is the source of truth.

```json
{ "event": "run.finished", "ref": "turquoise/main-site", "porter_url": "…/p/turquoise/runs/<id>",
  "run": { "id": "…", "state": "failed", "stages": ["load"], "apply": true, "exit_code": 1, "error": "…", "result_version": 6 } }
```

### Sign in with Foundry (OIDC)

- Porter uses Better Auth's `genericOAuth` with `providerId: "foundry"`, discovery at `FOUNDRY_OIDC_ISSUER/.well-known/openid-configuration`, plus `FOUNDRY_OIDC_CLIENT_ID` and `FOUNDRY_OIDC_CLIENT_SECRET`.
- The same rule as Google sign-in applies: only verified staff-domain emails, and no sign-up through it. Existing accounts link by email.
- The button shows only when it's configured.

## Foundry side

- **Service:** `porter` in `SERVICES`, with `SERVICE_CONFIG.porter = [baseUrl]`.
  - Catalogue entry: category `Content`, levels umi/client/project, project types `payload-site`, `auth: token`, `secretLabel: 'Service key'`, not client-completable.
  - Verified by `GET <baseUrl>/api/projects`, which must return 200.
  - Also needs: an icon, a contract type and `safe-url` handling.
- **Migrate content:** on the site's Integrations tab, a "Migrate content with Porter" button for people with `sites.settings` and integrations manage. It opens a dialog:
  - Environment: production by default.
  - Payload URL, prefilled `https://<envDomains[env]>`.
  - Payload API key: from Infisical `PAYLOAD_API_KEY` if present, otherwise typed in.
  - The dialog calls `POST /api/foundry/projects` with the caller's email as owner, and audits `project.porter.link`.
  - Once linked, it shows "Open in Porter".
- **Migration panel:** `migration` in `PANELS` (site surface, `needs: ['integrations.read']`).
  - It is fed live from `GET /api/foundry/status`, cached per request and hidden unless `porter` is attached (`NEEDS_INTEGRATION`).
  - It shows: source and ways, a stage strip, open errors and warnings, last run, dry run or written, and "Open in Porter".
- **Webhook:** `POST /api/webhooks/porter?c=<connectionId>`, using the Dokploy pattern.
  - Bearer token compared with `config.notifyHash`; the route is added to `public-paths`.
  - It is idempotent on `run.id` (dedupe through the audit trail or a small table).
  - It maps `ref` to the project and calls `notifyLater`:
    - `porter_failed: 'Porter run failed'` for state `failed`;
    - `porter_applied: 'Porter apply finished'` for state `finished` with `apply: true`.
  - A "Set up notifications" action on the Porter integration mints the token. It shows the URL and token once, to paste into Porter's env.
- **OIDC client:**
  - Foundry's `@better-auth/oauth-provider` (inside `@better-auth/mcp`) issues an `id_token` with `email` for a confidential client registered for Porter, using `skip_consent`.
  - The client is registered by an admin script that inserts into `oauth_client`.
  - If the installed provider can't issue an `id_token`, this part stops there and says so, rather than inventing a new protocol.
- **MCP tools:**
  - `porter_status(site)`;
  - `porter_run(site, stages)`, which is a dry run only and can never apply;
  - `porter_link(site)`.
  - They run through the resolved `porter` connection, and access is checked as for other site tools (`findVisibleProject`; `findEditableProject` for run and link).

## Out of scope

- Applying a load from Foundry: apply stays a deliberate act in Porter.
- Porter calling Foundry for anything other than the webhook.
- Creating Payload API keys on Foundry sites. That is a template change (`useAPIKey` on Users), noted separately.
