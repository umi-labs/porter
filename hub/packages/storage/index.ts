import { createHash } from "node:crypto";
import type { Readable } from "node:stream";
import { auditLog, type connect, upload } from "@porter-hub/db";
import { and, eq } from "drizzle-orm";

type Db = ReturnType<typeof connect>["db"];
/** The slice of object storage verification needs; apps/web's storage.ts provides it. No Next imports here. */
export type Store = {
  getStream(key: string): Promise<Readable>;
  deleteObject(key: string): Promise<void>;
};

/**
 * Streams the upload's object, hashes it, and moves `verifying` -> `ready` (hash matches the one the
 * client declared) or `failed` (mismatch; the object is deleted). Writes an audit row either way.
 * Safe to call twice: anything not `verifying` is left alone. The worker (Task 7) sweeps uploads stuck in `verifying`.
 */
export async function verifyUpload(db: Db, store: Store, uploadId: string) {
  const [u] = await db.select().from(upload).where(eq(upload.id, uploadId));
  if (!u) return null;
  if (u.state !== "verifying") return u.state;

  const fail = async (reason: string, detail: object = {}) => {
    const [row] = await db
      .update(upload)
      .set({ state: "failed" })
      .where(and(eq(upload.id, uploadId), eq(upload.state, "verifying")))
      .returning({ id: upload.id });
    if (!row) return "failed" as const;
    await store.deleteObject(u.objectKey).catch(() => {});
    await db.insert(auditLog).values({
      projectId: u.projectId,
      actorId: null,
      action: "upload.failed",
      targetType: "upload",
      targetId: u.id,
      detail: { reason, ...detail },
    });
    return "failed" as const;
  };

  let actual: string;
  try {
    const h = createHash("sha256");
    for await (const chunk of await store.getStream(u.objectKey)) h.update(chunk);
    actual = h.digest("hex");
  } catch (e) {
    // The object couldn't be read (missing, storage down): the upload can't be vouched for.
    return fail("unreadable", { error: (e as Error).message });
  }
  if (actual !== u.sha256) return fail("sha256 mismatch", { expected: u.sha256, actual });

  const [row] = await db
    .update(upload)
    .set({ state: "ready" })
    .where(and(eq(upload.id, uploadId), eq(upload.state, "verifying")))
    .returning({ id: upload.id });
  if (row)
    await db.insert(auditLog).values({
      projectId: u.projectId,
      actorId: null,
      action: "upload.ready",
      targetType: "upload",
      targetId: u.id,
      detail: { sha256: actual },
    });
  return "ready" as const;
}
