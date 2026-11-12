import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect } from "@porter-hub/db";
import { claim, type Db, reclaimLost } from "./claim";
import { execute, type Settings } from "./execute";
import { SWEEP_MS, sweepUploads } from "./sweep";

export function settings(env = process.env): Settings & { concurrency: number } {
  return {
    concurrency: Math.max(1, Number(env.WORKER_CONCURRENCY) || 1),
    bin: env.PORTER_BIN || "porter",
    scratchDir: env.WORKER_SCRATCH_DIR || join(tmpdir(), "porter-worker"),
  };
}

/** Claims and runs queued runs up to `concurrency` at once; reclaims lost leases and sweeps uploads every 60 s. */
export async function loop(db: Db, s: ReturnType<typeof settings>, signal: AbortSignal) {
  const active = new Set<Promise<void>>();
  let nextSweep = 0;
  while (!signal.aborted) {
    try {
      if (Date.now() >= nextSweep) {
        nextSweep = Date.now() + SWEEP_MS;
        await reclaimLost(db);
        await sweepUploads(db);
      }
      while (active.size < s.concurrency) {
        const r = await claim(db);
        if (!r) break;
        const p = execute(db, r, s).finally(() => active.delete(p));
        active.add(p);
      }
    } catch (e) {
      console.error("worker loop:", e);
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  await Promise.all(active);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const s = settings();
  const { db, client } = connect();
  const ctl = new AbortController();
  // Stop claiming on SIGTERM/SIGINT; runs in flight finish (porter gets its own SIGINT from the process group).
  for (const sig of ["SIGTERM", "SIGINT"] as const) process.once(sig, () => ctl.abort());
  console.log(`porter worker: concurrency ${s.concurrency}, porter ${s.bin}, scratch ${s.scratchDir}`);
  loop(db, s, ctl.signal).finally(() => client.end());
}
