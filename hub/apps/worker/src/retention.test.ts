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
  upload,
  user,
  versionUpload,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { head, keys, listSizes, putFile } from "@porter-hub/storage/s3";
import { eq } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { sweepRetention } from "./retention";

const DAY = 24 * 3_600_000;
let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let tmp: string;
let cache: string;

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
  const snap = keys.snapshot(p.id, randomUUID());
  await put(snap);
  const media = Buffer.from(`media ${p.id}`);
  const mediaSha = await put(keys.media(p.id, sha(media)), media);
  await conn.db.insert(mediaObject).values({ projectId: p.id, sha256: mediaSha, size: media.length });
  const [v] = await conn.db
    .insert(projectVersion)
    .values({ projectId: p.id, number: 1, snapshotKey: snap })
    .returning();
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
    expect(a.detail).toMatchObject({ project_id: p.id, slug: p.slug, retention_days: 90, objects: 3 });
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
