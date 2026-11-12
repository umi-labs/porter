import { auditLog, upload } from "@porter-hub/db";
import { verifyUpload } from "@porter-hub/storage";
import { abortMultipart, deleteObject, getStream } from "@porter-hub/storage/s3";
import { and, eq, lt } from "drizzle-orm";
import type { Db } from "./claim";

export const SWEEP_MS = 60_000;
/** A `verifying` upload this old means the web process died mid-hash. */
export const STUCK_VERIFYING_MS = 10 * 60_000;
/** An upload still `uploading` after this long is abandoned. */
export const ABANDONED_MS = 7 * 24 * 3_600_000;

/** Finishes stuck verifications and aborts abandoned multipart uploads. Each row's failure is logged and skipped. */
export async function sweepUploads(db: Db, now = Date.now()) {
  const stuck = await db
    .select({ id: upload.id })
    .from(upload)
    .where(and(eq(upload.state, "verifying"), lt(upload.updatedAt, new Date(now - STUCK_VERIFYING_MS))));
  for (const { id } of stuck) {
    // verifyUpload writes its own audit row (upload.ready / upload.failed) and only acts on `verifying`.
    await verifyUpload(db, { getStream: (k) => getStream(k), deleteObject }, id).catch((e) =>
      console.error(`sweep: verifying upload ${id} failed:`, e),
    );
  }

  const abandoned = await db
    .select()
    .from(upload)
    .where(and(eq(upload.state, "uploading"), lt(upload.createdAt, new Date(now - ABANDONED_MS))));
  for (const u of abandoned) {
    try {
      if (u.multipartId)
        await abortMultipart(u.objectKey, u.multipartId).catch((e) => {
          if ((e as { name?: string }).name !== "NoSuchUpload") throw e;
        });
      await db.transaction(async (tx) => {
        const [row] = await tx
          .update(upload)
          .set({ state: "aborted" })
          .where(and(eq(upload.id, u.id), eq(upload.state, "uploading")))
          .returning({ id: upload.id });
        if (row)
          await tx.insert(auditLog).values({
            projectId: u.projectId,
            actorId: null,
            action: "upload.abort",
            targetType: "upload",
            targetId: u.id,
            detail: { reason: "abandoned", age_days: 7 },
          });
      });
    } catch (e) {
      console.error(`sweep: aborting upload ${u.id} failed:`, e);
    }
  }
  return { verified: stuck.length, aborted: abandoned.length };
}
