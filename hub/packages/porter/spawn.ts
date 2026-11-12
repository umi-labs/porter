import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { parseLine, type StoredEvent } from "./events";

export const STDERR_TAIL_BYTES = 64 * 1024;
/** After SIGINT, porter gets this long to exit cleanly (exit 3, resumable) before SIGKILL. */
export const KILL_AFTER_MS = 30_000;

export type Exit = { code: number | null; signal: NodeJS.Signals | null; error?: string };

/** Keeps the last `max` bytes written to it. */
export class Tail {
  private buf = Buffer.alloc(0);
  constructor(private max = STDERR_TAIL_BYTES) {}
  push(chunk: Buffer) {
    this.buf = Buffer.concat([this.buf, chunk]);
    if (this.buf.length > this.max) this.buf = this.buf.subarray(this.buf.length - this.max);
  }
  toString() {
    return this.buf.toString("utf8");
  }
}

export type Spawned = {
  /** Resolves when porter has exited and every stdout line has been handed to `onEvent`. Never rejects. */
  done: Promise<Exit>;
  /** SIGINT now, SIGKILL if still running after `killAfterMs`. */
  interrupt(killAfterMs?: number): void;
  kill(): void;
};

/**
 * Runs `porter --json --project <dir> ...args` with exactly `env` (no inherited environment). Every stdout line
 * becomes one event, in order; stderr goes to `tail`.
 */
export function spawnPorter(opts: {
  bin: string;
  dir: string;
  args: string[];
  env: Record<string, string>;
  onEvent: (e: StoredEvent) => void;
  tail: Tail;
}): Spawned {
  const child = spawn(opts.bin, ["--json", "--project", opts.dir, ...opts.args], {
    env: opts.env,
    cwd: opts.dir,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.on("data", (c: Buffer) => opts.tail.push(c));
  const lines = createInterface({ input: child.stdout, crlfDelay: Number.POSITIVE_INFINITY });
  lines.on("line", (l) => {
    if (l.trim()) opts.onEvent(parseLine(l));
  });
  const stdoutClosed = new Promise<void>((r) => lines.once("close", () => r()));
  let killer: NodeJS.Timeout | undefined;
  const done = new Promise<Exit>((res) => {
    child.once("error", (e) => res({ code: null, signal: null, error: e.message }));
    child.once("close", (code, signal) => res({ code, signal }));
  }).then(async (x) => {
    clearTimeout(killer);
    if (!x.error) await stdoutClosed;
    return x;
  });
  return {
    done,
    interrupt(killAfterMs = KILL_AFTER_MS) {
      if (child.exitCode !== null || child.signalCode !== null || killer) return;
      child.kill("SIGINT");
      killer = setTimeout(() => child.kill("SIGKILL"), killAfterMs);
    },
    kill() {
      child.kill("SIGKILL");
    },
  };
}
