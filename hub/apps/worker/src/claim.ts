import { randomUUID } from "node:crypto";
import { hostname } from "node:os";
import { type connect, run, runEvent } from "@porter-hub/db";
import { and, eq, sql } from "drizzle-orm";

export type Db = ReturnType<typeof connect>["db"];
export type Run = typeof run.$inferSelect;

export const HEARTBEAT_MS = 15_000;
export const LEASE_MS = 60_000;
/** This process's lease identity. */
export const WORKER_ID = `${hostname()}:${process.pid}:${randomUUID().slice(0, 8)}`;

/** NOTIFY payloads are the newest seq (or "state" for a state change); listeners re-read from the table. */
export const channel = (runId: string) => `run_${runId}`;
export const notify = (db: Db | Tx, runId: string, payload: string) =>
  db.execute(sql`select pg_notify(${channel(runId)}, ${payload})`);
type Tx = Parameters<Parameters<Db["transaction"]>[0]>[0];

/** Takes the oldest queued run, if any, and marks it running with a fresh lease owned by `workerId`. */
export async function claim(db: Db, workerId = WORKER_ID): Promise<Run | null> {
  const rows = await db.execute(sql`
    update run set state = 'running', started_at = now(), heartbeat_at = now(), worker_id = ${workerId},
      updated_at = now()
    where id = (select id from run where state = 'queued' order by created_at
                for update skip locked limit 1)
    returning id`);
  const id = (rows as unknown as { id: string }[])[0]?.id;
  if (!id) return null;
  const [r] = await db.select().from(run).where(eq(run.id, id));
  await notify(db, id, "state");
  return r;
}

/** Refreshes the lease. False means the run is no longer ours (reclaimed, or claimed by another worker). */
export async function heartbeat(db: Db, runId: string, workerId: string | null) {
  const r = await db
    .update(run)
    .set({ heartbeatAt: sql`now()` })
    .where(
      and(eq(run.id, runId), eq(run.state, "running"), sql`${run.workerId} is not distinct from ${workerId}`),
    )
    .returning({ id: run.id });
  return r.length > 0;
}

/**
 * Fails every running run whose lease is older than LEASE_MS ("worker lost"). The note and the state change
 * land in one transaction, so an SSE stream that sees the final state also sees the note.
 */
export async function reclaimLost(db: Db, leaseMs = LEASE_MS) {
  return db.transaction(async (tx) => {
    const lost = await tx
      .select({ id: run.id })
      .from(run)
      .where(
        and(
          eq(run.state, "running"),
          sql`${run.heartbeatAt} < now() - make_interval(secs => ${leaseMs / 1000})`,
        ),
      )
      .for("update", { skipLocked: true });
    for (const { id } of lost) {
      await tx.execute(sql`
        insert into ${runEvent} (run_id, seq, event)
        select ${id}, coalesce(max(seq), 0) + 1, ${JSON.stringify({
          event: "hub-note",
          kind: "worker-lost",
          message: "worker lost: no heartbeat for over 60 s; the run was marked failed",
        })}::jsonb from ${runEvent} where run_id = ${id}`);
      await tx
        .update(run)
        .set({ state: "failed", error: "worker lost", finishedAt: new Date() })
        .where(eq(run.id, id));
      await notify(tx, id, "state");
    }
    return lost.map((r) => r.id);
  });
}
