"use client";
import { type Setup, SOURCES } from "@porter-hub/porter/sources";
import Link from "next/link";
import { useActionState, useState } from "react";
import { Banner } from "@/components/porter/banner";
import { Button } from "@/components/ui/button";
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

/** Pick a source, then the way in to it, then what that way needs. Saving commits a new version. */
export function SetupForm({
  slug,
  current,
  uploads,
  hasSanityToken,
  hasPayloadKey,
}: {
  slug: string;
  current: Setup | null;
  uploads: Upload[];
  hasSanityToken: boolean;
  hasPayloadKey: boolean;
}) {
  const [state, action, pending] = useActionState<State, FormData>(save.bind(null, slug), null);
  const [adapter, setAdapter] = useState(current?.adapter || SOURCES[0].adapter);
  const source = SOURCES.find((s) => s.adapter === adapter) ?? SOURCES[0];
  const [kind, setKind] = useState(
    source.ways.some((w) => w.kind === current?.kind) ? (current?.kind as string) : source.ways[0].kind,
  );
  const way = source.ways.find((w) => w.kind === kind) ?? source.ways[0];

  function pickSource(a: string) {
    setAdapter(a);
    setKind((SOURCES.find((s) => s.adapter === a) ?? SOURCES[0]).ways[0].kind);
  }

  const cards = "grid grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-2.5";
  return (
    <form action={action} className="flex flex-col gap-6">
      {/* Submitted from state: React resets the form after an action, and the radio groups' own inputs with it. */}
      <input type="hidden" name="adapter" value={adapter} />
      <input type="hidden" name="kind" value={way.kind} />
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
        <RadioGroup value={way.kind} onValueChange={(v) => setKind(String(v))} className={cards}>
          {source.ways.map((w) => (
            <FieldLabel key={w.kind} htmlFor={`way-${w.kind}`}>
              <Field orientation="horizontal">
                <RadioGroupItem value={w.kind} id={`way-${w.kind}`} />
                <FieldContent>
                  <FieldTitle>{w.label}</FieldTitle>
                  <FieldDescription>{w.hint}</FieldDescription>
                </FieldContent>
              </Field>
            </FieldLabel>
          ))}
        </RadioGroup>
      </FieldSet>

      {way.input === "local-only" && (
        <Banner tone="warn" title="Database dumps don’t run on the hub">
          Reading a dump runs the site’s own PHP in Docker, which the hub won’t do on its server. Upload the
          dump under <Link href={`/p/${slug}/uploads`}>Uploads</Link>, then on your machine run{" "}
          <code>porter hub uploads get &lt;id&gt;</code>, point a <code>wp-db</code> connector at it, run
          porter and <code>porter hub push</code>.
        </Banner>
      )}

      <FieldGroup>
        {way.input === "upload" && (
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
                No finished uploads yet. Add the file under <Link href={`/p/${slug}/uploads`}>Uploads</Link>,
                then come back.
              </FieldDescription>
            )}
          </Field>
        )}
        {way.input === "url" && (
          <Field>
            <FieldLabel htmlFor="setup-url">Site URL</FieldLabel>
            <Input
              id="setup-url"
              name="url"
              type="url"
              required
              defaultValue={current?.url}
              placeholder="https://example.com"
            />
          </Field>
        )}
        {way.input === "sanity-api" && (
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
        )}
        <Field>
          <FieldLabel htmlFor="setup-site">
            Live site URL{way.input === "none" ? "" : " (optional)"}
          </FieldLabel>
          <Input
            id="setup-site"
            name="siteUrl"
            type="url"
            required={way.input === "none"}
            defaultValue={current?.siteUrl}
            placeholder="https://www.example.com"
          />
          <FieldDescription>
            Porter crawls it for URLs and media, and checks the old URLs after loading.
          </FieldDescription>
        </Field>
      </FieldGroup>

      <FieldSet>
        <FieldLegend variant="label">Destination: Payload</FieldLegend>
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
      </FieldSet>

      <div className="flex flex-wrap items-center gap-3">
        <Button type="submit" size="lg" disabled={pending || way.input === "local-only"} className="press">
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
