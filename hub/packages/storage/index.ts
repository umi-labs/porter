import { createHash } from "node:crypto";
import type { Readable } from "node:stream";
import { auditLog, type connect, upload } from "@porter-hub/db";
import { and, eq } from "drizzle-orm";

const DEFAULT_MAX_BYTES = 20 * 1024 ** 3;
const notFound = (e: unknown) => {
  const x = e as { name?: string; $metadata?: { httpStatusCode?: number } };
  return x.name === "NoSuchKey" || x.name === "NotFound" || x.$metadata?.httpStatusCode === 404;
};

type Db = ReturnType<typeof connect>["db"];
/** The slice of object storage verification needs; apps/web's storage.ts provides it. No Next imports here. */
export type Store = {
  getStream(key: string): Promise<Readable>;
  deleteObject(key: string): Promise<void>;
};

/**
 * Streams the upload's object, hashes it, and moves `verifying` -> `ready` (hash matches the one the
 * client declared) or `failed` (mismatch; the object is deleted). Writes an audit row either way.
 * It is also the size authority: the streamed byte count must equal the declared size and not exceed the max.
 * Only a missing object counts as failure. A transport error or idle timeout is rethrown with the row left in
 * `verifying` and nothing deleted, so a retry (the Task 7 worker sweeps `verifying`) can finish the job.
 * Safe to call twice: anything not `verifying` is left alone.
 */
export async function verifyUpload(
  db: Db,
  store: Store,
  uploadId: string,
  maxBytes = Number(process.env.UPLOAD_MAX_BYTES) || DEFAULT_MAX_BYTES,
) {
  const [u] = await db.select().from(upload).where(eq(upload.id, uploadId));
  if (!u) return null;
  if (u.state !== "verifying") return u.state;

  /** One transaction: the state change and its audit row land together or not at all. */
  const move = (state: "ready" | "failed", action: string, detail: object) =>
    db.transaction(async (tx) => {
      const [row] = await tx
        .update(upload)
        .set({ state })
        .where(and(eq(upload.id, uploadId), eq(upload.state, "verifying")))
        .returning({ id: upload.id });
      if (!row) return false;
      await tx.insert(auditLog).values({
        projectId: u.projectId,
        actorId: null,
        action,
        targetType: "upload",
        targetId: u.id,
        detail,
      });
      return true;
    });
  const fail = async (reason: string, detail: object = {}) => {
    if (await move("failed", "upload.failed", { reason, ...detail }))
      await store.deleteObject(u.objectKey).catch(() => {});
    return "failed" as const;
  };

  let actual: string;
  let bytes = 0;
  try {
    const h = createHash("sha256");
    let over = false;
    for await (const chunk of await store.getStream(u.objectKey)) {
      h.update(chunk);
      bytes += chunk.length;
      if (bytes > maxBytes) {
        over = true;
        break;
      }
    }
    if (over) return fail("size mismatch", { declared: u.size, max: maxBytes, actual: ">max" });
    actual = h.digest("hex");
  } catch (e) {
    if (notFound(e)) return fail("missing", { error: (e as Error).message });
    throw e; // transient: leave `verifying`, delete nothing
  }
  if (bytes !== u.size) return fail("size mismatch", { declared: u.size, actual: bytes });
  if (actual !== u.sha256) return fail("sha256 mismatch", { expected: u.sha256, actual });

  await move("ready", "upload.ready", { sha256: actual });
  return "ready" as const;
}
