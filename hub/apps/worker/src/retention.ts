import { readdir, rm, stat } from "node:fs/promises";
import { join } from "node:path";
import { auditLog, project, projectVersion, run, versionUpload } from "@porter-hub/db";
import { deleteObject, deletePrefix, keys } from "@porter-hub/storage/s3";
import { and, eq, inArray, isNotNull, lt, notExists, sql } from "drizzle-orm";
import type { Db } from "./claim";

export const RETENTION_MS = 24 * 3_600_000;
/** A push init the hub no longer accepts a commit for (apps/web/lib/versions.ts UPLOAD_TTL_MS). */
const INIT_TTL_MS = 24 * 3_600_000;
/** An extraction temp dir this old was left by a crashed web process. */
const TMP_TTL_MS = 24 * 3_600_000;

/**
 * The daily retention job:
 * - archived projects past `retention_days` lose everything: storage under `projects/<id>/`, the DB rows
 *   (cascade) and their extracted snapshots in `cacheDir`, with one audit row each (the project id kept in it);
 * - push inits past their TTL lose their snapshot object and the media they uploaded that nothing committed;
 * - `.tmp-*` extraction dirs older than a day go from `cacheDir`.
 * Each item's failure is logged and skipped; the next day retries it. Safe to run on several workers at once.
 */
export async function sweepRetention(db: Db, cacheDir: string, now = Date.now()) {
  const expired = await db
    .select()
    .from(project)
    .where(
      and(
        isNotNull(project.archivedAt),
        sql`${project.archivedAt} + make_interval(days => ${project.retentionDays}) < ${new Date(now).toISOString()}::timestamptz`,
        // A run still going would write into the prefix after we empty it; wait for it to end.
        notExists(
          db
            .select({ id: run.id })
            .from(run)
            .where(and(eq(run.projectId, project.id), inArray(run.state, ["queued", "running"]))),
        ),
      ),
    );
  for (const p of expired) {
    try {
      const versions = await db
        .select({ id: projectVersion.id })
        .from(projectVersion)
        .where(eq(projectVersion.projectId, p.id));
      // Storage first: if the DB delete then fails, tomorrow's run finds the project again and finishes.
      const objects = await deletePrefix(keys.projectPrefix(p.id));
      await db.transaction(async (tx) => {
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
      });
      for (const v of versions) await rm(join(cacheDir, v.id), { recursive: true, force: true });
      console.log(`retention: deleted project ${p.id} (${p.slug}), ${objects} objects`);
    } catch (e) {
      console.error(`retention: deleting project ${p.id} failed:`, e);
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
  return { projects: expired.length, inits: stale.length, tmp };
}
