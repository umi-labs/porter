import type { State } from "@/components/porter/chip";
import { STATES } from "@/components/porter/chip";
import { fmtNum } from "./format";
import { PIPELINE } from "./stage-state";

type Stored = { state?: unknown; stale?: unknown };

/**
 * One status pill for a project in the projects list, from its latest version's stored `porter status --json`
 * (no snapshot needed). Same rules as the overview: green only once a real load wrote to the site. null when the
 * version carries no stage views (an older porter, or nothing pushed): the pill stays blank rather than guessed.
 */
export function projectHeadline(summary: unknown): { state: State; label: string } | null {
  const s = summary as {
    stage_views?: unknown;
    open_issues?: { error?: unknown };
    ever_written?: unknown;
  } | null;
  const sv = s?.stage_views as Record<string, Stored | undefined> | undefined;
  if (!sv || typeof sv !== "object") return null;
  const view = (stage: string) => {
    const v = sv[stage];
    return {
      state: STATES.includes(v?.state as State) ? (v?.state as State) : null,
      stale: typeof v?.stale === "string" ? v.stale : null,
    };
  };
  const errors = typeof s?.open_issues?.error === "number" ? s.open_issues.error : 0;
  if (errors > 0) return { state: "errors", label: `${fmtNum(errors)} error${errors === 1 ? "" : "s"}` };
  if (PIPELINE.some((st) => view(st).state === "interrupted"))
    return { state: "interrupted", label: "Interrupted" };
  const written = s?.ever_written === true;
  const load = view("load");
  if (load.state === "dry-run" && !written) return { state: "dry-run", label: "Dry run only" };
  if (written) {
    const verify = view("verify");
    const checked = (verify.state === "clean" || verify.state === "warnings") && !verify.stale;
    return { state: "clean", label: checked ? "Loaded · verified" : "Loaded · not verified" };
  }
  const reached = [...PIPELINE].reverse().find((st) => {
    const v = view(st).state;
    return v !== null && v !== "not-run" && v !== "skipped";
  });
  if (!reached) return { state: "not-run", label: "Not run" };
  const v = view(reached);
  return v.stale
    ? { state: "not-run", label: `Up to ${reached} (stale)` }
    : { state: v.state as State, label: `Up to ${reached}` };
}
