"use client";
import { type Setup, SOURCES, siteRoot } from "@porter-hub/porter/sources";
import { CheckIcon, LockIcon } from "lucide-react";
import Link from "next/link";
import { startTransition, useActionState, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
  FieldTitle,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { save } from "./actions";

type Upload = { id: string; filename: string };
type State = { saved?: number; error?: string } | null;

/** Pick a source, then one or more ways in to it, then what each needs. Saving commits a new version. */
export function SetupForm({
  slug,
  current,
  uploads,
  hasSanityToken,
  hasPayloadKey,
  hasWpLogin,
}: {
  slug: string;
  current: Setup | null;
  uploads: Upload[];
  hasSanityToken: boolean;
  hasPayloadKey: boolean;
  hasWpLogin: boolean;
}) {
  const [state, action, pending] = useActionState<State, FormData>(save.bind(null, slug), null);
  const [adapter, setAdapter] = useState(current?.adapter || SOURCES[0].adapter);
  const source = SOURCES.find((s) => s.adapter === adapter) ?? SOURCES[0];
  const runnable = (a: string) =>
    (SOURCES.find((x) => x.adapter === a) ?? SOURCES[0]).ways.filter((w) => w.input !== "local-only");
  const [kinds, setKinds] = useState<string[]>(() => {
    const ok = runnable(adapter).map((w) => w.kind);
    const had = (current?.kinds ?? []).filter((k) => ok.includes(k));
    return had.length ? had : [ok[0]];
  });
  // Controlled: crawl prefills it from the REST URL, and it must survive the form reset after a save.
  const [restUrl, setRestUrl] = useState(current?.url ?? "");
  const [siteUrl, setSiteUrl] = useState(current?.siteUrl ?? "");
  const on = (input: string) => source.ways.some((w) => w.input === input && kinds.includes(w.kind));
  const labelOf = (input: string) => source.ways.find((w) => w.input === input)?.label ?? "";

  function pickSource(a: string) {
    setAdapter(a);
    setKinds([runnable(a)[0].kind]);
  }
  function toggle(kind: string, checked: boolean) {
    const way = source.ways.find((w) => w.kind === kind);
    // One uploaded file at a time: ticking another upload way swaps it in.
    const rest = kinds.filter(
      (k) =>
        k !== kind && !(way?.input === "upload" && source.ways.find((w) => w.kind === k)?.input === "upload"),
    );
    setKinds(checked ? [...rest, kind] : rest);
    if (checked && way?.input === "site" && !siteUrl && restUrl) setSiteUrl(siteRoot(restUrl));
  }

  const cards = "grid grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-2.5";
  return (
    <form
      // Not `action={action}`: React resets a form after an action, and that resets the radio and checkbox
      // groups' hidden inputs out of step with their state (a click then appears to do nothing).
      onSubmit={(e) => {
        e.preventDefault();
        const data = new FormData(e.currentTarget);
        startTransition(() => action(data));
      }}
      className="flex flex-col gap-6"
    >
      {/* Submitted from state, so what's sent is what's shown. */}
      <input type="hidden" name="adapter" value={adapter} />
      <input type="hidden" name="kinds" value={kinds.join(",")} />
      <FieldSet>
        <FieldLegend variant="label">Source</FieldLegend>
        <RadioGroup value={adapter} onValueChange={(v) => pickSource(String(v))} className={cards}>
          {SOURCES.map((s) => (
            <FieldLabel key={s.adapter} htmlFor={`src-${s.adapter}`}>
              <Field orientation="horizontal">
                <RadioGroupItem value={s.adapter} id={`src-${s.adapter}`} />
                <FieldContent>
                  <FieldTitle>{s.label}</FieldTitle>
                </FieldContent>
              </Field>
            </FieldLabel>
          ))}
        </RadioGroup>
      </FieldSet>

      <FieldSet>
        <FieldLegend variant="label">How porter reads it</FieldLegend>
        <FieldDescription>Tick every way in you have. Porter combines them.</FieldDescription>
        <div className={cards}>
          {source.ways.map((w) =>
            w.input === "local-only" ? (
              <div key={w.kind} className="flex gap-2.5 rounded-lg border border-dashed p-2.5">
                <LockIcon className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />
                <FieldContent>
                  <FieldTitle>{w.label}</FieldTitle>
                  <FieldDescription>
                    Local only. Upload it under <Link href={`/p/${slug}/uploads`}>Uploads</Link>, then run{" "}
                    <code>porter hub uploads get &lt;id&gt;</code> and a <code>wp-db</code> connector on your
                    machine, and push.
                  </FieldDescription>
                </FieldContent>
              </div>
            ) : (
              <FieldLabel key={w.kind} htmlFor={`way-${w.kind}`}>
                <Field orientation="horizontal">
                  <Checkbox
                    id={`way-${w.kind}`}
                    checked={kinds.includes(w.kind)}
                    onCheckedChange={(c) => toggle(w.kind, c)}
                  />
                  <FieldContent>
                    <FieldTitle>{w.label}</FieldTitle>
                    <FieldDescription>{w.hint}</FieldDescription>
                  </FieldContent>
                </Field>
              </FieldLabel>
            ),
          )}
        </div>
      </FieldSet>

      <FieldGroup>
        {on("upload") && (
          <WaySection title={labelOf("upload")}>
            <Field>
              <FieldLabel htmlFor="setup-upload">File</FieldLabel>
              {uploads.length ? (
                <NativeSelect
                  id="setup-upload"
                  name="upload"
                  defaultValue={current?.upload ?? uploads[0].id}
                  required
                >
                  {uploads.map((u) => (
                    <NativeSelectOption key={u.id} value={u.id}>
                      {u.filename}
                    </NativeSelectOption>
                  ))}
                </NativeSelect>
              ) : (
                <FieldDescription>
                  No finished uploads yet. Add the file under <Link href={`/p/${slug}/uploads`}>Uploads</Link>
                  , then come back.
                </FieldDescription>
              )}
            </Field>
          </WaySection>
        )}
        {on("url") && (
          <WaySection title={labelOf("url")}>
            <>
              <Field>
                <FieldLabel htmlFor="setup-url">Site or REST API URL</FieldLabel>
                <Input
                  id="setup-url"
                  name="url"
                  type="url"
                  required
                  value={restUrl}
                  onChange={(e) => setRestUrl(e.target.value)}
                  placeholder="https://example.com"
                />
                <FieldDescription>The site’s address, or any /wp-json/ URL on it.</FieldDescription>
              </Field>
              <FieldSet>
                <FieldLegend variant="label">WordPress login (optional)</FieldLegend>
                <FieldDescription>
                  Without one, porter sees published content only: no drafts, menus, settings or user details.
                  Use an application password (Users → Profile → Application Passwords), not the account
                  password. Kept encrypted as run secrets.
                </FieldDescription>
                <div className="flex flex-wrap gap-3">
                  <Field className="flex-[1_1_200px]">
                    <FieldLabel htmlFor="setup-wpuser">Username</FieldLabel>
                    <Input
                      id="setup-wpuser"
                      name="wpUser"
                      autoComplete="off"
                      placeholder={hasWpLogin ? "Saved. Type a new one to replace it" : ""}
                    />
                  </Field>
                  <Field className="flex-[1_1_200px]">
                    <FieldLabel htmlFor="setup-wppass">Application password</FieldLabel>
                    <Input
                      id="setup-wppass"
                      name="wpPassword"
                      type="password"
                      autoComplete="off"
                      placeholder={hasWpLogin ? "Saved. Type a new one to replace it" : ""}
                    />
                  </Field>
                </div>
              </FieldSet>
            </>
          </WaySection>
        )}
        {on("sanity-api") && (
          <WaySection title={labelOf("sanity-api")}>
            <>
              <div className="flex flex-wrap gap-3">
                <Field className="flex-[1_1_180px]">
                  <FieldLabel htmlFor="setup-pid">Project id</FieldLabel>
                  <Input
                    id="setup-pid"
                    name="projectId"
                    required
                    defaultValue={current?.projectId}
                    className="font-mono"
                  />
                </Field>
                <Field className="flex-[1_1_180px]">
                  <FieldLabel htmlFor="setup-ds">Dataset</FieldLabel>
                  <Input
                    id="setup-ds"
                    name="dataset"
                    required
                    defaultValue={current?.dataset ?? "production"}
                    className="font-mono"
                  />
                </Field>
              </div>
              <Field>
                <FieldLabel htmlFor="setup-token">Read token</FieldLabel>
                <Input
                  id="setup-token"
                  name="sanityToken"
                  type="password"
                  autoComplete="off"
                  required={!hasSanityToken}
                  placeholder={hasSanityToken ? "Saved. Type a new one to replace it" : ""}
                />
                <FieldDescription>Kept encrypted as a run secret, never in porter.toml.</FieldDescription>
              </Field>
            </>
          </WaySection>
        )}
        {on("site") && (
          <WaySection title={labelOf("site")}>
            <Field>
              <FieldLabel htmlFor="setup-site">Live site URL</FieldLabel>
              <Input
                id="setup-site"
                name="siteUrl"
                type="url"
                required
                value={siteUrl}
                onChange={(e) => setSiteUrl(e.target.value)}
                onBlur={() => siteUrl && setSiteUrl(siteRoot(siteUrl))}
                placeholder="https://www.example.com"
              />
              <FieldDescription>
                The site’s home page. Porter crawls it for URLs and media, and checks the old URLs after
                loading.
              </FieldDescription>
            </Field>
          </WaySection>
        )}
      </FieldGroup>

      <section aria-label="Destination: Payload" className="flex flex-col gap-3 rounded-lg border p-4">
        <h3 className="m-0 text-sm font-semibold">Destination: Payload</h3>
        <FieldDescription>
          Needed for schema check, load and verify. Without it, runs stop after normalise.
        </FieldDescription>
        <div className="flex flex-wrap gap-3">
          <Field className="flex-[2_1_260px]">
            <FieldLabel htmlFor="setup-dest">Payload URL</FieldLabel>
            <Input
              id="setup-dest"
              name="destUrl"
              type="url"
              defaultValue={current?.destUrl}
              placeholder="https://cms.example.com"
            />
          </Field>
          <Field className="flex-[1_1_200px]">
            <FieldLabel htmlFor="setup-key">API key</FieldLabel>
            <Input
              id="setup-key"
              name="payloadKey"
              type="password"
              autoComplete="off"
              placeholder={hasPayloadKey ? "Saved. Type a new one to replace it" : ""}
            />
          </Field>
        </div>
      </section>

      <div className="flex flex-wrap items-center gap-3">
        <Button type="submit" size="lg" disabled={pending || !kinds.length} className="press">
          Save setup
        </Button>
        {state?.saved != null && (
          <span className="text-muted-foreground">
            Saved as v{state.saved}. <Link href={`/p/${slug}`}>Run it from the overview</Link>.
          </span>
        )}
        {state?.error && <FieldError className="w-full">{state.error}</FieldError>}
      </div>
    </form>
  );
}

/** The details one ticked way in needs, boxed and titled so it's clear which way they belong to. */
function WaySection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section
      aria-label={title}
      className="animate-enter flex flex-col gap-4 rounded-lg border bg-muted/30 p-4"
    >
      <h3 className="m-0 flex items-center gap-2 text-sm font-semibold">
        <span className="grid size-4 place-content-center rounded-[4px] bg-primary text-primary-foreground">
          <CheckIcon className="size-3" aria-hidden />
        </span>
        {title}
      </h3>
      {children}
    </section>
  );
}
