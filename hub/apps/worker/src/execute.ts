import { execFile, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { basename, join, relative } from "node:path";
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
  upload,
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
  secretNameProblem,
  signalGroup,
  spawnPorter,
  stripWpDb,
  Tail,
  uploadRefs,
} from "@porter-hub/porter";
import { deleteObject, getStream, keys, putFile } from "@porter-hub/storage/s3";
import { extractTarZst, packTarZst } from "@porter-hub/storage/tar";
import { and, eq, inArray, ne, sql } from "drizzle-orm";
import { type Db, HEARTBEAT_MS, heartbeat, LEASE_MS, notify, type Run } from "./claim";

export type Settings = {
  bin: string;
  scratchDir: string;
  killAfterMs?: number;
  heartbeatMs?: number;
  cancelPollMs?: number;
  leaseMs?: number;
  /** Injected in tests (e.g. a heartbeat that never returns). */
  heartbeat?: typeof heartbeat;
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

let ownPgid: number | null | undefined;
/** This process's group id (never signalled by the stale-pid sweep). */
const myPgid = () => {
  if (ownPgid === undefined) {
    try {
      ownPgid = Number(
        execFileSync("ps", ["-o", "pgid=", "-p", String(process.pid)], { encoding: "utf8" }).trim(),
      );
    } catch {
      ownPgid = null;
    }
  }
  return ownPgid;
};
/** The command name of `pid` (`ps -o comm=`, macOS and Linux), or null if there's no such process. */
const commOf = (pid: number) => {
  try {
    return execFileSync("ps", ["-o", "comm=", "-p", String(pid)], { encoding: "utf8" }).trim() || null;
  } catch {
    return null;
  }
};

/**
 * Deletes scratch dirs (`<scratchDir>/<runId>`) whose run exists in this database and isn't running,
 * SIGKILLing the porter process group still recorded in their porter.pid (left over from a crashed worker).
 * Ids this database doesn't know are left alone: they may belong to a worker on another database sharing the
 * dir. A stale pid is only signalled if it isn't us or our group and the process is still `bin`
 * (a recycled pid belongs to something else). `keep`: runs this process is executing.
 */
export async function cleanScratch(
  db: Db,
  scratchDir: string,
  keep: Iterable<string> = active.keys(),
  bin = "porter",
) {
  const mine = new Set(keep);
  const ids = (await readdir(scratchDir).catch(() => [] as string[])).filter(
    (n) => UUID.test(n) && !mine.has(n),
  );
  if (!ids.length) return [];
  const stale = (
    await db
      .select({ id: run.id })
      .from(run)
      .where(and(inArray(run.id, ids), ne(run.state, "running")))
  ).map((r) => r.id);
  const removed: string[] = [];
  for (const id of stale) {
    const pid = Number(await readFile(join(scratchDir, id, "porter.pid"), "utf8").catch(() => ""));
    if (
      Number.isInteger(pid) &&
      pid > 1 &&
      pid !== process.pid &&
      pid !== myPgid() &&
      commOf(pid) === basename(bin)
    )
      signalGroup(pid, "SIGKILL");
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
  let beating = false;
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
        // Measured from the last successful heartbeat on every tick: a heartbeat that hangs (a partition)
        // neither resolves nor rejects, and by now someone may have reclaimed the run.
        if (Date.now() - lastBeat > leaseMs) return markLost();
        if (beating) return; // one in flight at a time
        beating = true;
        try {
          if (await (s.heartbeat ?? heartbeat)(db, r.id, r.workerId)) lastBeat = Date.now();
          else markLost(); // reclaimed, or claimed by someone else
        } catch {
          // A failed heartbeat: the age check above decides.
        } finally {
          beating = false;
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

    // Hub uploads a connector reads (`uploads/<id>/<filename>`, set up on the hub): fetched in, never kept.
    for (const ref of uploadRefs(originalToml.toString("utf8"))) {
      const [u] = await db
        .select({ objectKey: upload.objectKey, state: upload.state })
        .from(upload)
        .where(and(eq(upload.id, ref.id), eq(upload.projectId, r.projectId)));
      if (u?.state !== "ready")
        return await finish("failed", null, `${ref.path} is not a finished upload on this project any more`);
      await mkdir(join(dir, "uploads", ref.id), { recursive: true });
      await pipeline(await getStream(u.objectKey), createWriteStream(join(dir, ref.path)));
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
    for (const x of secrets) {
      // Re-checked here: a row saved before a name became reserved must not reach porter.
      if (secretNameProblem(x.name)) {
        events.push({
          event: "hub-note",
          kind: "secret-skipped",
          message: `run secret ${x.name} has a reserved name and was not passed to porter; rename it`,
        });
        continue;
      }
      values[x.name] = decryptSecret(r.projectId, x.name, x);
    }
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
      current = spawnPorter({ bin: s.bin, dir, args, env: runEnv, onEvent: events.push, tail, redact });
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
      events.push({ event: "hub-stderr", text: tail.toString(redact) });
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
    console.error(`run ${r.id}:`, redact(String((e as Error)?.stack ?? e)));
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
    if (e.isFile() && !/^(media|uploads)\//.test(relative(dir, join(e.parentPath, e.name))))
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

  // The snapshot keeps the original porter.toml (never the stripped copy) and leaves media/ and uploads/ out.
  const out = join(scratch, "result.tar.zst");
  await packTarZst(dir, out, (rel) => rel === "media" || rel === "uploads", { "porter.toml": originalToml });
  const snapshotSha256 = await sha256File(out);
  const { size } = await stat(out);
  const snapshotKey = keys.snapshot(r.projectId, r.id);
  await putFile(snapshotKey, out, snapshotSha256, size);

  const quiet = { env, timeout: 60_000, maxBuffer: 16 * 1024 * 1024 };
  const porterVersion = await execFileP(bin, ["--version"], quiet).then(
    (x) => x.stdout.trim().replace(/^porter\s+/, ""),
    () => "unknown",
  );
  // The stored stage views (`stage_views`) are judged against the porter.toml the snapshot carries, so they match
  // what porter shows after a pull (a stage run without wp-db reads stale there too).
  await writeFile(join(dir, "porter.toml"), originalToml, { mode: 0o600 });
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
      // The worker's 10 s statement_timeout would fail this commit while retention's init sweep holds the project
      // lock; waiting a while is cheaper than losing the run's result.
      await tx.execute(sql`set local statement_timeout = '2min'`);
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
    // Nothing was committed: the snapshot object has no version and must not linger. Unless the commit did land
    // and only its acknowledgement was lost (a dropped connection): then the version needs it.
    const [kept] = await db
      .select({ id: projectVersion.id })
      .from(projectVersion)
      .where(eq(projectVersion.snapshotKey, snapshotKey))
      .catch(() => [{ id: "unknown" }]);
    if (!kept) await deleteObject(snapshotKey).catch(() => {});
    if (!(e instanceof LeaseLost)) throw e;
  }
}
