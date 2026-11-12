"use client";
import { STAGES } from "@porter-hub/porter/stages";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { buttonClass } from "./ui/button";

/**
 * Queues a remote run. Apply is never the default and needs the destination host typed out. Apply runs load on
 * its own (the hub refuses "all" with apply), so ticking it switches the form to load only.
 */
export function RunForm({ slug, host }: { slug: string; host: string | null }) {
  const router = useRouter();
  const [all, setAll] = useState(true);
  const [stages, setStages] = useState<string[]>([]);
  const [apply, setApply] = useState(false);
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const loads = all || stages.includes("load");
  const hostOk = !!host && confirm.trim().toLowerCase() === host.toLowerCase();

  function toggleApply(on: boolean) {
    setApply(on);
    if (on) {
      setAll(false);
      setStages(["load"]);
    }
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const res = await fetch(`/api/projects/${slug}/runs`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        stages: all ? "all" : stages,
        apply: loads && apply,
        ...(loads && apply ? { confirm_host: confirm } : {}),
      }),
    }).catch((x: Error) => ({ ok: false, status: 0, json: async () => ({ error: x.message }) }));
    const body = (await res.json().catch(() => ({}))) as { id?: string; error?: string };
    setBusy(false);
    if (res.ok && body.id) router.push(`/p/${slug}/runs/${body.id}`);
    else setError(body.error ?? `failed (${res.status})`);
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-3 rounded-lg border border-border bg-surface p-4">
      <fieldset className="m-0 flex flex-col gap-3 border-0 p-0">
        <legend className="mb-2 p-0 text-[14px] font-semibold">Run on the hub (latest version)</legend>
        <label className="flex min-h-11 items-center gap-2">
          <input
            type="checkbox"
            checked={all}
            onChange={(e) => {
              setAll(e.target.checked);
              if (e.target.checked) setApply(false);
            }}
          />{" "}
          All stages (<code>porter run</code>)
        </label>
        {!all && (
          <div className="flex flex-wrap gap-x-4">
            {STAGES.map((s) => (
              <label key={s} className="flex min-h-11 items-center gap-2">
                <input
                  type="checkbox"
                  checked={stages.includes(s)}
                  onChange={(e) =>
                    setStages(e.target.checked ? [...stages, s] : stages.filter((x) => x !== s))
                  }
                />
                {s}
              </label>
            ))}
          </div>
        )}
        {loads && (
          <div className="flex flex-col gap-2 rounded-md border border-dashed border-border-strong p-3">
            <label className="flex min-h-11 items-center gap-2">
              <input type="checkbox" checked={apply} onChange={(e) => toggleApply(e.target.checked)} />
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                aria-hidden="true"
              >
                <rect x="5" y="11" width="14" height="10" rx="2" />
                <path d="M8 11V8a4 4 0 0 1 8 0v3" />
              </svg>{" "}
              Apply: write to the destination. Otherwise load is a dry run.
            </label>
            {apply && (
              <small className="text-small text-muted">
                Apply runs load on its own, after a dry run. Open errors in this version block it.
              </small>
            )}
            {apply &&
              (host ? (
                <label className="flex flex-col gap-1.5 text-[14px]">
                  <span>
                    Type the destination host to confirm (<code>{host}</code>)
                  </span>
                  <input
                    value={confirm}
                    onChange={(e) => setConfirm(e.target.value)}
                    onPaste={(e) => e.preventDefault()}
                    autoComplete="off"
                    spellCheck={false}
                    required
                    className="font-mono"
                  />
                </label>
              ) : (
                <em className="text-err not-italic">
                  porter.toml has no [destination] url, so there is nothing to apply to.
                </em>
              ))}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-3">
          <button
            type="submit"
            disabled={busy || (!all && !stages.length) || (loads && apply && !hostOk)}
            className={buttonClass(loads && apply ? "danger" : "primary")}
          >
            {busy
              ? "Queueing…"
              : loads && apply
                ? "Run and apply"
                : loads
                  ? "Run (load as a dry run)"
                  : "Run"}
          </button>
          <small className="text-small text-muted">wp-db connectors are skipped on the hub.</small>
        </div>
        {error && (
          <p role="alert" className="m-0 text-err">
            {error}
          </p>
        )}
      </fieldset>
    </form>
  );
}
