import { readdir, rm, stat } from "node:fs/promises";
import { join } from "node:path";
import { auditLog, project, projectVersion, run, versionMedia, versionUpload } from "@porter-hub/db";
import { deleteKeys, deletePrefix, keys, listProjectIds } from "@porter-hub/storage/s3";
import { and, eq, inArray, isNotNull, lt, notExists, sql } from "drizzle-orm";
import type { Db } from "./claim";

export const RETENTION_MS = 24 * 3_600_000;
/**
 * A push init the hub no longer accepts a commit for: apps/web/lib/versions.ts UPLOAD_TTL_MS (24 h) plus an hour, so
 * a commit that started just inside the TTL (on another machine's clock) has finished before its objects go.
 */
const INIT_TTL_MS = 25 * 3_600_000;
/** An extraction temp dir this old was left by a crashed web process. */
const TMP_TTL_MS = 24 * 3_600_000;

/** Archived, past `retention_days` at `now`, and no queued or running run (it would write into the prefix after us). */
const expired = (db: Db, now: number) =>
  and(
    isNotNull(project.archivedAt),
    sql`${project.archivedAt} + make_interval(days => ${project.retentionDays}) < ${new Date(now).toISOString()}::timestamptz`,
    notExists(
      db
        .select({ id: run.id })
        .from(run)
        .where(and(eq(run.projectId, project.id), inArray(run.state, ["queued", "running"]))),
    ),
  );

/**
 * Deletes one expired project: in one transaction, lock the row (`for update skip locked`) re-checking that it is
 * still expired, write the audit row and delete the row (cascade); after the commit, empty its storage prefix.
 * Holding the lock keeps a push commit, a new run (its FK takes a key-share lock) or an unarchive out; a project that
 * was unarchived, got a run, or is locked by another replica is skipped (false), with no audit row.
 *
 * Order: rows first, storage after. A failed row delete rolls back with storage untouched, so the archived project is
 * still whole for the next pass. A failed (or crashed) storage delete leaves objects under a prefix whose project no
 * longer exists; `sweepOrphanPrefixes` empties those on every pass. Rows never outlive their files. Extracted
 * snapshots in `cacheDir` go last.
 */
export async function retireProject(
  db: Db,
  id: string,
  cacheDir: string,
  now = Date.now(),
  emptyPrefix: (prefix: string) => Promise<number> = deletePrefix,
) {
  const done = await db.transaction(async (tx) => {
    // The worker's 10 s statement_timeout is for its claim loop; a big project's cascade can take longer.
    await tx.execute(sql`set local statement_timeout = '10min'`);
    const [p] = await tx
      .select()
      .from(project)
      .where(and(eq(project.id, id), expired(tx as unknown as Db, now)))
      .for("update", { skipLocked: true });
    if (!p) return null;
    const versions = await tx
      .select({ id: projectVersion.id })
      .from(projectVersion)
      .where(eq(projectVersion.projectId, p.id));
    await tx.insert(auditLog).values({
      projectId: null, // the row is going; the id stays in target_id and detail
      actorId: null,
      action: "retention.delete",
      targetType: "project",
      targetId: p.id,
      detail: {
        project_id: p.id,
        org_id: p.orgId,
        name: p.name,
        slug: p.slug,
        archived_at: p.archivedAt,
        retention_days: p.retentionDays,
        versions: versions.length,
      },
    });
    // version_media's FK to media_object is NO ACTION. In one cascading delete its check can fire before the
    // cascade through project_version reaches version_media (RI triggers fire in constraint-name order, which
    // follows OIDs), and the delete fails. Clearing it first (by the project_id index) makes the order irrelevant.
    await tx.delete(versionMedia).where(eq(versionMedia.projectId, p.id));
    await tx.delete(project).where(eq(project.id, p.id));
    return { p, versions };
  });
  if (!done) return false;
  let objects: number | string = "unknown";
  try {
    objects = await emptyPrefix(keys.projectPrefix(done.p.id));
  } catch (e) {
    console.error(
      `retention: emptying storage for deleted project ${done.p.id} failed (swept next pass):`,
      e,
    );
  }
  for (const v of done.versions) await rm(join(cacheDir, v.id), { recursive: true, force: true });
  console.log(`retention: deleted project ${done.p.id} (${done.p.slug}), ${objects} objects`);
  return true;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
/**
 * Empties `projects/<id>/` for every id with no project row: storage a retirement couldn't finish, or a push PUT
 * that landed after it. Project ids are never reused, so an id missing from the table is gone for good. The bucket
 * must be this hub's alone (DEPLOY.md): another hub's projects look orphaned from here.
 */
export async function sweepOrphanPrefixes(
  db: Db,
  listIds: () => Promise<string[]> = listProjectIds,
  emptyPrefix: (prefix: string) => Promise<number> = deletePrefix,
) {
  const ids = (await listIds()).filter((id) => UUID.test(id));
  if (!ids.length) return 0;
  const live = new Set(
    (await db.select({ id: project.id }).from(project).where(inArray(project.id, ids))).map((r) => r.id),
  );
  let n = 0;
  for (const id of ids) {
    if (live.has(id)) continue;
    try {
      n += await emptyPrefix(keys.projectPrefix(id));
    } catch (e) {
      console.error(`retention: emptying orphaned prefix ${id} failed:`, e);
    }
  }
  return n;
}

/**
 * Clears one abandoned push init: its snapshot object, and the media it uploaded that no version committed and no
 * other init is still waiting on. Under `for update` on the project, which a push commit and a push init also
 * take: an init is either seen here (its hashes kept) or waits until the objects are gone, and no commit can claim
 * a hash between the orphan query and its delete.
 */
async function clearInit(db: Db, u: typeof versionUpload.$inferSelect) {
  await db.transaction(async (tx) => {
    const [p] = await tx
      .select({ id: project.id })
      .from(project)
      .where(eq(project.id, u.projectId))
      .for("update");
    if (!p) return; // the project went (and its prefix with it)
    // A commit may have claimed it while we waited for the lock.
    const [still] = await tx
      .select({ id: versionUpload.id })
      .from(versionUpload)
      .where(eq(versionUpload.id, u.id));
    if (!still) return;
    // Never committed (a commit deletes its init row), so the snapshot goes too.
    const gone = [keys.snapshot(u.projectId, u.id)];
    if (u.missing.length) {
      const missing = sql.join(
        u.missing.map((h) => sql`${h}`),
        sql`,`,
      );
      const orphans = await tx.execute<{ h: string }>(sql`
        select h from unnest(array[${missing}]::text[]) as h
        where not exists (select 1 from media_object m where m.project_id = ${u.projectId} and m.sha256 = h)
          and not exists (select 1 from version_upload v
                          where v.project_id = ${u.projectId} and v.id <> ${u.id} and h = any(v.missing))`);
      for (const { h } of orphans) gone.push(keys.media(u.projectId, h));
    }
    // Batched (1000 per call): a push commit or run commit on this project waits on our lock meanwhile.
    await deleteKeys(gone);
    await tx.delete(versionUpload).where(eq(versionUpload.id, u.id));
  });
}

/**
 * The daily retention job:
 * - archived projects past `retention_days` lose everything (retireProject);
 * - push inits past their TTL lose their snapshot object and the media they uploaded that nothing committed;
 * - storage prefixes of projects that no longer exist are emptied (sweepOrphanPrefixes);
 * - `.tmp-*` extraction dirs older than a day go from `cacheDir`.
 * Each item's failure is logged and skipped; the next day retries it. Safe to run on several workers at once.
 */
export async function sweepRetention(
  db: Db,
  cacheDir: string,
  now = Date.now(),
  listIds: () => Promise<string[]> = listProjectIds,
) {
  const candidates = await db.select({ id: project.id }).from(project).where(expired(db, now));
  let projects = 0;
  for (const { id } of candidates) {
    try {
      if (await retireProject(db, id, cacheDir, now)) projects++;
    } catch (e) {
      console.error(`retention: deleting project ${id} failed:`, e);
    }
  }

  const stale = await db
    .select()
    .from(versionUpload)
    .where(lt(versionUpload.createdAt, new Date(now - INIT_TTL_MS)));
  for (const u of stale) {
    try {
      await clearInit(db, u);
    } catch (e) {
      console.error(`retention: clearing push init ${u.id} failed:`, e);
    }
  }
  const orphans = await sweepOrphanPrefixes(db, listIds).catch((e) => {
    console.error("retention: orphaned prefix sweep failed:", e);
    return 0;
  });

  let tmp = 0;
  for (const name of await readdir(cacheDir).catch(() => [] as string[])) {
    if (!name.startsWith(".tmp-")) continue;
    const dir = join(cacheDir, name);
    const s = await stat(dir).catch(() => null);
    if (s && s.mtimeMs < now - TMP_TTL_MS) {
      await rm(dir, { recursive: true, force: true });
      tmp++;
    }
  }
  return { projects, inits: stale.length, orphans, tmp };
}
