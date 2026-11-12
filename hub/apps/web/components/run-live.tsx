"use client";
import { useEffect, useState } from "react";
import type { RunJson } from "../lib/runs";

type Ev = { event: string; [k: string]: unknown };
type StageState = {
  status: "running" | "done";
  done: number;
  total: number | null;
  label: string;
  items: number;
};
const LOG_MAX = 2000;
const TERMINAL = ["finished", "failed", "cancelled"];

/** One readable line per event; item results are counted per stage instead of logged. */
function line(e: Ev): string | null {
  const i = e.issue as { severity?: string; kind?: string; message?: string } | undefined;
  switch (e.event) {
    case "item-result":
      return null;
    case "stage-started":
      return `▶ ${e.stage}`;
    case "progress":
      return `  ${e.stage}: ${e.done}${e.total == null ? "" : `/${e.total}`} ${e.label}`;
    case "issue-opened":
      return `  [${i?.severity}] ${i?.kind}: ${i?.message}`;
    case "stage-finished":
      return `■ ${e.stage} ${JSON.stringify(e.summary)}`;
    case "hub-stderr":
      return `stderr:\n${e.text}`;
    case "hub-malformed":
      return `(unreadable line) ${e.line}`;
    default:
      return e.message ? `${e.event}: ${e.message}` : JSON.stringify(e);
  }
}

export function RunLive({
  slug,
  initial,
  canCancel,
}: {
  slug: string;
  initial: RunJson;
  canCancel: boolean;
}) {
  const [run, setRun] = useState(initial);
  const [log, setLog] = useState<string[]>([]);
  const [stages, setStages] = useState<Record<string, StageState>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const es = new EventSource(`/api/projects/${slug}/runs/${initial.id}/events`);
    es.onmessage = (m) => {
      const e = JSON.parse(m.data) as Ev;
      const l = line(e);
      if (l) setLog((xs) => [...xs, l].slice(-LOG_MAX));
      const st = typeof e.stage === "string" ? e.stage : null;
      if (!st) return;
      setStages((s) => {
        const cur = s[st] ?? { status: "running", done: 0, total: null, label: "", items: 0 };
        if (e.event === "stage-started") return { ...s, [st]: { ...cur, status: "running" } };
        if (e.event === "progress")
          return {
            ...s,
            [st]: {
              ...cur,
              done: Number(e.done),
              total: e.total == null ? null : Number(e.total),
              label: String(e.label),
            },
          };
        if (e.event === "item-result") return { ...s, [st]: { ...cur, items: cur.items + 1 } };
        if (e.event === "stage-finished") return { ...s, [st]: { ...cur, status: "done" } };
        return s;
      });
    };
    es.addEventListener("end", (m) => {
      setRun(JSON.parse((m as MessageEvent).data) as RunJson);
      es.close(); // otherwise EventSource reconnects
    });
    return () => es.close();
  }, [slug, initial.id]);

  async function cancel() {
    const res = await fetch(`/api/projects/${slug}/runs/${run.id}/cancel`, { method: "POST" });
    const body = await res.json().catch(() => ({}));
    if (res.ok) setRun(body as RunJson);
    else setError((body as { error?: string }).error ?? `cancel failed (${res.status})`);
  }

  const over = TERMINAL.includes(run.state);
  return (
    <>
      <p>
        State: <strong>{run.state}</strong>
        {run.exit_code != null && <> (exit {run.exit_code})</>}
        {run.error && <> — {run.error}</>}
        {run.cancel_requested && !over && <> — cancelling…</>}{" "}
        {canCancel && !over && !run.cancel_requested && (
          <button type="button" onClick={cancel}>
            Cancel
          </button>
        )}
      </p>
      <p>
        Stages: {run.stages === "all" ? "all" : run.stages.join(", ")}
        {run.apply ? " (apply)" : ""} on v{run.base_version}
        {run.result_version != null && (
          <>
            {" "}
            → result <a href={`/p/${slug}`}>v{run.result_version}</a>
          </>
        )}
      </p>
      {error && <p role="alert">{error}</p>}
      {Object.entries(stages).map(([name, s]) => (
        <div key={name}>
          <label>
            {name} {s.status === "done" ? "✓" : ""} ({s.items} items
            {s.label ? `, ${s.done}/${s.total ?? "?"} ${s.label}` : ""})
            <br />
            <progress
              max={s.status === "done" ? 1 : (s.total ?? undefined)}
              value={s.status === "done" ? 1 : s.total ? s.done : undefined}
            />
          </label>
        </div>
      ))}
      <pre style={{ maxHeight: "60vh", overflow: "auto", background: "#f6f6f6", padding: "0.5em" }}>
        {log.join("\n") || (over ? "No events." : "Waiting for events…")}
      </pre>
    </>
  );
}
