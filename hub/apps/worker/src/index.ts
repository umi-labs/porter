import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect } from "@porter-hub/db";
import { claim, type Db, reclaimLost } from "./claim";
import { active, cleanScratch, execute, interruptAll, type Settings } from "./execute";
import { SWEEP_MS, sweepUploads } from "./sweep";

export function settings(env = process.env): Settings & { concurrency: number } {
  return {
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
        await cleanScratch(db, s.scratchDir, runs.keys());
        await sweepUploads(db);
      }
      while (runs.size < s.concurrency) {
        const r = await claim(db);
        if (!r) break;
        runs.set(
          r.id,
          execute(db, r, s).finally(() => runs.delete(r.id)),
        );
      }
    } catch (e) {
      console.error("worker loop:", e);
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  await Promise.all(runs.values());
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const s = settings();
  const { db, client } = connect();
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
  loop(db, s, ctl.signal).finally(() => client.end());
}
