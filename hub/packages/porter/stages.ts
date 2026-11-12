// No dependencies: the browser imports this (run form).
/** Stage names a run request may list, in pipeline order. */
export const STAGES = ["analyse", "extract", "normalise", "schema-check", "load", "verify"] as const;
export type StageName = (typeof STAGES)[number];

/**
 * The porter invocations for a run, in order. "all" is one `porter run`; a list runs each stage in pipeline
 * order whatever order it was given in. Load only applies with `apply`; otherwise it is a dry run.
 */
export function commandsFor(stages: readonly StageName[] | "all", apply: boolean): string[][] {
  const ap = apply ? ["--apply"] : [];
  if (stages === "all") return [["run", ...ap]];
  return STAGES.filter((s) => stages.includes(s)).map((s) =>
    s === "schema-check" ? ["schema", "check"] : s === "load" ? ["load", ...ap] : [s],
  );
}
