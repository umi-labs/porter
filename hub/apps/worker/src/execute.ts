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
  type StageName,
  type StoredEvent,
  spawnPorter,
  stripWpDb,
  Tail,
} from "@porter-hub/porter";
import { getStream, keys, putFile } from "@porter-hub/storage/s3";
import { extractTarZst, packTarZst } from "@porter-hub/storage/tar";
import { and, eq, inArray, sql } from "drizzle-orm";
import { type Db, HEARTBEAT_MS, heartbeat, notify, type Run } from "./claim";

export type Settings = {
  bin: string;
  scratchDir: string;
  killAfterMs?: number;
  heartbeatMs?: number;
  cancelPollMs?: number;
};

const HASH = /^[0-9a-f]{64}$/;
const MEDIA_CONCURRENCY = 8;
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

/** Ordered, batched run_event writer: rows go in seq order, each batch followed by a NOTIFY. */
function eventWriter(db: Db, runId: string) {
  let seq = 0;
  let buf: { runId: string; seq: number; event: StoredEvent }[] = [];
  let chain: Promise<void> = Promise.resolve();
  let timer: NodeJS.Timeout | undefined;
  const flush = () => {
    clearTimeout(timer);
    timer = undefined;
    const rows = buf;
    buf = [];
    if (rows.length)
      chain = chain
        .then(async () => {
          await db.insert(runEvent).values(rows);
          await notify(db, runId, String(rows[rows.length - 1].seq));
        })
        .catch((e) => console.error(`run ${runId}: storing events failed:`, e));
    return chain;
  };
  return {
    async init() {
      const [r] = await db
        .select({ n: sql<number>`coalesce(max(${runEvent.seq}), 0)` })
        .from(runEvent)
        .where(eq(runEvent.runId, runId));
      seq = Number(r.n);
    },
    push(event: StoredEvent) {
      buf.push({ runId, seq: ++seq, event });
      if (buf.length >= 200) flush();
      else timer ??= setTimeout(flush, 100);
    },
    flush,
  };
}

/** Exit of the last porter process -> the run's final state. `sawIssues`: an earlier stage exited 2. */
export function outcome(x: Exit, cancelRequested: boolean, sawIssues = false) {
  type O = { state: "finished" | "failed" | "cancelled"; exitCode: number | null; error: string | null };
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
 * Runs one claimed run to completion: snapshot + media into a scratch dir, wp-db stripped, secrets as env only,
 * porter spawned per stage, events stored in order, then the result packed and committed as a new version.
 * The scratch dir is always deleted.
 */
export async function execute(db: Db, r: Run, s: Settings) {
  const scratch = join(s.scratchDir, r.id);
  const dir = join(scratch, "project");
  const events = eventWriter(db, r.id);
  const tail = new Tail();
  const env = {
    PATH: process.env.PATH ?? "/usr/bin:/bin",
    HOME: scratch,
    LANG: process.env.LANG || "C.UTF-8",
  };
  let lost = false;
  let cancelRequested = false;
  let current: ReturnType<typeof spawnPorter> | undefined;
  const timers: NodeJS.Timeout[] = [];
  const finish = async (
    state: "finished" | "failed" | "cancelled",
    exitCode: number | null,
    error: string | null,
  ) => {
    await events.flush();
    const done = await db
      .update(run)
      .set({ state, exitCode, error, finishedAt: new Date() })
      .where(and(eq(run.id, r.id), eq(run.state, "running")))
      .returning({ id: run.id });
    if (done.length)
      await db.insert(auditLog).values({
        projectId: r.projectId,
        actorId: null,
        action: "run.finish",
        targetType: "run",
        targetId: r.id,
        detail: { state, exit_code: exitCode, error },
      });
    await notify(db, r.id, "state");
  };

  try {
    await events.init();
    await rm(scratch, { recursive: true, force: true });
    await mkdir(dir, { recursive: true });

    timers.push(
      setInterval(async () => {
        if (!(await heartbeat(db, r.id).catch(() => true))) {
          lost = true;
          current?.kill();
        }
      }, s.heartbeatMs ?? HEARTBEAT_MS),
      setInterval(async () => {
        if (cancelRequested) return;
        const [x] = await db
          .select({ c: run.cancelRequested })
          .from(run)
          .where(eq(run.id, r.id))
          .catch(() => []);
        if (x?.c) {
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
    const runEnv: Record<string, string> = { ...env };
    for (const x of secrets) runEnv[x.name] = decryptSecret(r.projectId, x.name, x);

    // ---- run porter, one invocation per stage (or one `porter run` for all) ----
    const stages = r.stages[0] === "all" ? "all" : (r.stages as StageName[]);
    let last: Exit | undefined;
    let sawIssues = false;
    for (const args of commandsFor(stages, r.apply)) {
      if (cancelRequested || lost) break;
      current = spawnPorter({ bin: s.bin, dir, args, env: runEnv, onEvent: events.push, tail });
      last = await current.done;
      current = undefined;
      if (last.code === 2) sawIssues = true;
      if (last.error || (last.code !== 0 && last.code !== 2)) break;
    }
    if (lost) return; // reclaimed as "worker lost": the run is already failed; keep nothing
    if (!last) return await finish("cancelled", null, null); // cancelled before porter started
    const o = outcome(last, cancelRequested, sawIssues);
    if (o.state === "failed" && tail.toString()) events.push({ event: "hub-stderr", text: tail.toString() });
    if (last.error) return await finish(o.state, o.exitCode, o.error); // could not even start

    // ---- result: pack, upload, commit as latest+1 ----
    await commitResult(db, r, base, dir, scratch, originalToml, s.bin, env, events.push);
    await finish(o.state, o.exitCode, o.error);
  } catch (e) {
    console.error(`run ${r.id}:`, e);
    if (!lost) await finish("failed", null, `hub error: ${(e as Error).message}`).catch(() => {});
  } finally {
    for (const t of timers) clearInterval(t);
    current?.kill();
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
  note: (e: StoredEvent) => void,
) {
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
        note({
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

  const number = await db.transaction(async (tx) => {
    await tx.select({ id: project.id }).from(project).where(eq(project.id, r.projectId)).for("update");
    const [l] = await tx
      .select({ n: sql<number>`coalesce(max(${projectVersion.number}), 0)` })
      .from(projectVersion)
      .where(eq(projectVersion.projectId, r.projectId));
    const latest = Number(l.n);
    const n = latest + 1;
    if (latest > base.number)
      note({
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
    await tx.update(run).set({ resultVersionId: v.id }).where(eq(run.id, r.id));
    await tx.insert(auditLog).values({
      projectId: r.projectId,
      actorId: null,
      action: "version.commit",
      targetType: "version",
      targetId: v.id,
      detail: { number: n, size, run: r.id, base: base.number, newMedia: fresh.length },
    });
    return n;
  });
  note({ event: "hub-result", version: number });
}
