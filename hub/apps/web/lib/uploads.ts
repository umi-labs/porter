import { randomUUID } from "node:crypto";
import { upload } from "@porter-hub/db";
import { verifyUpload } from "@porter-hub/storage";
import { and, count, desc, eq } from "drizzle-orm";
import { z } from "zod";
import { type Db, type Role, roleCan } from "./permissions";
import { type Actor, access, audit, HttpError, parse } from "./projects";
import * as store from "./storage";

export const PART_SIZE = 16 * 1024 * 1024;
/** S3 allows at most 10,000 parts per multipart upload. */
const MAX_PARTS = 10_000;
/** 16 MiB, or bigger (whole MiB) for files past 10,000 × 16 MiB (~156 GiB), so the part limit never caps a file. */
export const partSizeFor = (size: number) =>
  Math.max(PART_SIZE, Math.ceil(size / MAX_PARTS / 1024 ** 2) * 1024 ** 2);
export const MAX_PART_BATCH = 100;
const DEFAULT_MAX_BYTES = 20 * 1024 ** 3;
/** Open (still `uploading`) uploads one user may have per project. */
export const MAX_OPEN_UPLOADS = 5;
export const maxBytes = () => Number(process.env.UPLOAD_MAX_BYTES) || DEFAULT_MAX_BYTES;

export const PURPOSES = ["dump", "wxr", "media-archive", "other"] as const;
type Purpose = (typeof PURPOSES)[number];
/** Matched on the lowercased filename suffix only; content is not sniffed in v1. */
export const ALLOWED: Record<Purpose, readonly string[] | null> = {
  dump: [".sql", ".sql.gz", ".gz", ".zip", ".tar.gz", ".tgz"],
  wxr: [".xml", ".xml.gz", ".zip"],
  "media-archive": [".zip", ".tar", ".tar.gz", ".tgz"],
  other: null,
};

/** Strips path separators and control characters, caps at 255 chars. It is metadata only, never part of a key. */
export function sanitizeFilename(name: string) {
  // Controls (C0, DEL, C1), format chars such as bidi overrides, and path separators; truncated by code points.
  return [...name.replace(/[\p{Cc}\p{Cf}\p{Cs}/\\]/gu, "")].slice(0, 255).join("").trim();
}

export const contentDisposition = (filename: string) =>
  `attachment; filename*=UTF-8''${encodeURIComponent(filename).replace(/['()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`)}`;

const schemas = {
  create: z.object({
    filename: z.string().min(1).max(4096),
    size: z.number().int().min(1),
    purpose: z.enum(PURPOSES),
  }),
  /** Each part is hashed by the client (sha256 of its exact bytes); S3 rejects a body that doesn't match. */
  parts: z.object({
    parts: z
      .array(
        z.object({
          part_number: z.number().int().min(1),
          sha256: z.string().regex(/^[0-9a-f]{64}$/, "must be 64 lowercase hex characters"),
        }),
      )
      .min(1)
      .max(MAX_PART_BATCH),
  }),
  complete: z.object({ sha256: z.string().regex(/^[0-9a-f]{64}$/, "must be 64 lowercase hex characters") }),
  id: z.uuid(),
};

const partCount = (size: number) => Math.ceil(size / partSizeFor(size));
/** Exact byte length of part n (1-based): the part size, except the last. */
const partLength = (size: number, n: number) => {
  const ps = partSizeFor(size);
  return Math.min(ps, size - (n - 1) * ps);
};
const view = (u: typeof upload.$inferSelect) => ({
  id: u.id,
  filename: u.filename,
  size: u.size,
  purpose: u.purpose,
  state: u.state,
  // The client-declared hash is only shown once the server's own hash has matched it.
  sha256: u.state === "ready" ? u.sha256 : null,
  uploader_id: u.uploaderId,
  created_at: u.createdAt,
});

/** The upload, if this caller may see it: clients only see their own. A hidden upload is a 404. */
async function load(
  db: Db,
  actor: Actor,
  slug: string,
  rawId: string,
  action: "view" | "upload" | "push",
  opts?: { archivedOk?: boolean },
) {
  const { project: p, role } = await access(db, actor, slug, action, opts);
  const id = parse(schemas.id, rawId);
  const [u] = await db
    .select()
    .from(upload)
    .where(and(eq(upload.id, id), eq(upload.projectId, p.id)));
  if (!u || (role === "client" && u.uploaderId !== actor.id)) throw new HttpError(404, "Upload not found");
  return { p, role, u };
}
/** Uploader or anyone with `push` may drive or abort an upload. */
const mayDrive = (role: Role | null, u: { uploaderId: string | null }, actor: Actor) =>
  u.uploaderId === actor.id || roleCan(role, "push");
const needDrive = (role: Role | null, u: { uploaderId: string | null }, actor: Actor) => {
  if (!mayDrive(role, u, actor)) throw new HttpError(403, "You don't have permission to do that");
};

export async function createUpload(db: Db, actor: Actor, slug: string, input: unknown) {
  const { project: p } = await access(db, actor, slug, "upload");
  const body = parse(schemas.create, input);
  const filename = sanitizeFilename(body.filename);
  if (!filename) throw new HttpError(400, "filename: empty after removing unsafe characters");
  const allowed = ALLOWED[body.purpose];
  if (allowed && !allowed.some((ext) => filename.toLowerCase().endsWith(ext)))
    throw new HttpError(400, `filename: a ${body.purpose} upload must end in ${allowed.join(", ")}`, {
      allowed,
    });
  if (body.size > maxBytes())
    throw new HttpError(413, `size: larger than the ${maxBytes()}-byte upload limit`, {
      max_bytes: maxBytes(),
    });

  const [open] = await db
    .select({ n: count() })
    .from(upload)
    .where(and(eq(upload.projectId, p.id), eq(upload.uploaderId, actor.id), eq(upload.state, "uploading")));
  if (open.n >= MAX_OPEN_UPLOADS)
    throw new HttpError(
      409,
      `You already have ${MAX_OPEN_UPLOADS} unfinished uploads; finish or abort one first`,
    );

  const id = randomUUID();
  const objectKey = store.keys.upload(p.id, id);
  const multipartId = await store.createMultipart(objectKey);
  await db.transaction(async (tx) => {
    await tx.insert(upload).values({
      id,
      projectId: p.id,
      uploaderId: actor.id,
      filename,
      size: body.size,
      objectKey,
      multipartId,
      state: "uploading",
      purpose: body.purpose,
    });
    await audit(tx, actor, p.id, "upload.create", id, { filename, size: body.size, purpose: body.purpose });
  });
  return { id, state: "uploading", part_size: partSizeFor(body.size), part_count: partCount(body.size), filename };
}

export async function listUploads(db: Db, actor: Actor, slug: string) {
  const { project: p, role } = await access(db, actor, slug, "view");
  const mine = role === "client" ? eq(upload.uploaderId, actor.id) : undefined;
  const rows = await db
    .select()
    .from(upload)
    .where(and(eq(upload.projectId, p.id), mine))
    .orderBy(desc(upload.createdAt));
  return rows.map(view);
}

export async function getUpload(db: Db, actor: Actor, slug: string, id: string) {
  const { role, u } = await load(db, actor, slug, id, "view");
  // Parts already in storage, so a client resuming after a dropped connection sends only the rest.
  const parts =
    u.state === "uploading" && u.multipartId && mayDrive(role, u, actor)
      ? (await store.listParts(u.objectKey, u.multipartId)).map((x) => ({
          part_number: x.partNumber,
          size: x.size,
        }))
      : undefined;
  return { ...view(u), part_size: partSizeFor(u.size ?? 0), part_count: partCount(u.size ?? 0), parts };
}

export async function partUrls(db: Db, actor: Actor, slug: string, id: string, input: unknown) {
  const { role, u } = await load(db, actor, slug, id, "upload");
  needDrive(role, u, actor);
  const { parts } = parse(schemas.parts, input);
  const part_numbers = parts.map((x) => x.part_number);
  if (u.state !== "uploading" || !u.multipartId) throw new HttpError(409, `Upload is ${u.state}`);
  const max = partCount(u.size ?? 0);
  const bad = part_numbers.filter((n) => n > max);
  if (bad.length) throw new HttpError(400, `part_numbers: this upload has ${max} parts`, { invalid: bad });
  const urls: Record<string, string> = {};
  for (const x of parts)
    urls[x.part_number] = await store.presignPart(
      u.objectKey,
      u.multipartId,
      x.part_number,
      x.sha256,
      partLength(u.size ?? 0, x.part_number),
    );
  return { urls };
}

async function fail(db: Db, u: typeof upload.$inferSelect, reason: string, detail: object = {}) {
  await store.deleteObject(u.objectKey).catch(() => {});
  await db.transaction(async (tx) => {
    await tx.update(upload).set({ state: "failed" }).where(eq(upload.id, u.id));
    await audit(tx, { id: u.uploaderId ?? "" }, u.projectId, "upload.failed", u.id, { reason, ...detail });
  });
}

export async function completeUpload(db: Db, actor: Actor, slug: string, id: string, input: unknown) {
  const { p, role, u } = await load(db, actor, slug, id, "upload");
  needDrive(role, u, actor);
  const { sha256 } = parse(schemas.complete, input);
  if (u.state !== "uploading" || !u.multipartId) throw new HttpError(409, `Upload is ${u.state}`);
  const size = u.size ?? 0;

  const parts = await store.listParts(u.objectKey, u.multipartId);
  const have = new Set(parts.map((x) => x.partNumber));
  const missing = Array.from({ length: partCount(size) }, (_, i) => i + 1).filter((n) => !have.has(n));
  if (missing.length) throw new HttpError(409, "Some parts have not been uploaded", { missing });
  // Nothing may be assembled from a part of the wrong length: re-upload it (same part number overwrites).
  const bad_parts = parts.filter((x) => x.size !== partLength(size, x.partNumber)).map((x) => x.partNumber);
  if (bad_parts.length) throw new HttpError(409, "Some parts have the wrong size", { bad_parts });

  // Claim the transition first so two completes can't both go on to call S3.
  const claimed = await db
    .update(upload)
    .set({ state: "verifying", sha256 })
    .where(and(eq(upload.id, u.id), eq(upload.state, "uploading")))
    .returning({ id: upload.id });
  if (!claimed.length) throw new HttpError(409, "Upload is already being completed");

  try {
    await store.completeMultipart(
      u.objectKey,
      u.multipartId,
      [...parts].sort((a, b) => a.partNumber - b.partNumber),
    );
  } catch (e) {
    await store.abortMultipart(u.objectKey, u.multipartId).catch(() => {});
    await fail(db, u, "complete failed", { error: (e as Error).message });
    throw new HttpError(502, "Storage refused to complete the upload", { state: "failed" });
  }
  // From here the row is `verifying`. A storage blip must not leave a bare 500: verifyUpload re-checks the size
  // itself, and the worker sweep retries it.
  const stillVerifying = (e: unknown): never => {
    console.error("complete", u.id, e);
    throw new HttpError(502, "Could not finish checking the upload yet; it will be verified shortly", {
      state: "verifying",
    });
  };
  const h = await store.head(u.objectKey).catch(stillVerifying);
  if (!h || h.size !== size || h.size > maxBytes()) {
    await fail(db, u, "size mismatch", { declared: size, actual: h?.size ?? null });
    throw new HttpError(422, "Uploaded size does not match the declared size", {
      state: "failed",
      declared: size,
      actual: h?.size ?? null,
    });
  }
  await audit(db, actor, p.id, "upload.complete", u.id, { size, sha256 }).catch(stillVerifying);
  // Fire and forget: hashing 20 GB must not hold the request. Task 7's worker sweeps anything left in `verifying`.
  void verifyUpload(db, store, u.id).catch((e) => console.error("verifyUpload", u.id, e));
  return { id: u.id, state: "verifying" };
}

export async function abortUpload(db: Db, actor: Actor, slug: string, id: string) {
  const { p, role, u } = await load(db, actor, slug, id, "upload", { archivedOk: true });
  needDrive(role, u, actor);
  if (u.state !== "uploading" || !u.multipartId) throw new HttpError(409, `Upload is ${u.state}`);
  await store.abortMultipart(u.objectKey, u.multipartId).catch((e) => {
    if ((e as { name?: string }).name !== "NoSuchUpload") throw e;
  });
  const moved = await db.transaction(async (tx) => {
    // Guarded: an abort racing a complete must not overwrite `verifying`.
    const r = await tx
      .update(upload)
      .set({ state: "aborted" })
      .where(and(eq(upload.id, u.id), eq(upload.state, "uploading")))
      .returning({ id: upload.id });
    if (r.length) await audit(tx, actor, p.id, "upload.abort", u.id);
    return r.length > 0;
  });
  if (!moved) throw new HttpError(409, "Upload is no longer uploading");
  return { id: u.id, state: "aborted" };
}

/** Client dumps carry personal data: downloading needs `push` (owner, developer), not `pull` (viewers). */
export async function downloadUpload(db: Db, actor: Actor, slug: string, id: string) {
  // Reading only: allowed on an archived project until retention deletes it.
  const { p, u } = await load(db, actor, slug, id, "push", { archivedOk: true });
  if (u.state !== "ready") throw new HttpError(409, `Upload is ${u.state}, not ready`);
  const url = await store.presignGet(u.objectKey, store.URL_TTL_SECONDS, contentDisposition(u.filename));
  await audit(db, actor, p.id, "upload.download", u.id);
  return { url, filename: u.filename, size: u.size, sha256: u.sha256, expires_in: store.URL_TTL_SECONDS };
}
