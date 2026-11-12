import { createHash, randomUUID } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, utimesSync, writeFileSync } from "node:fs";
import { rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  auditLog,
  connect,
  mediaObject,
  organization,
  project,
  projectVersion,
  run,
  upload,
  user,
  versionMedia,
  versionUpload,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { deletePrefix, head, keys, listProjectIds, listSizes, putFile } from "@porter-hub/storage/s3";
import { eq, sql } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { retireProject, sweepRetention as sweepAll, sweepOrphanPrefixes } from "./retention";

const DAY = 24 * 3_600_000;
let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let tmp: string;
let cache: string;
/** Projects this file made. The test bucket is shared by every test file: never sweep another file's prefixes. */
const seeded = new Set<string>();
const ours = async () => (await listProjectIds()).filter((id) => seeded.has(id));
const sweepRetention = (db: typeof conn.db, dir: string) => sweepAll(db, dir, Date.now(), ours);

const sha = (b: Buffer) => createHash("sha256").update(b).digest("hex");
/** Puts `body` at `key` in storage. */
async function put(key: string, body = Buffer.from(randomUUID())) {
  const f = join(tmp, randomUUID());
  writeFileSync(f, body);
  await putFile(key, f, sha(body), body.length);
  return sha(body);
}

/** A project with a version (snapshot + media + cache dir) and an upload, all in storage. */
async function seed(archivedDaysAgo: number | null, retentionDays = 90) {
  const [p] = await conn.db
    .insert(project)
    .values({
      orgId: "org",
      name: "P",
      slug: `p-${randomUUID().slice(0, 8)}`,
      archivedAt: archivedDaysAgo === null ? null : new Date(Date.now() - archivedDaysAgo * DAY),
      retentionDays,
    })
    .returning();
  seeded.add(p.id);
  const snap = keys.snapshot(p.id, randomUUID());
  await put(snap);
  const media = Buffer.from(`media ${p.id}`);
  const mediaSha = await put(keys.media(p.id, sha(media)), media);
  await conn.db.insert(mediaObject).values({ projectId: p.id, sha256: mediaSha, size: media.length });
  const [v] = await conn.db
    .insert(projectVersion)
    .values({ projectId: p.id, number: 1, snapshotKey: snap })
    .returning();
  await conn.db.insert(versionMedia).values({ versionId: v.id, projectId: p.id, sha256: mediaSha });
  const upId = randomUUID();
  await put(keys.upload(p.id, upId));
  await conn.db.insert(upload).values({
    id: upId,
    projectId: p.id,
    filename: "dump.sql",
    objectKey: keys.upload(p.id, upId),
    state: "ready",
  });
  mkdirSync(join(cache, v.id), { recursive: true });
  writeFileSync(join(cache, v.id, "project.db"), "");
  return { p, v };
}

beforeAll(async () => {
  tmp = mkdtempSync(join(tmpdir(), "retention-test-"));
  cache = join(tmp, "cache");
  mkdirSync(cache);
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  const now = new Date();
  await conn.db.insert(user).values({
    id: "dev",
    name: "dev",
    email: "dev@x.test",
    emailVerified: true,
    createdAt: now,
    updatedAt: now,
  });
  await conn.db.insert(organization).values({ id: "org", name: "Umi", slug: "umi", createdAt: now });
}, 60_000);
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
  if (tmp) await rm(tmp, { recursive: true, force: true });
});

describe("retention", () => {
  test("retention deletes archived project data after N days and logs it", async () => {
    const { p, v } = await seed(100, 90);
    expect((await listSizes(keys.projectPrefix(p.id))).size).toBe(3);

    await sweepRetention(conn.db, cache);

    expect((await listSizes(keys.projectPrefix(p.id))).size).toBe(0);
    expect(await conn.db.select().from(project).where(eq(project.id, p.id))).toEqual([]);
    // Cascade: versions, media rows and uploads go with the project.
    expect(await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, p.id))).toEqual([]);
    expect(await conn.db.select().from(upload).where(eq(upload.projectId, p.id))).toEqual([]);
    expect(existsSync(join(cache, v.id))).toBe(false);
    const [a] = await conn.db.select().from(auditLog).where(eq(auditLog.targetId, p.id));
    expect(a).toMatchObject({ action: "retention.delete", targetType: "project", actorId: null });
    expect(a.detail).toMatchObject({ project_id: p.id, slug: p.slug, retention_days: 90, versions: 1 });
  });

  test("active projects untouched", async () => {
    const active = await seed(null);
    const recent = await seed(10, 90); // archived, but still inside its retention window
    await sweepRetention(conn.db, cache);
    for (const { p, v } of [active, recent]) {
      expect((await listSizes(keys.projectPrefix(p.id))).size).toBe(3);
      expect(await conn.db.select().from(project).where(eq(project.id, p.id))).toHaveLength(1);
      expect(existsSync(join(cache, v.id))).toBe(true);
      expect(await conn.db.select().from(auditLog).where(eq(auditLog.targetId, p.id))).toEqual([]);
    }
  });

  const audits = async (id: string) => conn.db.select().from(auditLog).where(eq(auditLog.targetId, id));
  const intact = async (id: string) => {
    expect((await listSizes(keys.projectPrefix(id))).size).toBe(3);
    expect(await conn.db.select().from(project).where(eq(project.id, id))).toHaveLength(1);
    expect(await audits(id)).toEqual([]);
  };

  test("unarchived after the daily list was taken: the locked re-check skips it, nothing deleted", async () => {
    const { p } = await seed(100, 90);
    await conn.db.update(project).set({ archivedAt: null }).where(eq(project.id, p.id));
    expect(await retireProject(conn.db, p.id, cache)).toBe(false);
    await intact(p.id);
  });

  test("a queued or running run holds retention back until it ends", async () => {
    const { p } = await seed(100, 90);
    const [r] = await conn.db
      .insert(run)
      .values({ projectId: p.id, stages: ["analyse"] })
      .returning();
    await sweepRetention(conn.db, cache);
    await intact(p.id);
    await conn.db.update(run).set({ state: "running" }).where(eq(run.id, r.id));
    expect(await retireProject(conn.db, p.id, cache)).toBe(false);
    await conn.db.update(run).set({ state: "finished" }).where(eq(run.id, r.id));
    await sweepRetention(conn.db, cache);
    expect(await conn.db.select().from(project).where(eq(project.id, p.id))).toEqual([]);
    expect(await audits(p.id)).toHaveLength(1);
  });

  test("two replicas at once: one deletes, the other skips; one audit row", async () => {
    const { p } = await seed(100, 90);
    const slow = async (prefix: string) => {
      await new Promise((r) => setTimeout(r, 300));
      return deletePrefix(prefix);
    };
    const results = await Promise.all([
      retireProject(conn.db, p.id, cache, Date.now(), slow),
      retireProject(conn.db, p.id, cache, Date.now(), slow),
    ]);
    expect(results.sort()).toEqual([false, true]);
    expect(await audits(p.id)).toHaveLength(1);
    expect((await listSizes(keys.projectPrefix(p.id))).size).toBe(0);
  });

  test("storage failing after the rows went: files stay until the orphaned-prefix sweep empties them", async () => {
    const { p } = await seed(100, 90);
    const broken = async () => {
      throw new Error("storage down");
    };
    expect(await retireProject(conn.db, p.id, cache, Date.now(), broken)).toBe(true);
    expect(await conn.db.select().from(project).where(eq(project.id, p.id))).toEqual([]);
    expect(await audits(p.id)).toHaveLength(1);
    expect((await listSizes(keys.projectPrefix(p.id))).size).toBe(3);
    const live = await seed(null); // a project that still exists keeps its files
    expect(await sweepOrphanPrefixes(conn.db, ours)).toBe(3);
    expect((await listSizes(keys.projectPrefix(p.id))).size).toBe(0);
    expect((await listSizes(keys.projectPrefix(live.p.id))).size).toBe(3);
  });

  test("the row delete failing leaves storage untouched: the project stays whole for the next pass", async () => {
    const { p } = await seed(100, 90);
    await conn.db.execute(
      sql.raw(`
      create function refuse() returns trigger language plpgsql as $$ begin raise exception 'no delete'; end $$;
      create trigger refuse before delete on project for each row execute function refuse();`),
    );
    let emptied = false;
    const spy = async (prefix: string) => {
      emptied = true;
      return deletePrefix(prefix);
    };
    try {
      await expect(retireProject(conn.db, p.id, cache, Date.now(), spy)).rejects.toThrow();
    } finally {
      await conn.db.execute(sql.raw("drop trigger refuse on project; drop function refuse();"));
    }
    expect(emptied).toBe(false);
    await intact(p.id);
    expect(await retireProject(conn.db, p.id, cache)).toBe(true);
  });

  test("a media-heavy project (5k media, 20 versions) is deleted well inside the worker's statement timeout", async () => {
    const { p } = await seed(100, 90);
    // The worker's own connection settings: every statement outside retention's SET LOCAL gets 10 s.
    const worker = connect(fresh.url, { connection: { statement_timeout: 10_000 } });
    try {
      await worker.db.execute(sql`
        insert into media_object (project_id, sha256, size)
        select ${p.id}, md5(g::text) || md5((g + 1)::text), 1 from generate_series(1, 5000) g`);
      await worker.db.execute(sql`
        insert into project_version (project_id, number, snapshot_key)
        select ${p.id}, n, 'x' from generate_series(2, 20) n`);
      await worker.db.execute(sql`
        insert into version_media (version_id, project_id, sha256)
        select v.id, m.project_id, m.sha256 from project_version v join media_object m on m.project_id = v.project_id
        where v.project_id = ${p.id} on conflict do nothing`);
      const t = Date.now();
      expect(await retireProject(worker.db, p.id, cache)).toBe(true);
      // ~50 ms with the version_media(project_id, sha256) index; seconds (quadratic) without it.
      expect(Date.now() - t).toBeLessThan(2000);
      expect(await conn.db.select().from(project).where(eq(project.id, p.id))).toEqual([]);
    } finally {
      await worker.client.end();
    }
  });

  test("an init made while the sweep would run keeps its media: both lock the project row", async () => {
    const { p } = await seed(null);
    const body = Buffer.from(`contested ${p.id}`);
    const h = await put(keys.media(p.id, sha(body)), body);
    const oldId = randomUUID();
    await conn.db.insert(versionUpload).values({
      id: oldId,
      projectId: p.id,
      userId: "dev",
      snapshotSha256: "0".repeat(64),
      mediaHashes: [h],
      missing: [h],
      createdAt: new Date(Date.now() - 2 * DAY),
    });
    // What initVersion does: `for share` on the project, then insert its init (listing h as missing).
    let sweep: Promise<unknown> | undefined;
    await conn.db.transaction(async (tx) => {
      await tx.select({ id: project.id }).from(project).where(eq(project.id, p.id)).for("share");
      await tx.insert(versionUpload).values({
        projectId: p.id,
        userId: "dev",
        snapshotSha256: "1".repeat(64),
        mediaHashes: [h],
        missing: [h],
      });
      sweep = sweepRetention(conn.db, cache);
      await new Promise((r) => setTimeout(r, 300)); // the sweep is now waiting on our lock
    });
    await sweep;
    expect(await head(keys.media(p.id, h))).not.toBeNull();
    const left = await conn.db.select().from(versionUpload).where(eq(versionUpload.projectId, p.id));
    expect(left.map((r) => r.id)).not.toContain(oldId);
  });

  test("expired push inits: their snapshot and never-committed media are deleted; live ones kept", async () => {
    const { p } = await seed(null);
    const [kept] = await conn.db.select().from(mediaObject).where(eq(mediaObject.projectId, p.id));
    const orphan = Buffer.from(`orphan ${p.id}`);
    const shared = Buffer.from(`shared ${p.id}`); // also missing in a live init: must survive
    const orphanSha = await put(keys.media(p.id, sha(orphan)), orphan);
    const sharedSha = await put(keys.media(p.id, sha(shared)), shared);
    const oldId = randomUUID();
    const liveId = randomUUID();
    await put(keys.snapshot(p.id, oldId));
    await put(keys.snapshot(p.id, liveId));
    await conn.db.insert(versionUpload).values([
      {
        id: oldId,
        projectId: p.id,
        userId: "dev",
        snapshotSha256: "0".repeat(64),
        mediaHashes: [orphanSha, sharedSha, kept.sha256],
        missing: [orphanSha, sharedSha, kept.sha256],
        createdAt: new Date(Date.now() - 2 * DAY),
      },
      {
        id: liveId,
        projectId: p.id,
        userId: "dev",
        snapshotSha256: "0".repeat(64),
        mediaHashes: [sharedSha],
        missing: [sharedSha],
      },
    ]);

    await sweepRetention(conn.db, cache);

    expect(await head(keys.snapshot(p.id, oldId))).toBeNull();
    expect(await head(keys.media(p.id, orphanSha))).toBeNull();
    expect(await head(keys.media(p.id, sharedSha))).not.toBeNull();
    expect(await head(keys.media(p.id, kept.sha256))).not.toBeNull(); // committed by some version
    expect(await head(keys.snapshot(p.id, liveId))).not.toBeNull();
    const left = await conn.db.select().from(versionUpload).where(eq(versionUpload.projectId, p.id));
    expect(left.map((r) => r.id)).toEqual([liveId]);
  });

  test("stale .tmp- extraction dirs in the cache go after a day; fresh ones and versions stay", async () => {
    const old = join(cache, `.tmp-${randomUUID()}`);
    const young = join(cache, `.tmp-${randomUUID()}`);
    const version = join(cache, randomUUID());
    for (const d of [old, young, version]) mkdirSync(d);
    const twoDaysAgo = new Date(Date.now() - 2 * DAY);
    utimesSync(old, twoDaysAgo, twoDaysAgo);
    utimesSync(version, twoDaysAgo, twoDaysAgo);

    await sweepRetention(conn.db, cache);

    expect(existsSync(old)).toBe(false);
    expect(existsSync(young)).toBe(true);
    expect(existsSync(version)).toBe(true);
  });
});
