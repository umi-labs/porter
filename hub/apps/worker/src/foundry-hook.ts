import { project, projectVersion, run } from "@porter-hub/db";
import { eq } from "drizzle-orm";
import type { Db } from "./claim";

const TIMEOUT_MS = 10_000;

/** What Foundry hub receives when a run on a linked project ends (its `/api/webhooks/porter`). */
export async function foundryPayload(db: Db, runId: string) {
  const [row] = await db
    .select({ r: run, ref: project.foundryRef, slug: project.slug })
    .from(run)
    .innerJoin(project, eq(project.id, run.projectId))
    .where(eq(run.id, runId));
  if (!row?.ref || !["finished", "failed", "cancelled"].includes(row.r.state)) return null;
  const [result] = row.r.resultVersionId
    ? await db
        .select({ n: projectVersion.number })
        .from(projectVersion)
        .where(eq(projectVersion.id, row.r.resultVersionId))
    : [];
  const origin = (process.env.BETTER_AUTH_URL ?? "").replace(/\/$/, "");
  return {
    event: "run.finished",
    ref: row.ref,
    porter_url: `${origin}/p/${row.slug}/runs/${row.r.id}`,
    run: {
      id: row.r.id,
      state: row.r.state,
      stages: row.r.stages[0] === "all" ? "all" : row.r.stages,
      apply: row.r.apply,
      exit_code: row.r.exitCode,
      error: row.r.error,
      result_version: result?.n ?? null,
    },
  };
}

/**
 * Tells Foundry hub a run on a linked project ended: POSTs to FOUNDRY_WEBHOOK_URL with the bearer
 * FOUNDRY_WEBHOOK_TOKEN. Best effort (one try, 10 s); Foundry's status call is the source of truth.
 */
export async function notifyFoundry(db: Db, runId: string, fetchImpl: typeof fetch = fetch) {
  const url = process.env.FOUNDRY_WEBHOOK_URL;
  const token = process.env.FOUNDRY_WEBHOOK_TOKEN;
  if (!url || !token) return;
  try {
    const body = await foundryPayload(db, runId);
    if (!body) return;
    const res = await fetchImpl(url, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(TIMEOUT_MS),
      redirect: "error",
    });
    if (!res.ok) console.error(`foundry webhook for run ${runId}: HTTP ${res.status}`);
  } catch (e) {
    console.error(`foundry webhook for run ${runId}:`, (e as Error).message);
  }
}
