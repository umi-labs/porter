import { type connect, run } from "@porter-hub/db";
import { and, eq, sql } from "drizzle-orm";

export type Db = ReturnType<typeof connect>["db"];
export type Run = typeof run.$inferSelect;

export const HEARTBEAT_MS = 15_000;
export const LEASE_MS = 60_000;

/** NOTIFY payloads are the newest seq (or "state" for a state change); listeners re-read from the table. */
export const channel = (runId: string) => `run_${runId}`;
export const notify = (db: Db, runId: string, payload: string) =>
  db.execute(sql`select pg_notify(${channel(runId)}, ${payload})`);

/** Appends events after the run's current last seq (used outside the worker's own ordered writer). */
export async function appendEvent(db: Db, runId: string, event: object) {
  await db.execute(sql`
    insert into run_event (run_id, seq, event)
    select ${runId}, coalesce(max(seq), 0) + 1, ${JSON.stringify(event)}::jsonb from run_event where run_id = ${runId}`);
  await notify(db, runId, "event");
}

/** Takes the oldest queued run, if any, and marks it running with a fresh lease. Safe across workers. */
export async function claim(db: Db): Promise<Run | null> {
  const rows = await db.execute(sql`
    update run set state = 'running', started_at = now(), heartbeat_at = now(), updated_at = now()
    where id = (select id from run where state = 'queued' order by created_at
                for update skip locked limit 1)
    returning id`);
  const id = (rows as unknown as { id: string }[])[0]?.id;
  if (!id) return null;
  const [r] = await db.select().from(run).where(eq(run.id, id));
  await notify(db, id, "state");
  return r;
}

/** Refreshes the lease. False means the run is no longer ours (reclaimed as lost): stop it, keep nothing. */
export async function heartbeat(db: Db, runId: string) {
  const r = await db
    .update(run)
    .set({ heartbeatAt: sql`now()` })
    .where(and(eq(run.id, runId), eq(run.state, "running")))
    .returning({ id: run.id });
  return r.length > 0;
}

/** Fails every running run whose lease is older than LEASE_MS ("worker lost"), with a run_event saying so. */
export async function reclaimLost(db: Db, leaseMs = LEASE_MS) {
  const lost = await db
    .update(run)
    .set({ state: "failed", error: "worker lost", finishedAt: new Date() })
    .where(
      and(
        eq(run.state, "running"),
        sql`${run.heartbeatAt} < now() - make_interval(secs => ${leaseMs / 1000})`,
      ),
    )
    .returning({ id: run.id });
  for (const { id } of lost) {
    await appendEvent(db, id, {
      event: "hub-note",
      kind: "worker-lost",
      message: "worker lost: no heartbeat for over 60 s; the run was marked failed",
    });
    await notify(db, id, "state");
  }
  return lost.map((r) => r.id);
}
