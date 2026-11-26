import { readFileSync } from "node:fs";
import { join } from "node:path";
import { project } from "@porter-hub/db";
import { applyDestination, destinationHost, PAYLOAD_KEY_ENV, readSetup } from "@porter-hub/porter/config";
import { eq } from "drizzle-orm";
import { z } from "zod";
import type { Db } from "./permissions";
import * as svc from "./projects";
import { type Actor, access, HttpError, parse } from "./projects";
import { listRuns } from "./runs";
import { commitToml } from "./setup";
import { latestSnapshot, overview, withDb } from "./snapshot-reader";

/** Foundry hub's site ref: `<org>/<project>`. */
const REF = /^[a-z0-9]+(-[a-z0-9]+)*\/[a-z0-9]+(-[a-z0-9]+)*$/;
const https = z
  .string()
  .trim()
  .max(2000)
  .refine((v) => {
    try {
      const u = new URL(v);
      return /^https?:$/.test(u.protocol) && !u.username && !u.password;
    } catch {
      return false;
    }
  }, "must be an http(s) URL without user:pass@");

const schemas = {
  link: z.object({
    ref: z.string().regex(REF, "must be <org>/<project>"),
    name: z.string().trim().min(1).max(120),
    foundry_url: https.optional(),
    members: z
      .array(
        z.object({
          email: z.string().trim().toLowerCase().pipe(z.email().max(254)),
          role: z.enum(["owner", "developer", "viewer"]),
        }),
      )
      .max(20)
      .default([]),
    destination: z
      .object({
        url: https,
        api_key: z.string().min(1).max(8192).optional(),
      })
      .optional(),
  }),
  ref: z.string().regex(REF, "must be <org>/<project>"),
};

const origin = () => (process.env.BETTER_AUTH_URL ?? "").replace(/\/$/, "");
const projectUrl = (slug: string) => `${origin()}/p/${slug}`;

const byRef = async (db: Db, ref: string) =>
  (await db.select().from(project).where(eq(project.foundryRef, ref)))[0];

/** Foundry's site slug style: `<org>-<project>`, or `<org>` when they're the same. */
const slugFor = (ref: string) => {
  const [org, p] = ref.split("/");
  return org === p ? org : `${org}-${p}`.slice(0, 60).replace(/-+$/, "");
};

/**
 * Creates the Porter project for a Foundry site, or finds the one already linked to it (idempotent on `ref`).
 * Adds the given members (an invitation for anyone without a Porter account) and, with `destination`, commits a
 * version whose porter.toml points at that Payload site, with the API key as the PAYLOAD_API_KEY run secret.
 * Staff only (Foundry calls it with its service key).
 */
export async function linkFoundry(db: Db, actor: Actor, input: unknown) {
  const body = parse(schemas.link, input);
  if (!(await svc.isStaff(db, actor))) throw new HttpError(403, "Only staff can link Foundry sites");

  let p = await byRef(db, body.ref);
  const created = !p;
  if (!p) {
    // A Porter project may already use the slug (made by hand): take the next free one.
    let slug = "";
    for (let i = 1; i <= 20 && !slug; i++) {
      const candidate = i === 1 ? slugFor(body.ref) : `${slugFor(body.ref).slice(0, 56)}-${i}`;
      try {
        slug = (await svc.createProject(db, actor, { name: body.name, slug: candidate })).slug;
      } catch (e) {
        if (!(e instanceof HttpError && e.status === 409)) throw e;
      }
    }
    if (!slug) throw new HttpError(409, "No free project slug for this site");
    await db
      .update(project)
      .set({ foundryRef: body.ref, foundryUrl: body.foundry_url ?? null })
      .where(eq(project.slug, slug));
    p = await byRef(db, body.ref);
  } else if (body.foundry_url && body.foundry_url !== p.foundryUrl) {
    await db.update(project).set({ foundryUrl: body.foundry_url }).where(eq(project.id, p.id));
  }

  for (const m of body.members) {
    try {
      await svc.addMember(db, actor, p.slug, m);
    } catch (e) {
      if (!(e instanceof HttpError)) throw e;
      if (e.status === 404) await svc.createInvitation(db, actor, p.slug, m).catch(ignoreConflict);
      else if (e.status !== 409) throw e; // already a member: their role stays as it is
    }
  }

  let version: number | null = null;
  if (body.destination) {
    await access(db, actor, p.slug, "push");
    const url = body.destination.url;
    version = (
      await commitToml(db, actor, p, (prev) => applyDestination(prev, p.name, url), {
        message: "destination set from Foundry hub",
        secrets: { [PAYLOAD_KEY_ENV]: body.destination.api_key },
        detail: { foundry: body.ref, destination: new URL(url).host },
      })
    ).number;
  }
  return { slug: p.slug, url: projectUrl(p.slug), created, version };
}

const ignoreConflict = (e: unknown) => {
  if (!(e instanceof HttpError && e.status === 409)) throw e;
};

/** The linked project's migration status for Foundry's site panel; `{ linked: false }` when nothing is linked. */
export async function foundryStatus(db: Db, actor: Actor, rawRef: string | null) {
  const ref = parse(schemas.ref, rawRef ?? "");
  const p = await byRef(db, ref);
  if (!p) return { linked: false as const };
  await access(db, actor, p.slug, "view_raw", { archivedOk: true });
  const snap = await latestSnapshot(db, p.id);
  const lastRun = (await listRuns(db, actor, p.slug, 1))[0] ?? null;
  const base = { linked: true as const, slug: p.slug, url: projectUrl(p.slug), archived: !!p.archivedAt };
  if (!snap)
    return {
      ...base,
      source: null,
      destination_host: null,
      version: null,
      stages: null,
      issues: null,
      dry_run: false,
      ever_written: false,
      last_run: lastRun && runOf(p.slug, lastRun),
    };

  let toml = "";
  try {
    toml = readFileSync(join(snap.dir, "porter.toml"), "utf8");
  } catch {}
  const setup = toml ? readSetup(toml) : null;
  const o = overview(snap.dir, snap.version.summary);
  const sev = (s: string) =>
    (o.issueCounts ?? []).filter((c) => c.severity === s).reduce((a, c) => a + c.n, 0);
  const dryRun =
    withDb(
      snap.dir,
      (q) =>
        q(
          "SELECT 1 FROM runs WHERE stage = 'load' AND dry_run = 1 AND ended_at IS NOT NULL AND deleted_at IS NULL LIMIT 1",
        ).length > 0,
    ) ?? false;
  return {
    ...base,
    source: setup ? { adapter: setup.adapter, kinds: setup.kinds } : null,
    destination_host: toml ? safeHost(toml) : null,
    version: { number: snap.version.number, created_at: snap.version.createdAt },
    stages:
      o.views?.map((v) => ({ stage: v.stage, state: v.state ?? v.unknown ?? null, stale: v.stale })) ?? null,
    issues: o.issueCounts ? { error: sev("error"), warning: sev("warning") } : null,
    dry_run: dryRun,
    ever_written: o.writes?.everWritten ?? false,
    last_run: lastRun && runOf(p.slug, lastRun),
  };
}

const safeHost = (toml: string) => {
  try {
    return destinationHost(toml);
  } catch {
    return null;
  }
};

const runOf = (slug: string, r: Awaited<ReturnType<typeof listRuns>>[number]) => ({
  id: r.id,
  state: r.state,
  stages: r.stages,
  apply: r.apply,
  exit_code: r.exit_code,
  error: r.error,
  result_version: r.result_version,
  finished_at: r.finished_at,
  url: `${projectUrl(slug)}/runs/${r.id}`,
});
