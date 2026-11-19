import type { State } from "../components/ui/chip";

// No server imports: the run page's client component uses this too.

type RunLike = { state: string; exit_code: number | null; apply: boolean; stages: readonly string[] | "all" };
type Sum = { warnings: number; interrupted?: boolean };

/** Whether the run includes load without --apply: then load was a dry run and nothing was written. */
export const isDryRun = (r: RunLike) => !r.apply && (r.stages === "all" || r.stages.includes("load"));

/**
 * A hub run's chip. `summaries` (from its stage-finished events) are needed to call exit 0 clean: exit 0 still
 * allows warnings. Without them a finished run gets no state (null), only its words.
 */
export function runLook(r: RunLike, summaries?: Sum[]): { state: State | null; label: string } {
  switch (r.state) {
    case "queued":
      return { state: "not-run", label: "Queued" };
    case "running":
      return { state: "running", label: "Running" };
    case "cancelled":
      return { state: "interrupted", label: "Cancelled" };
    case "failed":
      return { state: "failed", label: "Failed" };
  }
  if (r.exit_code === 2) return { state: "errors", label: "Finished with issues" };
  if (r.exit_code === 3) return { state: "interrupted", label: "Interrupted" };
  if (r.exit_code !== 0) return { state: "failed", label: `Failed (exit ${r.exit_code ?? "?"})` };
  if (isDryRun(r)) return { state: "dry-run", label: "Finished · dry run" };
  if (!summaries?.length) return { state: null, label: "Finished" };
  return summaries.some((s) => s.warnings > 0)
    ? { state: "warnings", label: "Finished · warnings" }
    : { state: "clean", label: "Finished · clean" };
}
