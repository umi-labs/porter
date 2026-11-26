import { writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect } from "@porter-hub/db";
import { secretKey } from "@porter-hub/porter";
import { claim, type Db, reclaimLost } from "./claim";
import { active, cleanScratch, execute, interruptAll, type Settings } from "./execute";
import { notifyFoundry } from "./foundry-hook";
import { RETENTION_MS, sweepRetention } from "./retention";
import { SWEEP_MS, sweepUploads } from "./sweep";

/** Touched every loop turn; the container health check fails when it goes stale. */
export const ALIVE_FILE = join(tmpdir(), "porter-worker.alive");

export function settings(env = process.env): Settings & { concurrency: number; cacheDir: string } {
  return {
    // Same default as apps/web; in Docker both mount one volume here so retention can clear extracted snapshots.
    cacheDir: env.HUB_CACHE_DIR || join(tmpdir(), "porter-hub-cache"),
    concurrency: Math.max(1, Number(env.WORKER_CONCURRENCY) || 1),
    bin: env.PORTER_BIN || "porter",
    scratchDir: env.WORKER_SCRATCH_DIR || join(tmpdir(), "porter-worker"),
  };
}

/**
 * Claims and runs queued runs up to `concurrency` at once. Every 60 s (and at startup): reclaims lost leases,
 * deletes scratch dirs of runs that aren't running (killing their stale porter groups) and sweeps uploads.
 */
export async function loop(db: Db, s: ReturnType<typeof settings>, signal: AbortSignal) {
  const runs = new Map<string, Promise<void>>();
  let nextSweep = 0;
  while (!signal.aborted) {
    try {
      if (Date.now() >= nextSweep) {
        nextSweep = Date.now() + SWEEP_MS;
        await reclaimLost(db);
        await cleanScratch(db, s.scratchDir, runs.keys(), s.bin);
        await sweepUploads(db);
      }
      while (runs.size < s.concurrency) {
        const r = await claim(db);
        if (!r) break;
        runs.set(
          r.id,
          execute(db, r, s)
            .then(() => notifyFoundry(db, r.id))
            .finally(() => runs.delete(r.id)),
        );
      }
    } catch (e) {
      console.error("worker loop:", e);
    }
    await writeFile(ALIVE_FILE, "").catch(() => {});
    await new Promise((r) => setTimeout(r, 1000));
  }
  await Promise.all(runs.values());
}

/** Retention once at startup and then daily, beside the run loop so a long sweep never delays claiming. */
async function retentionLoop(db: Db, cacheDir: string, signal: AbortSignal) {
  while (!signal.aborted) {
    await sweepRetention(db, cacheDir).catch((e) => console.error("retention:", e));
    await new Promise((r) => {
      const t = setTimeout(r, RETENTION_MS);
      signal.addEventListener("abort", () => r(clearTimeout(t)), { once: true });
    });
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (process.env.NODE_ENV === "production") {
    try {
      secretKey(); // run secrets can't be decrypted without it: refuse to start rather than fail every run
    } catch (e) {
      console.error((e as Error).message);
      process.exit(1);
    }
  }
  const s = settings();
  // A query that hangs server-side (locks, a stalled pooler) errors after 10 s instead of stalling the loop.
  // A network partition doesn't reach the server at all; the heartbeat's lease-age check covers that.
  const { db, client } = connect(undefined, { connection: { statement_timeout: 10_000 } });
  const ctl = new AbortController();
  // SIGTERM/SIGINT: stop claiming and SIGINT every porter process group (they run detached, so a terminal
  // Ctrl-C doesn't reach them on its own). Their runs end "interrupted" with the result kept. A second signal
  // SIGKILLs them.
  let stopping = false;
  for (const sig of ["SIGTERM", "SIGINT"] as const)
    process.on(sig, () => {
      if (stopping) for (const p of active.values()) p.kill();
      stopping = true;
      ctl.abort();
      interruptAll();
    });
  console.log(`porter worker: concurrency ${s.concurrency}, porter ${s.bin}, scratch ${s.scratchDir}`);
  Promise.all([loop(db, s, ctl.signal), retentionLoop(db, s.cacheDir, ctl.signal)]).finally(() =>
    client.end(),
  );
}
