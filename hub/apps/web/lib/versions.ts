import { mediaObject, project, projectVersion, versionMedia, versionUpload } from "@porter-hub/db";
import { and, desc, eq, inArray, sql } from "drizzle-orm";
import { z } from "zod";
import type { Db } from "./permissions";
import { type Actor, access, audit, HttpError, parse } from "./projects";
import { head, hexToBase64, keys, presignGet, presignPut } from "./storage";

/** First push: `base_version` is null (there is no version 0). A number must be an existing version. */
export const MAX_SUMMARY_BYTES = 256 * 1024;
export const MAX_MEDIA_HASHES = 100_000;
export const MAX_GET_BATCH = 1000;
/** An init'd push that isn't committed within a day is dead. */
const UPLOAD_TTL_MS = 24 * 3_600_000;
const HEAD_CONCURRENCY = 16;

const hash = z.string().regex(/^[0-9a-f]{64}$/, "must be 64 lowercase hex characters");
const base = z.number().int().min(1).nullable();
const schemas = {
  init: z.object({ base_version: base, media_hashes: z.array(hash).max(MAX_MEDIA_HASHES) }),
  commit: z.object({
    upload_id: z.uuid(),
    base_version: base,
    message: z.string().max(2000).optional(),
    porter_version: z.string().min(1).max(100),
    summary: z.record(z.string(), z.unknown()),
  }),
  media: z.object({ hashes: z.array(hash).min(1).max(MAX_GET_BATCH) }),
  n: z.coerce.number().int().min(1),
};

const latestOf = async (db: Db | Parameters<Parameters<Db["transaction"]>[0]>[0], projectId: string) => {
  const [r] = await db
    .select({ n: sql<number | null>`max(${projectVersion.number})` })
    .from(projectVersion)
    .where(eq(projectVersion.projectId, projectId));
  return r?.n ?? null;
};

async function mapLimit<T, R>(items: T[], fn: (x: T) => Promise<R>): Promise<R[]> {
  const out: R[] = new Array(items.length);
  let i = 0;
  await Promise.all(
    Array.from({ length: Math.min(HEAD_CONCURRENCY, items.length) }, async () => {
      while (i < items.length) {
        const k = i++;
        out[k] = await fn(items[k]);
      }
    }),
  );
  return out;
}

export async function listVersions(db: Db, actor: Actor, slug: string) {
  const { project: p } = await access(db, actor, slug, "pull");
  const rows = await db
    .select()
    .from(projectVersion)
    .where(eq(projectVersion.projectId, p.id))
    .orderBy(desc(projectVersion.number));
  return rows.map((v) => ({
    number: v.number,
    message: v.message,
    porter_version: v.porterVersion,
    size: v.size,
    created_by: v.createdBy,
    created_at: v.createdAt,
  }));
}

export async function getVersion(db: Db, actor: Actor, slug: string, n: string) {
  const { project: p } = await access(db, actor, slug, "pull");
  const number = parse(schemas.n, n);
  const [v] = await db
    .select()
    .from(projectVersion)
    .where(and(eq(projectVersion.projectId, p.id), eq(projectVersion.number, number)));
  if (!v) throw new HttpError(404, "Version not found");
  const media = await db
    .select({ sha256: versionMedia.sha256 })
    .from(versionMedia)
    .where(eq(versionMedia.versionId, v.id))
    .orderBy(versionMedia.sha256);
  return {
    number: v.number,
    message: v.message,
    porter_version: v.porterVersion,
    size: v.size,
    summary: v.summary,
    created_by: v.createdBy,
    created_at: v.createdAt,
    snapshot_get_url: await presignGet(v.snapshotKey),
    media_hashes: media.map((m) => m.sha256),
  };
}

export async function initVersion(db: Db, actor: Actor, slug: string, input: unknown) {
  const { project: p } = await access(db, actor, slug, "push");
  const { base_version, media_hashes } = parse(schemas.init, input);
  const latest = await latestOf(db, p.id);
  // Fail early so the client doesn't upload gigabytes for a push that will be refused.
  if (latest !== base_version) throw new HttpError(409, "Base version is not the latest", { latest });
  const hashes = [...new Set(media_hashes)];
  const have = new Set<string>();
  for (let i = 0; i < hashes.length; i += 5000) {
    const rows = await db
      .select({ sha256: mediaObject.sha256 })
      .from(mediaObject)
      .where(and(eq(mediaObject.projectId, p.id), inArray(mediaObject.sha256, hashes.slice(i, i + 5000))));
    for (const r of rows) have.add(r.sha256);
  }
  const missing = hashes.filter((h) => !have.has(h));
  const [up] = await db
    .insert(versionUpload)
    .values({ projectId: p.id, userId: actor.id, mediaHashes: hashes, missing })
    .returning({ id: versionUpload.id });
  return {
    upload_id: up.id,
    missing,
    snapshot_put_url: await presignPut(keys.snapshot(p.id, up.id)),
    // PUT each with header `x-amz-checksum-sha256: base64(raw sha256 bytes)`; S3 rejects a mismatching body.
    media_put_urls: Object.fromEntries(
      await Promise.all(missing.map(async (h) => [h, await presignPut(keys.media(p.id, h), h)] as const)),
    ),
  };
}

export async function commitVersion(db: Db, actor: Actor, slug: string, input: unknown) {
  const { project: p } = await access(db, actor, slug, "push");
  const body = parse(schemas.commit, input);
  const summaryJson = JSON.stringify(body.summary);
  if (Buffer.byteLength(summaryJson) > MAX_SUMMARY_BYTES) {
    throw new HttpError(413, `summary: larger than ${MAX_SUMMARY_BYTES / 1024} KB`);
  }
  // Another project's or another user's upload_id looks exactly like an unknown one.
  const [up] = await db
    .select()
    .from(versionUpload)
    .where(
      and(
        eq(versionUpload.id, body.upload_id),
        eq(versionUpload.projectId, p.id),
        eq(versionUpload.userId, actor.id),
      ),
    );
  if (!up || Date.now() - up.createdAt.getTime() > UPLOAD_TTL_MS) {
    throw new HttpError(404, "Unknown or expired upload_id");
  }

  const snapshotKey = keys.snapshot(p.id, up.id);
  const snap = await head(snapshotKey);
  if (!snap) throw new HttpError(422, "The snapshot has not been uploaded");
  const heads = await mapLimit(up.missing, async (h) => [h, await head(keys.media(p.id, h))] as const);
  const absent = heads.filter(([, o]) => !o).map(([h]) => h);
  if (absent.length) throw new HttpError(422, "Some media has not been uploaded", { missing: absent });
  // Belt and braces with the signed checksum header on the PUT: what S3 stored must be the hash.
  const wrong = heads
    .filter(([h, o]) => o?.checksumSha256 && o.checksumSha256 !== hexToBase64(h))
    .map(([h]) => h);
  if (wrong.length) throw new HttpError(422, "Some media does not match its hash", { mismatched: wrong });

  try {
    return await db.transaction(async (tx) => {
      // Serialise pushes to one project so the latest check and the insert can't interleave.
      await tx.select({ id: project.id }).from(project).where(eq(project.id, p.id)).for("update");
      const latest = await latestOf(tx, p.id);
      if (latest !== body.base_version)
        throw new HttpError(409, "Base version is not the latest", { latest });
      const number = (latest ?? 0) + 1;
      const [v] = await tx
        .insert(projectVersion)
        .values({
          projectId: p.id,
          number,
          snapshotKey,
          porterVersion: body.porter_version,
          createdBy: actor.id,
          size: snap.size,
          summary: body.summary,
          message: body.message,
        })
        .returning({ id: projectVersion.id });
      if (up.missing.length) {
        const size = new Map(heads.map(([h, o]) => [h, o?.size ?? 0]));
        await tx
          .insert(mediaObject)
          .values(
            up.missing.map((h) => ({
              projectId: p.id,
              sha256: h,
              size: size.get(h) ?? 0,
              contentType: null,
            })),
          )
          .onConflictDoNothing();
      }
      if (up.mediaHashes.length) {
        await tx
          .insert(versionMedia)
          .values(up.mediaHashes.map((sha256) => ({ versionId: v.id, projectId: p.id, sha256 })));
      }
      await tx.delete(versionUpload).where(eq(versionUpload.id, up.id));
      await audit(tx, actor, p.id, "version.commit", v.id, {
        number,
        size: snap.size,
        media: up.mediaHashes.length,
        newMedia: up.missing.length,
      });
      return { number, created_at: new Date() };
    });
  } catch (e) {
    const code =
      (e as { code?: string; cause?: { code?: string } }).cause?.code ?? (e as { code?: string }).code;
    // Unique (project_id, number) backstop, or a replayed upload_id.
    if (code === "23505")
      throw new HttpError(409, "Base version is not the latest", { latest: await latestOf(db, p.id) });
    throw e;
  }
}

/** Presigned GETs, per hash, on request. Hashes the project doesn't have come back in `missing`. */
export async function mediaUrls(db: Db, actor: Actor, slug: string, input: unknown) {
  const { project: p } = await access(db, actor, slug, "pull");
  const { hashes } = parse(schemas.media, input);
  const uniq = [...new Set(hashes)];
  const rows = await db
    .select({ sha256: mediaObject.sha256 })
    .from(mediaObject)
    .where(and(eq(mediaObject.projectId, p.id), inArray(mediaObject.sha256, uniq)));
  const have = new Set(rows.map((r) => r.sha256));
  return {
    urls: Object.fromEntries(
      await Promise.all([...have].map(async (h) => [h, await presignGet(keys.media(p.id, h))] as const)),
    ),
    missing: uniq.filter((h) => !have.has(h)),
  };
}
