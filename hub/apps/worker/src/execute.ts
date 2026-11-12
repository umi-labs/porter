import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pipeline } from "node:stream/promises";
import { promisify } from "node:util";
import {
  auditLog,
  mediaObject,
  project,
  projectVersion,
  run,
  runEvent,
  runSecret,
  versionMedia,
} from "@porter-hub/db";
import {
  commandsFor,
  decryptSecret,
  type Exit,
  KILL_AFTER_MS,
  redactor,
  type Spawned,
  type StageName,
  type StoredEvent,
  signalGroup,
  spawnPorter,
  stripWpDb,
  Tail,
} from "@porter-hub/porter";
import { deleteObject, getStream, keys, putFile } from "@porter-hub/storage/s3";
import { extractTarZst, packTarZst } from "@porter-hub/storage/tar";
import { and, eq, inArray, sql } from "drizzle-orm";
import { type Db, HEARTBEAT_MS, heartbeat, LEASE_MS, notify, type Run } from "./claim";

export type Settings = {
  bin: string;
  scratchDir: string;
  killAfterMs?: number;
  heartbeatMs?: number;
  cancelPollMs?: number;
  leaseMs?: number;
};
type Tx = Parameters<Parameters<Db["transaction"]>[0]>[0];
type Final = { state: "finished" | "failed" | "cancelled"; exitCode: number | null; error: string | null };

const HASH = /^[0-9a-f]{64}$/;
const MEDIA_CONCURRENCY = 8;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const maxResultBytes = () => Number(process.env.HUB_SNAPSHOT_MAX_BYTES) || 2 * 1024 ** 3;
const WP_DB_NOTE = "wp-db connector skipped on the hub; run it locally and push";
const sha256File = async (path: string) => {
  const h = createHash("sha256");
  for await (const c of createReadStream(path)) h.update(c);
  return h.digest("hex");
};
/** porter's media layout: media/<h0h1>/<h2h3>/<sha256> (crates/porter-engine/src/store/media.rs). */
const mediaPath = (dir: string, h: string) => join(dir, "media", h.slice(0, 2), h.slice(2, 4), h);

async function mapLimit<T>(items: T[], n: number, fn: (x: T) => Promise<void>) {
  let i = 0;
  await Promise.all(
    Array.from({ length: Math.min(n, items.length) }, async () => {
      while (i < items.length) await fn(items[i++]);
    }),
  );
}

/**
 * Ordered, batched run_event writer: rows go in seq order, each batch followed by a NOTIFY. A seq collision
 * (another writer, e.g. a reclaim note, took the number) re-reads max(seq) and renumbers the batch once.
 */
function eventWriter(db: Db, runId: string, redact: () => (e: StoredEvent) => StoredEvent) {
  let seq = 0;
  let buf: { runId: string; seq: number; event: StoredEvent }[] = [];
  let chain: Promise<void> = Promise.resolve();
  let timer: NodeJS.Timeout | undefined;
  const maxSeq = async (q: Db | Tx) => {
    const [r] = await q
      .select({ n: sql<number>`coalesce(max(${runEvent.seq}), 0)` })
      .from(runEvent)
      .where(eq(runEvent.runId, runId));
    return Number(r.n);
  };
  const insert = async (rows: typeof buf) => {
    try {
      await db.insert(runEvent).values(rows);
    } catch (e) {
      const code =
        (e as { code?: string; cause?: { code?: string } }).cause?.code ?? (e as { code?: string }).code;
      if (code !== "23505") throw e;
      let n = await maxSeq(db);
      for (const r of rows) r.seq = ++n;
      seq = Math.max(seq, n);
      await db.insert(runEvent).values(rows);
    }
  };
  const flush = () => {
    clearTimeout(timer);
    timer = undefined;
    const rows = buf;
    buf = [];
    if (rows.length)
      chain = chain
        .then(async () => {
          await insert(rows);
          await notify(db, runId, String(rows[rows.length - 1].seq));
        })
        .catch((e) => console.error(`run ${runId}: storing events failed:`, e));
    return chain;
  };
  return {
    async init() {
      seq = await maxSeq(db);
    },
    push(event: StoredEvent) {
      buf.push({ runId, seq: ++seq, event: redact()(event) });
      if (buf.length >= 200) flush();
      else timer ??= setTimeout(flush, 100);
    },
    /** Writes `events` inside `tx`, so they commit with it. Flush before opening `tx`. */
    async insertIn(tx: Tx, events: StoredEvent[]) {
      if (!events.length) return;
      seq = Math.max(seq, await maxSeq(tx));
      await tx.insert(runEvent).values(events.map((e) => ({ runId, seq: ++seq, event: redact()(e) })));
    },
    flush,
  };
}

/** Porter processes this worker is running, by run id (for shutdown). */
export const active = new Map<string, Spawned>();
/** SIGINT every porter process group (SIGKILL 30 s later), e.g. when the worker is told to stop. */
export function interruptAll(killAfterMs = KILL_AFTER_MS) {
  for (const p of active.values()) p.interrupt(killAfterMs);
}

/**
 * Deletes scratch dirs (`<scratchDir>/<runId>`) whose run isn't running, SIGKILLing any porter process group
 * still recorded in their porter.pid (left over from a crashed worker). `keep`: runs this process is executing.
 */
export async function cleanScratch(db: Db, scratchDir: string, keep: Iterable<string> = active.keys()) {
  const mine = new Set(keep);
  const ids = (await readdir(scratchDir).catch(() => [] as string[])).filter(
    (n) => UUID.test(n) && !mine.has(n),
  );
  if (!ids.length) return [];
  const running = new Set(
    (
      await db
        .select({ id: run.id })
        .from(run)
        .where(and(inArray(run.id, ids), eq(run.state, "running")))
    ).map((r) => r.id),
  );
  const removed: string[] = [];
  for (const id of ids.filter((i) => !running.has(i))) {
    const pid = Number(await readFile(join(scratchDir, id, "porter.pid"), "utf8").catch(() => ""));
    if (Number.isInteger(pid) && pid > 1) signalGroup(pid, "SIGKILL");
    await rm(join(scratchDir, id), { recursive: true, force: true });
    removed.push(id);
  }
  return removed;
}

/** Thrown inside the commit transaction when the run is no longer ours to finish. */
class LeaseLost extends Error {}

/** Exit of the last porter process -> the run's final state. `sawIssues`: an earlier stage exited 2. */
export function outcome(x: Exit, cancelRequested: boolean, sawIssues = false) {
  type O = Final;
  const o = (state: O["state"], exitCode: number | null, error: string | null = null): O => ({
    state,
    exitCode,
    error,
  });
  if (x.error) return o("failed", null, `porter could not start: ${x.error}`);
  if (x.code === 0) return o("finished", sawIssues ? 2 : 0);
  if (x.code === 2) return o("finished", 2);
  if (x.code === 3) return cancelRequested ? o("cancelled", 3) : o("failed", 3, "interrupted");
  if (x.signal)
    return cancelRequested ? o("cancelled", null) : o("failed", null, `porter was killed by ${x.signal}`);
  if (x.code === 1) return o("failed", 1, "porter could not run (exit 1)");
  return o("failed", x.code, `porter exited with ${x.code}`);
}

const execFileP = promisify(execFile);

/**
 * Runs one claimed run to completion: snapshot + media into a scratch dir, wp-db stripped, secrets as env only
 * (and redacted from everything stored), porter spawned per stage in its own process group, events stored in
 * order, then the result packed and committed as a new version together with the final state, only while
 * this worker still holds the lease. The scratch dir is always deleted.
 */
export async function execute(db: Db, r: Run, s: Settings) {
  const scratch = join(s.scratchDir, r.id);
  const dir = join(scratch, "project");
  let redact = (t: string) => t;
  const redactEvent = (e: StoredEvent): StoredEvent => {
    const json = JSON.stringify(e);
    const clean = redact(json);
    if (clean === json) return e;
    try {
      return JSON.parse(clean) as StoredEvent;
    } catch {
      return {
        event: "hub-note",
        kind: "redacted",
        message: "an event contained a run secret and was withheld",
      };
    }
  };
  const events = eventWriter(db, r.id, () => redactEvent);
  const tail = new Tail();
  const env = {
    PATH: process.env.PATH ?? "/usr/bin:/bin",
    HOME: scratch,
    LANG: process.env.LANG || "C.UTF-8",
  };
  const leaseMs = s.leaseMs ?? LEASE_MS;
  let lost = false;
  let lastBeat = Date.now();
  let cancelRequested = false;
  let current: Spawned | undefined;
  const timers: NodeJS.Timeout[] = [];
  const markLost = () => {
    lost = true;
    current?.kill();
  };
  /** Our lease, checked under a row lock: still running and still claimed by us. */
  const holdLease = async (tx: Tx) => {
    const [x] = await tx
      .select({ state: run.state, workerId: run.workerId })
      .from(run)
      .where(eq(run.id, r.id))
      .for("no key update"); // not plain "update": run_event's FK check (key share) must not block on it
    if (lost || x?.state !== "running" || x.workerId !== r.workerId) throw new LeaseLost();
  };
  const finishIn = async (tx: Tx, f: Final, resultVersionId?: string) => {
    const error = f.error && redact(f.error);
    await tx
      .update(run)
      .set({ state: f.state, exitCode: f.exitCode, error, finishedAt: new Date(), resultVersionId })
      .where(eq(run.id, r.id));
    await tx.insert(auditLog).values({
      projectId: r.projectId,
      actorId: null,
      action: "run.finish",
      targetType: "run",
      targetId: r.id,
      detail: { state: f.state, exit_code: f.exitCode, error },
    });
    await notify(tx, r.id, "state");
  };
  /** Final state without a result version (nothing to keep). A run that isn't ours any more is left alone. */
  const finish = async (state: Final["state"], exitCode: number | null, error: string | null) => {
    await events.flush();
    await db
      .transaction(async (tx) => {
        await holdLease(tx);
        await finishIn(tx, { state, exitCode, error });
      })
      .catch((e) => {
        if (!(e instanceof LeaseLost)) throw e;
      });
  };
  const cancelFlag = async () => {
    const [x] = await db.select({ c: run.cancelRequested }).from(run).where(eq(run.id, r.id));
    return !!x?.c;
  };

  try {
    await events.init();
    await rm(scratch, { recursive: true, force: true });
    await mkdir(dir, { recursive: true });

    timers.push(
      setInterval(async () => {
        try {
          if (await heartbeat(db, r.id, r.workerId)) lastBeat = Date.now();
          else markLost(); // reclaimed, or claimed by someone else
        } catch {
          // Can't reach the database: once that outlasts the lease, someone may have reclaimed the run.
          if (Date.now() - lastBeat > leaseMs) markLost();
        }
      }, s.heartbeatMs ?? HEARTBEAT_MS),
      setInterval(async () => {
        if (cancelRequested) return;
        if (await cancelFlag().catch(() => false)) {
          cancelRequested = true;
          current?.interrupt(s.killAfterMs ?? KILL_AFTER_MS);
        }
      }, s.cancelPollMs ?? 1000),
    );

    // ---- setup: anything failing here means porter never started, so there is no result to keep ----
    const [base] = r.baseVersionId
      ? await db.select().from(projectVersion).where(eq(projectVersion.id, r.baseVersionId))
      : [];
    if (!base) return await finish("failed", null, "the base version no longer exists");
    const hash = createHash("sha256");
    await extractTarZst(await getStream(base.snapshotKey), dir, undefined, hash);
    if (base.snapshotSha256 && hash.digest("hex") !== base.snapshotSha256)
      return await finish("failed", null, "the base snapshot does not match its checksum");
    const originalToml = await readFile(join(dir, "porter.toml")).catch(() => null);
    if (!originalToml) return await finish("failed", null, "the base snapshot has no porter.toml");

    const stripped = stripWpDb(originalToml.toString("utf8"));
    if (stripped.removed) {
      events.push({ event: "hub-note", kind: "access-gap", message: WP_DB_NOTE });
      if (!stripped.left) return await finish("failed", null, WP_DB_NOTE);
      await writeFile(join(dir, "porter.toml"), stripped.toml ?? "", { mode: 0o600 });
    }

    const media = await db
      .select({ sha256: versionMedia.sha256 })
      .from(versionMedia)
      .where(eq(versionMedia.versionId, base.id));
    await mapLimit(media, MEDIA_CONCURRENCY, async ({ sha256 }) => {
      const out = mediaPath(dir, sha256);
      await mkdir(join(out, ".."), { recursive: true });
      await pipeline(await getStream(keys.media(r.projectId, sha256)), createWriteStream(out));
    });

    const secrets = await db.select().from(runSecret).where(eq(runSecret.projectId, r.projectId));
    const values: Record<string, string> = {};
    for (const x of secrets) values[x.name] = decryptSecret(r.projectId, x.name, x);
    redact = redactor(values);
    const runEnv: Record<string, string> = { ...env, ...values };

    // ---- run porter, one invocation per stage (or one `porter run` for all) ----
    const stages = r.stages[0] === "all" ? "all" : (r.stages as StageName[]);
    let last: Exit | undefined;
    let sawIssues = false;
    let stoppedForCancel = false;
    for (const args of commandsFor(stages, r.apply)) {
      if (lost) break;
      // Checked against the database too, so a cancel between stages is never missed by the 1 s poll.
      if (cancelRequested || (await cancelFlag())) {
        cancelRequested = true;
        stoppedForCancel = true;
        break;
      }
      current = spawnPorter({ bin: s.bin, dir, args, env: runEnv, onEvent: events.push, tail });
      active.set(r.id, current);
      await writeFile(join(scratch, "porter.pid"), String(current.pid ?? "")).catch(() => {});
      last = await current.done;
      current.kill(); // reap anything porter left behind in its group
      active.delete(r.id);
      current = undefined;
      if (last.code === 2) sawIssues = true;
      if (last.error || (last.code !== 0 && last.code !== 2)) break;
    }
    if (lost) return; // reclaimed as "worker lost": the run is already failed; keep nothing
    if (!last) return await finish("cancelled", null, null); // cancelled before porter started
    // Cancelled between stages: the rest never ran, so the run is not "finished".
    const o: Final = stoppedForCancel
      ? { state: "cancelled", exitCode: null, error: null }
      : outcome(last, cancelRequested, sawIssues);
    if (o.state === "failed" && tail.toString())
      events.push({ event: "hub-stderr", text: redact(tail.toString()) });
    if (last.error) return await finish(o.state, o.exitCode, o.error); // could not even start

    // ---- result: pack, upload, then commit the version and the final state in one transaction ----
    await commitResult(db, r, base, dir, scratch, originalToml, s.bin, env, o, {
      note: events.push,
      insertIn: events.insertIn,
      holdLease,
      finishIn,
      finish,
      flush: events.flush,
    });
  } catch (e) {
    console.error(`run ${r.id}:`, e);
    if (!lost) await finish("failed", null, `hub error: ${(e as Error).message}`).catch(() => {});
  } finally {
    for (const t of timers) clearInterval(t);
    current?.kill();
    active.delete(r.id);
    await events.flush();
    await rm(scratch, { recursive: true, force: true });
  }
}

async function commitResult(
  db: Db,
  r: Run,
  base: typeof projectVersion.$inferSelect,
  dir: string,
  scratch: string,
  originalToml: Buffer,
  bin: string,
  env: Record<string, string>,
  final: Final,
  hooks: {
    note: (e: StoredEvent) => void;
    insertIn: (tx: Tx, e: StoredEvent[]) => Promise<void>;
    holdLease: (tx: Tx) => Promise<void>;
    finishIn: (tx: Tx, f: Final, resultVersionId?: string) => Promise<void>;
    finish: (state: Final["state"], exitCode: number | null, error: string | null) => Promise<void>;
    flush: () => Promise<void>;
  },
) {
  // The result must stay readable by the hub (extractTarZst caps the extracted bytes).
  let total = 0;
  for (const e of await readdir(dir, { recursive: true, withFileTypes: true }))
    if (e.isFile() && !join(e.parentPath, e.name).startsWith(join(dir, "media")))
      total += (await stat(join(e.parentPath, e.name))).size;
  const limit = maxResultBytes();
  if (total > limit)
    return hooks.finish(
      "failed",
      final.exitCode,
      `the result is ${total} bytes, over the hub's snapshot limit (HUB_SNAPSHOT_MAX_BYTES=${limit}); not kept`,
    );

  // Media porter fetched or kept: upload the ones the hub doesn't have, by hash.
  const onDisk: string[] = [];
  for (const a of await readdir(join(dir, "media")).catch(() => []))
    for (const b of await readdir(join(dir, "media", a)).catch(() => []))
      for (const h of await readdir(join(dir, "media", a, b)).catch(() => []))
        if (HASH.test(h) && h.startsWith(a + b)) onDisk.push(h);
  const have = new Set<string>();
  for (let i = 0; i < onDisk.length; i += 5000) {
    const rows = await db
      .select({ sha256: mediaObject.sha256 })
      .from(mediaObject)
      .where(
        and(eq(mediaObject.projectId, r.projectId), inArray(mediaObject.sha256, onDisk.slice(i, i + 5000))),
      );
    for (const x of rows) have.add(x.sha256);
  }
  const fresh: { sha256: string; size: number }[] = [];
  await mapLimit(
    onDisk.filter((h) => !have.has(h)),
    MEDIA_CONCURRENCY,
    async (h) => {
      const path = mediaPath(dir, h);
      if ((await sha256File(path)) !== h) {
        hooks.note({
          event: "hub-note",
          kind: "media-mismatch",
          message: `media/${h} does not match its name; not kept`,
        });
        return;
      }
      const { size } = await stat(path);
      await putFile(keys.media(r.projectId, h), path, h, size);
      fresh.push({ sha256: h, size });
    },
  );
  const kept = [...have, ...fresh.map((f) => f.sha256)];

  // The snapshot keeps the original porter.toml (never the stripped copy) and leaves media/ out.
  const out = join(scratch, "result.tar.zst");
  await packTarZst(dir, out, (rel) => rel === "media", { "porter.toml": originalToml });
  const snapshotSha256 = await sha256File(out);
  const { size } = await stat(out);
  const snapshotKey = keys.snapshot(r.projectId, r.id);
  await putFile(snapshotKey, out, snapshotSha256, size);

  const quiet = { env, timeout: 60_000, maxBuffer: 16 * 1024 * 1024 };
  const porterVersion = await execFileP(bin, ["--version"], quiet).then(
    (x) => x.stdout.trim().replace(/^porter\s+/, ""),
    () => "unknown",
  );
  const summary = await execFileP(bin, ["--json", "--project", dir, "status"], quiet).then(
    (x) => {
      try {
        return JSON.parse(x.stdout.split("\n")[0]) as Record<string, unknown>;
      } catch {
        return {};
      }
    },
    () => ({}),
  );

  const notes: StoredEvent[] = [];
  await hooks.flush(); // everything buffered goes in before the commit transaction
  try {
    await db.transaction(async (tx) => {
      await hooks.holdLease(tx); // still running, still ours: else nothing is committed
      await tx.select({ id: project.id }).from(project).where(eq(project.id, r.projectId)).for("update");
      const [l] = await tx
        .select({ n: sql<number>`coalesce(max(${projectVersion.number}), 0)` })
        .from(projectVersion)
        .where(eq(projectVersion.projectId, r.projectId));
      const latest = Number(l.n);
      const n = latest + 1;
      if (latest > base.number)
        notes.push({
          event: "hub-note",
          kind: "newer-version",
          message: `v${latest} was pushed while this run was going; the result is v${n}, based on v${base.number}`,
        });
      const [v] = await tx
        .insert(projectVersion)
        .values({
          projectId: r.projectId,
          number: n,
          snapshotKey,
          snapshotSha256,
          porterVersion,
          createdBy: r.requestedBy,
          size,
          summary,
          message: `remote run ${r.id} of v${base.number}`,
        })
        .returning({ id: projectVersion.id });
      if (fresh.length)
        await tx
          .insert(mediaObject)
          .values(fresh.map((f) => ({ projectId: r.projectId, sha256: f.sha256, size: f.size })))
          .onConflictDoNothing();
      for (let i = 0; i < kept.length; i += 5000)
        await tx
          .insert(versionMedia)
          .values(
            kept.slice(i, i + 5000).map((sha256) => ({ versionId: v.id, projectId: r.projectId, sha256 })),
          );
      await tx.insert(auditLog).values({
        projectId: r.projectId,
        actorId: null,
        action: "version.commit",
        targetType: "version",
        targetId: v.id,
        detail: { number: n, size, run: r.id, base: base.number, newMedia: fresh.length },
      });
      // Events first, then the state: a stream that sees the final state has every event.
      await hooks.insertIn(tx, [...notes, { event: "hub-result", version: n }]);
      await hooks.finishIn(tx, final, v.id);
    });
  } catch (e) {
    // Nothing was committed: the snapshot object has no version and must not linger.
    await deleteObject(snapshotKey).catch(() => {});
    if (!(e instanceof LeaseLost)) throw e;
  }
}
