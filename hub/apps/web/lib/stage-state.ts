import { STATES, type State } from "../components/ui/chip";

// A version pushed by a current porter carries porter's own stage views (`porter --json status`'s `stage_views`,
// from the engine's `stage_view`), so the hub shows what the desktop and TUI show: `storedViews`. Older versions
// fall back to `stageViews`, a port for what the snapshot's runs table can show: "running" never (a snapshot is a
// still), "failed" (exit 1) never (the engine leaves no row for a stage that could not run), stale from upstream
// re-runs, and stale for every other run too: with no recorded settings fingerprint it "ran before settings were
// recorded"; with one, the hub can't re-check it (that needs porter's hashing), and the engine's rule is that what
// can't be checked counts as stale.

export const PIPELINE = ["analyse", "extract", "normalise", "schema", "load", "verify"] as const;
export type Summary = {
  done: number;
  partial: number;
  skipped: number;
  failed: number;
  errors: number;
  warnings: number;
  infos: number;
  interrupted: boolean;
};
export type RunRow = {
  id: string;
  stage: string;
  startedAt: string;
  endedAt: string | null;
  summary: Summary | null;
  /** null: an older snapshot without the dry_run column, so a load can't be told apart from a dry run. */
  dryRun: boolean | null;
  /** The run's settings fingerprint; null or undefined (no column): none recorded, so stale. */
  configFp?: string | null;
};
export type StageView = {
  stage: (typeof PIPELINE)[number];
  /**
   * null: can't be derived (a load that finished without errors, in a snapshot that doesn't record dry runs), or
   * porter stored a state this hub doesn't know (`unknown` holds it).
   */
  state: State | null;
  /** A stored state from a newer porter that this hub doesn't know. */
  unknown?: string;
  stale: string | null;
  run: RunRow | null;
};

/** Same rules as the engine's `finished_state`: errors before dry run, dry run before warnings/clean. */
export function runState(r: RunRow): State | null {
  const s = r.summary;
  if (!s || s.interrupted) return "interrupted"; // no summary: started and never ended
  if (s.failed > 0 || s.errors > 0) return "errors";
  if (r.dryRun === null && r.stage === "load") return null;
  if (r.dryRun) return "dry-run";
  return s.warnings > 0 ? "warnings" : "clean";
}

/** One view per pipeline stage from the latest run row of each stage. */
export function stageViews(last: RunRow[]): StageView[] {
  const of = (s: string) => last.find((r) => r.stage === s) ?? null;
  return PIPELINE.map((stage, pos) => {
    const run = of(stage);
    if (!run) {
      // Verify after a dry-run load: nothing in the destination to check.
      const skipped = stage === "verify" && of("load")?.dryRun === true;
      return { stage, state: skipped ? "skipped" : "not-run", stale: null, run: null };
    }
    let stale: string | null = null;
    for (const up of PIPELINE.slice(0, pos).reverse()) {
      const u = of(up);
      // Run ids are UUIDv7, so they order by start. A dry-run load wrote nothing, so verify still stands.
      if (u && u.id > run.id && !(up === "load" && u.dryRun)) {
        stale = `${up} re-ran since`;
        break;
      }
    }
    // Same as the engine: a run from before porter recorded settings can't be shown to still hold.
    if (!stale)
      stale = run.configFp
        ? "settings not checked on the hub; push with a current porter"
        : "ran before settings were recorded";
    return { stage, state: runState(run), stale, run };
  });
}

/**
 * porter's own views stored with the version (`summary.stage_views`), each with its run row for the counts.
 * null when the version doesn't carry them (pushed by an older porter): use `stageViews`. A state this hub doesn't
 * know (a newer porter's) keeps the rest: that stage reads "unknown (newer porter)".
 */
export function storedViews(summary: unknown, last: RunRow[]): StageView[] | null {
  const sv = (summary as { stage_views?: unknown } | null)?.stage_views;
  if (!sv || typeof sv !== "object") return null;
  const out: StageView[] = [];
  for (const stage of PIPELINE) {
    const v = (sv as Record<string, { state?: unknown; stale?: unknown } | undefined>)[stage];
    const known = !!v && STATES.includes(v.state as State);
    out.push({
      stage,
      state: known ? (v.state as State) : null,
      ...(known ? {} : { unknown: typeof v?.state === "string" ? v.state : "missing" }),
      stale: typeof v?.stale === "string" ? v.stale : null,
      run: last.find((r) => r.stage === stage) ?? null,
    });
  }
  return out;
}

export function parseSummary(json: unknown): Summary | null {
  if (typeof json !== "string") return null;
  try {
    const s = JSON.parse(json) as Partial<Summary>;
    const n = (x: unknown) => (typeof x === "number" ? x : 0);
    return {
      done: n(s.done),
      partial: n(s.partial),
      skipped: n(s.skipped),
      failed: n(s.failed),
      errors: n(s.errors),
      warnings: n(s.warnings),
      infos: n(s.infos),
      interrupted: s.interrupted === true,
    };
  } catch {
    return null;
  }
}
