import { z } from "zod";

/** Porter's `--json` NDJSON contract, v1 (crates/porter-engine/src/events.rs, README "Automation: --json"). */
const stage = z.enum(["analyse", "extract", "normalise", "schema", "load", "verify"]);
const summary = z.object({
  done: z.number(),
  partial: z.number(),
  skipped: z.number(),
  failed: z.number(),
  errors: z.number(),
  warnings: z.number(),
  infos: z.number(),
  interrupted: z.boolean(),
});
const issue = z.looseObject({
  id: z.string(),
  kind: z.string(),
  severity: z.enum(["error", "warning", "info"]),
  item: z.string().nullable(),
  stage: z.string(),
  message: z.string(),
});
const v1 = { v: z.literal(1) };

export const knownEvents = {
  "stage-started": z.looseObject({ ...v1, event: z.literal("stage-started"), stage, run: z.string() }),
  progress: z.looseObject({
    ...v1,
    event: z.literal("progress"),
    stage,
    done: z.number(),
    total: z.number().nullable(),
    label: z.string(),
  }),
  "item-result": z.looseObject({
    ...v1,
    event: z.literal("item-result"),
    stage,
    item: z.string(),
    kind: z.string(),
    status: z.union([z.literal("done"), z.record(z.string(), z.unknown())]),
  }),
  "issue-opened": z.looseObject({ ...v1, event: z.literal("issue-opened"), issue }),
  "stage-finished": z.looseObject({
    ...v1,
    event: z.literal("stage-finished"),
    stage,
    run: z.string(),
    summary,
  }),
} as const;
export type Progress = z.infer<(typeof knownEvents)["progress"]>;
export type StageFinished = z.infer<(typeof knownEvents)["stage-finished"]>;

/** Every line needs at least this; other event names (status, report, future ones) are stored as they are. */
const envelope = z.looseObject({ v: z.number(), event: z.string().min(1) });

export const MALFORMED_MAX = 4096;
export type StoredEvent = { event: string; [k: string]: unknown };

/**
 * One stdout line -> the JSON stored as a run_event. A line that isn't a JSON object with `v` and `event`, or a
 * known v1 event whose fields don't match the contract, becomes `{event: "hub-malformed", line}` (first 4 KB).
 */
export function parseLine(line: string): StoredEvent {
  let raw: unknown;
  try {
    raw = JSON.parse(line);
  } catch {
    raw = undefined;
  }
  const env = envelope.safeParse(raw);
  const known = env.success ? knownEvents[env.data.event as keyof typeof knownEvents] : undefined;
  if (!env.success || (known && !known.safeParse(raw).success))
    return { event: "hub-malformed", line: line.slice(0, MALFORMED_MAX) };
  return env.data as StoredEvent;
}
