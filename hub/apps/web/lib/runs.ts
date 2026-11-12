import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { projectVersion, run, runEvent } from "@porter-hub/db";
import { destinationHost, STAGES } from "@porter-hub/porter/config";
import { and, asc, desc, eq, gt, inArray, sql } from "drizzle-orm";
import { alias } from "drizzle-orm/pg-core";
import { z } from "zod";
import type { Listener } from "./listen";
import type { Db } from "./permissions";
import { type Actor, access, audit, HttpError, parse } from "./projects";
import { versionDir } from "./snapshot-reader";

const ACTIVE = ["queued", "running"] as const;
const TERMINAL = new Set(["finished", "failed", "cancelled"]);
const REPLAY_BATCH = 1000;
const KEEPALIVE_MS = 15_000;

const schemas = {
  create: z.object({
    stages: z.union([z.literal("all"), z.array(z.enum(STAGES)).min(1).max(STAGES.length)]),
    apply: z.boolean().default(false),
    confirm_host: z.string().trim().max(253).optional(),
  }),
  id: z.uuid(),
  after: z.coerce.number().int().min(0),
};

/** The version's porter.toml (from the cached extracted snapshot). */
export type ReadToml = (v: typeof projectVersion.$inferSelect) => Promise<string>;
const readToml: ReadToml = async (v) => readFile(join(await versionDir(v), "porter.toml"), "utf8");

const base = alias(projectVersion, "base");
const result = alias(projectVersion, "result");
const runRows = (db: Db) =>
  db
    .select({ r: run, base: base.number, result: result.number })
    .from(run)
    .leftJoin(base, eq(base.id, run.baseVersionId))
    .leftJoin(result, eq(result.id, run.resultVersionId));
type RunRow = { r: typeof run.$inferSelect; base: number | null; result: number | null };
export const runJson = ({ r, base, result }: RunRow) => ({
  id: r.id,
  state: r.state,
  stages: r.stages[0] === "all" ? ("all" as const) : r.stages,
  apply: r.apply,
  exit_code: r.exitCode,
  error: r.error,
  base_version: base,
  result_version: result,
  requested_by: r.requestedBy,
  cancel_requested: r.cancelRequested,
  created_at: r.createdAt,
  started_at: r.startedAt,
  finished_at: r.finishedAt,
});
export type RunJson = ReturnType<typeof runJson>;

const isActiveConflict = (e: unknown) => {
  const x = e as {
    code?: string;
    constraint_name?: string;
    cause?: { code?: string; constraint_name?: string };
  };
  return (
    (x.cause?.code ?? x.code) === "23505" &&
    (x.cause?.constraint_name ?? x.constraint_name) === "run_one_active_per_project"
  );
};

/** Queues a run on the latest version. Apply needs `confirm_host` equal to that version's destination host. */
export async function createRun(db: Db, actor: Actor, slug: string, input: unknown, toml = readToml) {
  const { project: p } = await access(db, actor, slug, "run");
  const body = parse(schemas.create, input);
  if (body.apply && body.stages !== "all" && !body.stages.includes("load"))
    throw new HttpError(400, "apply: only meaningful with the load stage (or all)");
  const [v] = await db
    .select()
    .from(projectVersion)
    .where(eq(projectVersion.projectId, p.id))
    .orderBy(desc(projectVersion.number))
    .limit(1);
  if (!v) throw new HttpError(409, "The project has no version yet; push one first");
  if (body.apply) {
    const host = destinationHost(await toml(v));
    if (!host) throw new HttpError(409, "porter.toml has no [destination] url to apply to");
    if ((body.confirm_host ?? "").toLowerCase() !== host.toLowerCase())
      throw new HttpError(400, "confirm_host: does not match the destination host in porter.toml");
  }
  const [busy] = await db
    .select({ id: run.id })
    .from(run)
    .where(and(eq(run.projectId, p.id), inArray(run.state, [...ACTIVE])));
  if (busy)
    throw new HttpError(409, "A run is already queued or running for this project", { run_id: busy.id });
  try {
    const r = await db.transaction(async (tx) => {
      const [r] = await tx
        .insert(run)
        .values({
          projectId: p.id,
          baseVersionId: v.id,
          stages:
            body.stages === "all" ? ["all"] : STAGES.filter((s) => (body.stages as string[]).includes(s)),
          apply: body.apply,
          requestedBy: actor.id,
        })
        .returning();
      await audit(tx, actor, p.id, "run.create", r.id, { stages: r.stages, apply: r.apply, base: v.number });
      return r;
    });
    return runJson({ r, base: v.number, result: null });
  } catch (e) {
    if (isActiveConflict(e)) throw new HttpError(409, "A run is already queued or running for this project");
    throw e;
  }
}

export async function listRuns(db: Db, actor: Actor, slug: string, limit = 50) {
  const { project: p } = await access(db, actor, slug, "view_raw");
  const rows = await runRows(db).where(eq(run.projectId, p.id)).orderBy(desc(run.createdAt)).limit(limit);
  return rows.map(runJson);
}

async function loadRun(db: Db, projectId: string, id: string) {
  const runId = parse(schemas.id, id);
  const [row] = await runRows(db).where(and(eq(run.id, runId), eq(run.projectId, projectId)));
  if (!row) throw new HttpError(404, "Run not found");
  return row;
}

export async function getRun(db: Db, actor: Actor, slug: string, id: string) {
  const { project: p } = await access(db, actor, slug, "view_raw");
  return runJson(await loadRun(db, p.id, id));
}

const notifyRun = (db: Db, runId: string) => db.execute(sql`select pg_notify(${`run_${runId}`}, 'state')`);

/** Queued: cancelled at once. Running: flags it; the worker sends porter SIGINT (SIGKILL 30 s later). */
export async function cancelRun(db: Db, actor: Actor, slug: string, id: string) {
  const { project: p } = await access(db, actor, slug, "run");
  const row = await loadRun(db, p.id, id);
  if (TERMINAL.has(row.r.state))
    throw new HttpError(409, `The run is already ${row.r.state}`, { state: row.r.state });
  await db.transaction(async (tx) => {
    const [q] = await tx
      .update(run)
      .set({ state: "cancelled", cancelRequested: true, finishedAt: new Date() })
      .where(and(eq(run.id, row.r.id), eq(run.state, "queued")))
      .returning({ id: run.id });
    if (!q) {
      const [running] = await tx
        .update(run)
        .set({ cancelRequested: true })
        .where(and(eq(run.id, row.r.id), eq(run.state, "running")))
        .returning({ id: run.id });
      if (!running) {
        // It ended between the read above and these updates.
        const [now] = await tx.select({ state: run.state }).from(run).where(eq(run.id, row.r.id));
        throw new HttpError(409, `The run is already ${now?.state}`, { state: now?.state });
      }
    }
    await audit(tx, actor, p.id, "run.cancel", row.r.id, { was: q ? "queued" : "running" });
  });
  await notifyRun(db, row.r.id);
  return runJson(await loadRun(db, p.id, id));
}

/**
 * Server-sent events: replays the run's events after `Last-Event-ID` (or `?after=`), then streams new ones as
 * the worker NOTIFYs. Each message is `id: <seq>` + `data: <event JSON>`. When the run is over and every event
 * has been sent, one `event: end` with the run JSON closes the stream (EventSource clients must then close()).
 * Access is checked once, at connect: a member removed mid-stream keeps receiving this run's log until it ends.
 */
export async function runEvents(
  db: Db,
  listener: Listener,
  actor: Actor,
  slug: string,
  id: string,
  req: Request,
) {
  const { project: p } = await access(db, actor, slug, "view_raw");
  const row = await loadRun(db, p.id, id);
  const runId = row.r.id;
  const q = new URL(req.url).searchParams.get("after");
  let last = parse(schemas.after, req.headers.get("last-event-id") ?? q ?? 0);
  const enc = new TextEncoder();
  let stop = () => {};

  const stream = new ReadableStream<Uint8Array>({
    async start(ctl) {
      let closed = false;
      let busy = false;
      let again = false;
      const send = (s: string) => {
        if (!closed) ctl.enqueue(enc.encode(s));
      };
      const pump = async () => {
        if (busy) {
          again = true;
          return;
        }
        busy = true;
        try {
          do {
            again = false;
            // State before events: the worker stores every event before it sets a final state.
            const [st] = await db.select({ state: run.state }).from(run).where(eq(run.id, runId));
            let n: number;
            do {
              const evs = await db
                .select({ seq: runEvent.seq, event: runEvent.event })
                .from(runEvent)
                .where(and(eq(runEvent.runId, runId), gt(runEvent.seq, last)))
                .orderBy(asc(runEvent.seq))
                .limit(REPLAY_BATCH);
              for (const e of evs) send(`id: ${e.seq}\ndata: ${JSON.stringify(e.event)}\n\n`);
              n = evs.length;
              if (n) last = evs[n - 1].seq;
            } while (n === REPLAY_BATCH && !closed);
            if (st && TERMINAL.has(st.state)) {
              send(`event: end\ndata: ${JSON.stringify(runJson(await loadRun(db, p.id, runId)))}\n\n`);
              stop();
              return;
            }
          } while (again && !closed);
        } catch (e) {
          console.error(`sse run ${runId}:`, e);
          stop();
        } finally {
          busy = false;
        }
      };
      // Subscribe before the replay so nothing written in between is missed.
      const unsubscribe = await listener.subscribe(`run_${runId}`, () => void pump());
      const ping = setInterval(() => send(": ping\n\n"), KEEPALIVE_MS);
      stop = () => {
        if (closed) return;
        closed = true;
        clearInterval(ping);
        unsubscribe();
        try {
          ctl.close();
        } catch {}
      };
      req.signal.addEventListener("abort", () => stop());
      send("retry: 2000\n\n");
      await pump();
    },
    cancel() {
      stop();
    },
  });
  return new Response(stream, {
    headers: {
      "content-type": "text/event-stream; charset=utf-8",
      "cache-control": "no-store",
      "x-accel-buffering": "no",
    },
  });
}
