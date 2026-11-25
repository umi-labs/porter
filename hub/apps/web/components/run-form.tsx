"use client";
import { STAGES } from "@porter-hub/porter/stages";
import { LockIcon } from "lucide-react";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

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

  const check = "flex min-h-11 items-center gap-2 text-sm font-normal";
  return (
    <form onSubmit={submit} className="flex flex-col gap-3 rounded-lg border bg-muted/40 p-4">
      <FieldSet className="gap-3">
        <FieldLegend variant="label">Run on the hub (latest version)</FieldLegend>
        <Label className={check}>
          <Checkbox
            checked={all}
            onCheckedChange={(on) => {
              setAll(on);
              if (on) setApply(false);
            }}
          />
          <span>
            All stages (<code className="text-xs">porter run</code>)
          </span>
        </Label>
        {!all && (
          <div className="flex flex-wrap gap-x-4">
            {STAGES.map((s) => (
              <Label key={s} className={check}>
                <Checkbox
                  checked={stages.includes(s)}
                  onCheckedChange={(on) => setStages(on ? [...stages, s] : stages.filter((x) => x !== s))}
                />
                {s}
              </Label>
            ))}
          </div>
        )}
        {loads && (
          <div className="flex flex-col gap-2 rounded-md border border-dashed p-3">
            <Label className={check}>
              <Checkbox checked={apply} onCheckedChange={toggleApply} />
              <LockIcon className="size-3.5" aria-hidden="true" />
              <span>Apply: write to the destination. Otherwise load is a dry run.</span>
            </Label>
            {apply && (
              <FieldDescription>
                Apply runs load on its own, after a dry run. Open errors in this version block it.
              </FieldDescription>
            )}
            {apply &&
              (host ? (
                <Field>
                  <FieldLabel htmlFor="confirm-host" className="font-normal">
                    <span>
                      Type the destination host to confirm (<code className="text-xs">{host}</code>)
                    </span>
                  </FieldLabel>
                  <Input
                    id="confirm-host"
                    value={confirm}
                    onChange={(e) => setConfirm(e.target.value)}
                    onPaste={(e) => e.preventDefault()}
                    autoComplete="off"
                    spellCheck={false}
                    required
                    className="h-9 font-mono"
                  />
                </Field>
              ) : (
                <em className="text-err not-italic">
                  porter.toml has no [destination] url, so there is nothing to apply to.
                </em>
              ))}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-3">
          <Button
            type="submit"
            size="lg"
            variant={loads && apply ? "destructive" : "default"}
            disabled={busy || (!all && !stages.length) || (loads && apply && !hostOk)}
            className="press h-9 px-4"
          >
            {busy
              ? "Queueing…"
              : loads && apply
                ? "Run and apply"
                : loads
                  ? "Run (load as a dry run)"
                  : "Run"}
          </Button>
          <span className="text-xs text-muted-foreground">wp-db connectors are skipped on the hub.</span>
        </div>
        {error && <FieldError>{error}</FieldError>}
      </FieldSet>
    </form>
  );
}
