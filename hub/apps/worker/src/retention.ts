import { readdir, rm, stat } from "node:fs/promises";
import { join } from "node:path";
import { auditLog, project, projectVersion, run, versionUpload } from "@porter-hub/db";
import { deleteObject, deletePrefix, keys } from "@porter-hub/storage/s3";
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
 * Deletes one expired project in a single transaction: lock the row (`for update skip locked`) re-checking that it
 * is still expired, empty its storage prefix, write the audit row, delete the row (cascade). Holding the lock keeps a
 * push commit, a new run (its FK takes a key-share lock) or an unarchive out until we're done; a project that was
 * unarchived, got a run, or is locked by another replica is skipped (false), with no audit row. If storage fails the
 * transaction rolls back; a crash after storage is emptied is safe too, since the next pass deletes the (now empty)
 * prefix again and commits. Extracted snapshots in `cacheDir` go after the commit.
 */
export async function retireProject(
  db: Db,
  id: string,
  cacheDir: string,
  now = Date.now(),
  emptyPrefix: (prefix: string) => Promise<number> = deletePrefix,
) {
  const done = await db.transaction(async (tx) => {
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
    const objects = await emptyPrefix(keys.projectPrefix(p.id));
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
        objects,
      },
    });
    await tx.delete(project).where(eq(project.id, p.id));
    return { p, versions, objects };
  });
  if (!done) return false;
  for (const v of done.versions) await rm(join(cacheDir, v.id), { recursive: true, force: true });
  console.log(`retention: deleted project ${done.p.id} (${done.p.slug}), ${done.objects} objects`);
  return true;
}

/**
 * The daily retention job:
 * - archived projects past `retention_days` lose everything (retireProject);
 * - push inits past their TTL lose their snapshot object and the media they uploaded that nothing committed;
 * - `.tmp-*` extraction dirs older than a day go from `cacheDir`.
 * Each item's failure is logged and skipped; the next day retries it. Safe to run on several workers at once.
 */
export async function sweepRetention(db: Db, cacheDir: string, now = Date.now()) {
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
      await deleteObject(keys.snapshot(u.projectId, u.id)); // never committed: a commit deletes its init row
      // Media this init uploaded that no version committed and no other init is still waiting on.
      // ponytail: one DeleteObject per hash; batch with DeleteObjects if inits with huge media sets pile up.
      const missing = sql.join(
        u.missing.map((h) => sql`${h}`),
        sql`,`,
      );
      const orphans = await db.execute<{ h: string }>(sql`
        select h from unnest(array[${missing}]::text[]) as h
        where not exists (select 1 from media_object m where m.project_id = ${u.projectId} and m.sha256 = h)
          and not exists (select 1 from version_upload v
                          where v.project_id = ${u.projectId} and v.id <> ${u.id} and h = any(v.missing))`);
      for (const { h } of orphans) await deleteObject(keys.media(u.projectId, h));
      await db.delete(versionUpload).where(eq(versionUpload.id, u.id));
    } catch (e) {
      console.error(`retention: clearing push init ${u.id} failed:`, e);
    }
  }

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
  return { projects, inits: stale.length, tmp };
}
