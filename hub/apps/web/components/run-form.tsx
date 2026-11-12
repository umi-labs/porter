"use client";
import { STAGES } from "@porter-hub/porter/config";
import { useRouter } from "next/navigation";
import { useState } from "react";

/** Queues a remote run. Apply is never the default and needs the destination host typed out. */
export function RunForm({ slug, host }: { slug: string; host: string | null }) {
  const router = useRouter();
  const [all, setAll] = useState(true);
  const [stages, setStages] = useState<string[]>([]);
  const [apply, setApply] = useState(false);
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const loads = all || stages.includes("load");

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
    <form onSubmit={submit}>
      <fieldset>
        <legend>Run on the hub (latest version)</legend>
        <label>
          <input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} /> all stages (
          <code>porter run</code>)
        </label>
        {!all &&
          STAGES.map((s) => (
            <label key={s} style={{ marginLeft: "1em" }}>
              <input
                type="checkbox"
                checked={stages.includes(s)}
                onChange={(e) => setStages(e.target.checked ? [...stages, s] : stages.filter((x) => x !== s))}
              />{" "}
              {s}
            </label>
          ))}
        {loads && (
          <p>
            <label>
              <input type="checkbox" checked={apply} onChange={(e) => setApply(e.target.checked)} /> apply
              (write to the destination; otherwise load is a dry run)
            </label>
            {apply && (
              <>
                <br />
                {host ? (
                  <label>
                    Type the destination host to confirm (<code>{host}</code>):{" "}
                    <input value={confirm} onChange={(e) => setConfirm(e.target.value)} required />
                  </label>
                ) : (
                  <em>porter.toml has no [destination] url, so there is nothing to apply to.</em>
                )}
              </>
            )}
          </p>
        )}
        <p>
          <button type="submit" disabled={busy || (!all && !stages.length)}>
            {busy ? "Queueing…" : "Run"}
          </button>{" "}
          <small>wp-db connectors are skipped on the hub.</small>
        </p>
        {error && <p role="alert">{error}</p>}
      </fieldset>
    </form>
  );
}
