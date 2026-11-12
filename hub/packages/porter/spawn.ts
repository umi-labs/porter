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
    if (this.buf.length > this.max) {
      let start = this.buf.length - this.max;
      // Don't start mid-character: skip UTF-8 continuation bytes (10xxxxxx).
      while (start < this.buf.length && (this.buf[start] & 0xc0) === 0x80) start++;
      this.buf = this.buf.subarray(start);
    }
  }
  toString() {
    return this.buf.toString("utf8");
  }
}

export type Spawned = {
  /** Porter's pid, which is also its process group id (it runs detached in its own group). */
  pid: number | undefined;
  /** Resolves when porter has exited and every stdout line has been handed to `onEvent`. Never rejects. */
  done: Promise<Exit>;
  /** SIGINT to porter's process group now, SIGKILL to the group if porter is still running after `killAfterMs`. */
  interrupt(killAfterMs?: number): void;
  /** SIGKILL to the whole process group (porter and anything it left behind). */
  kill(): void;
};

/** Signals a whole process group (porter runs as its own group leader). */
export function signalGroup(pid: number | undefined, sig: NodeJS.Signals) {
  if (!pid) return;
  try {
    process.kill(-pid, sig);
  } catch {} // ESRCH: the group is already gone
}

/**
 * Runs `porter --json --project <dir> ...args` with exactly `env` (no inherited environment). Every stdout line
 * becomes one event, in order; stderr goes to `tail`. Porter runs detached in its own process group, so a
 * terminal Ctrl-C aimed at the worker doesn't reach it; the worker signals the group itself.
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
    detached: true,
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
    pid: child.pid,
    done,
    interrupt(killAfterMs = KILL_AFTER_MS) {
      if (child.exitCode !== null || child.signalCode !== null || killer) return;
      signalGroup(child.pid, "SIGINT");
      killer = setTimeout(() => signalGroup(child.pid, "SIGKILL"), killAfterMs);
    },
    kill() {
      signalGroup(child.pid, "SIGKILL");
    },
  };
}
