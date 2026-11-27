"use client";
import { CopyIcon } from "lucide-react";
import Link from "next/link";
import { useEffect, useState } from "react";
import { Banner, type Tone } from "@/components/porter/banner";
import { Chip, type State } from "@/components/porter/chip";
import { PageTitle } from "@/components/porter/frame";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { isDryRun, runLook } from "@/lib/run-look";
import type { RunJson } from "@/lib/runs";
import { PIPELINE, runState, type Summary } from "@/lib/stage-state";

type Ev = { event: string; [k: string]: unknown };
type Lvl = "info" | "warn" | "error";
type Line = { text: string; lvl: Lvl };
type StageState = {
  status: "running" | "done";
  done: number;
  total: number | null;
  label: string;
  items: number;
  summary?: Summary;
};
const LOG_MAX = 2000;
const TERMINAL = ["finished", "failed", "cancelled"];

/** One readable line per event; item results are counted per stage instead of logged. */
function line(e: Ev): Line | null {
  const i = e.issue as { severity?: string; kind?: string; message?: string } | undefined;
  switch (e.event) {
    case "item-result":
      return null;
    case "stage-started":
      return { text: `▶ ${e.stage}`, lvl: "info" };
    case "progress":
      return {
        text: `  ${e.stage}: ${e.done}${e.total == null ? "" : `/${e.total}`} ${e.label}`,
        lvl: "info",
      };
    case "issue-opened":
      return {
        text: `  [${i?.severity}] ${i?.kind}: ${i?.message}`,
        lvl: i?.severity === "error" ? "error" : i?.severity === "warning" ? "warn" : "info",
      };
    case "stage-finished":
      return { text: `■ ${e.stage} ${JSON.stringify(e.summary)}`, lvl: "info" };
    case "hub-stderr":
      return { text: `stderr:\n${e.text}`, lvl: "warn" };
    case "hub-malformed":
      return { text: `(unreadable line) ${e.line}`, lvl: "warn" };
    default:
      return { text: e.message ? `${e.event}: ${e.message}` : JSON.stringify(e), lvl: "info" };
  }
}

const LVL_CLASS: Record<Lvl, string> = {
  info: "text-neutral-300",
  warn: "text-amber-300",
  error: "bg-red-950/70 text-red-300",
};

function result(
  run: RunJson,
  look: ReturnType<typeof runLook>,
): { tone: Tone; title: string; body?: string } {
  if (run.state === "failed")
    return { tone: "err", title: "The run couldn’t finish.", body: run.error ?? undefined };
  if (run.state === "cancelled") return { tone: "int", title: "Cancelled before it finished." };
  if (look.state === "errors")
    return { tone: "err", title: "Finished with error issues.", body: "Fix them, then run again." };
  if (look.state === "interrupted") return { tone: "int", title: "Interrupted." };
  if (look.state === "dry-run")
    return { tone: "dry", title: "Finished. Nothing was written: load ran as a dry run." };
  if (look.state === "warnings") return { tone: "warn", title: "Finished, with warnings." };
  if (look.state === "clean") return { tone: "ok", title: "Finished clean." };
  return { tone: "info", title: "Finished." };
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
  const [log, setLog] = useState<Line[]>([]);
  const [stages, setStages] = useState<Record<string, StageState>>({});
  const [error, setError] = useState<string | null>(null);
  const [lost, setLost] = useState(false);
  const [onlyProblems, setOnlyProblems] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    const es = new EventSource(`/api/projects/${slug}/runs/${initial.id}/events`);
    es.onopen = () => setLost(false);
    es.onerror = () => setLost(true); // EventSource reconnects by itself, resuming after Last-Event-ID
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
        if (e.event === "stage-finished")
          return { ...s, [st]: { ...cur, status: "done", summary: e.summary as Summary } };
        return s;
      });
    };
    es.addEventListener("end", (m) => {
      setRun(JSON.parse((m as MessageEvent).data) as RunJson);
      setLost(false);
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
  const summaries = Object.values(stages).flatMap((s) => (s.summary ? [s.summary] : []));
  const look = runLook(run, over ? summaries : undefined);
  const planned =
    run.stages === "all" ? [...PIPELINE] : run.stages.map((s) => (s === "schema-check" ? "schema" : s));
  const names = [...new Set([...planned, ...Object.keys(stages)])];
  const res = over ? result(run, look) : null;
  const shown = onlyProblems ? log.filter((l) => l.lvl !== "info") : log;

  // Copies what's on screen with enough context to read it cold (e.g. pasted to someone for review).
  function copyLog() {
    const head = [
      `Porter run ${run.id} (${slug}) · ${run.state}${run.exit_code != null ? ` · exit ${run.exit_code}` : ""}`,
      `${run.stages === "all" ? "porter run" : run.stages.join(", ")}${isDryRun(run) ? " (dry run)" : run.apply ? " (apply)" : ""} · on v${run.base_version}${run.result_version != null ? ` · result v${run.result_version}` : ""}`,
      run.error ? `error: ${run.error}` : null,
      onlyProblems ? "(warnings and errors only)" : null,
    ].filter(Boolean);
    navigator.clipboard?.writeText([...head, "", ...shown.map((l) => l.text)].join("\n")).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      },
      () => setError("Couldn’t copy the log"),
    );
  }

  const stageChip = (name: string): { state: State; label: string } => {
    const s = stages[name];
    if (!s) {
      if (over && name === "verify" && isDryRun(run)) return { state: "skipped", label: "Skipped" };
      return { state: "not-run", label: over ? "Didn’t run" : "Waiting" };
    }
    if (s.status === "running")
      return {
        state: "running",
        label: s.total ? `${Math.floor((s.done / s.total) * 100)}%` : over ? "Stopped" : "Running",
      };
    const dry = name === "load" && !run.apply;
    const state =
      runState({
        id: "",
        stage: name,
        startedAt: "",
        endedAt: "",
        summary: s.summary ?? null,
        dryRun: dry,
      }) ?? "interrupted";
    return { state, label: `${s.items} item${s.items === 1 ? "" : "s"}` };
  };

  return (
    <>
      <PageTitle
        title={
          <span className="flex flex-wrap items-center gap-3">
            Run {run.id.slice(0, 8)}
            {look.state ? (
              <Chip state={look.state} label={look.label} />
            ) : (
              <span className="text-base">{look.label}</span>
            )}
          </span>
        }
        sub={
          <>
            {run.stages === "all" ? "porter run" : run.stages.join(", ")}
            {isDryRun(run) ? " (load as a dry run)" : run.apply ? " (apply)" : ""} · on v{run.base_version}
            {run.result_version != null && (
              <>
                {" "}
                · result{" "}
                <Link href={`/p/${slug}`} className="text-primary hover:underline">
                  v{run.result_version}
                </Link>
              </>
            )}
            {run.cancel_requested && !over && " · cancelling…"}
          </>
        }
        actions={
          canCancel &&
          !over &&
          !run.cancel_requested && (
            <Button variant="destructive" size="lg" className="press h-9" onClick={cancel}>
              Cancel run
            </Button>
          )
        }
      />
      {error && <Banner tone="err" role="alert" title={error} />}
      {res && (
        <Banner
          tone={res.tone}
          title={res.title}
          aside={
            run.exit_code != null && (
              <span className="font-mono text-xs text-muted-foreground">exit {run.exit_code}</span>
            )
          }
        >
          {res.body}
        </Banner>
      )}
      <ol className="m-0 flex list-none flex-wrap gap-2 p-0">
        {names.map((name) => {
          const c = stageChip(name);
          return (
            <li key={name}>
              <Chip state={c.state} label={`${name} · ${c.label}`} />
            </li>
          );
        })}
      </ol>
      <section aria-label="Log" className="overflow-hidden rounded-lg ring-1 ring-foreground/10">
        <div className="flex flex-wrap items-center gap-3 border-b bg-card px-4 py-1 text-xs">
          <span className="font-semibold">Log</span>
          <span className="text-muted-foreground">
            {log.length} lines{over ? "" : " · streaming"}
          </span>
          <span className="flex-1" />
          <Label className="flex min-h-11 items-center gap-2 font-normal">
            <Checkbox checked={onlyProblems} onCheckedChange={setOnlyProblems} />
            Warnings and errors only
          </Label>
          <Button variant="outline" size="sm" className="press" onClick={copyLog} disabled={!shown.length}>
            <CopyIcon data-icon="inline-start" />
            {copied ? "Copied" : "Copy log"}
          </Button>
        </div>
        {lost && !over && (
          <p role="status" className="m-0 bg-warn-soft px-4 py-2 text-xs text-warn">
            Lost the live log. Reconnecting… The run itself is still going on the server; this only affects
            what you see.
          </p>
        )}
        <pre className="m-0 max-h-[60vh] overflow-auto bg-neutral-950 p-0 py-2 font-mono text-xs leading-5">
          {shown.length === 0 ? (
            <span className="block px-4 text-neutral-500">
              {over ? "No events." : log.length ? "No warnings or errors yet." : "Connecting to log…"}
            </span>
          ) : (
            shown.map((l, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: the log only appends
              <span key={i} className={`block px-4 whitespace-pre-wrap break-words ${LVL_CLASS[l.lvl]}`}>
                {l.text}
              </span>
            ))
          )}
        </pre>
      </section>
    </>
  );
}
