"use client";
import { CopyIcon } from "lucide-react";
import { useActionState, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldLabel,
  FieldLegend,
  FieldSet,
  FieldTitle,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { createShare } from "./actions";

type State = { token?: string; expiresAt?: string; error?: string } | null;

const SCOPES = [
  ["report", "Reports only", "Porter’s reports. No overview, issues, URLs or raw data."],
  [
    "project-readonly",
    "Read-only project",
    "Overview, issues (no messages), URLs and reports. Not for clients.",
  ],
] as const;

/** Creates a share link. The token is shown once, here, then only its fingerprint in the list. */
export function ShareForm({ slug }: { slug: string }) {
  const [state, action, pending] = useActionState<State, FormData>(createShare.bind(null, slug), null);
  const [shown, setShown] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    setShown(state?.token ?? null);
    setCopied(false);
  }, [state]);
  const url = shown ? `${typeof window === "undefined" ? "" : window.location.origin}/s/${shown}` : "";

  return (
    <>
      {shown && state?.expiresAt && (
        <section
          aria-labelledby="share-once"
          className="animate-enter flex flex-col gap-3.5 rounded-xl border-2 border-primary bg-card p-5"
        >
          <div className="flex flex-col gap-0.5">
            <h3 id="share-once" className="m-0 text-base font-semibold">
              Copy this link now. It won’t be shown again.
            </h3>
            <span className="text-muted-foreground">
              We only keep a fingerprint of it. If you lose it, revoke it and make a new one.
            </span>
          </div>
          <div className="flex flex-wrap gap-2">
            <div className="flex min-w-0 flex-[1_1_360px]">
              <Input
                aria-label="Share link"
                readOnly
                value={url}
                onFocus={(e) => e.currentTarget.select()}
                className="h-9 bg-muted font-mono text-xs"
              />
            </div>
            <Button
              size="lg"
              className="press h-9"
              onClick={() =>
                navigator.clipboard?.writeText(url).then(
                  () => setCopied(true),
                  () => {},
                )
              }
            >
              <CopyIcon data-icon="inline-start" />
              {copied ? "Copied" : "Copy link"}
            </Button>
          </div>
          <span className="text-muted-foreground">Expires {new Date(state.expiresAt).toUTCString()}</span>
          <div className="flex justify-end">
            <Button variant="outline" className="press" onClick={() => setShown(null)}>
              I’ve copied it
            </Button>
          </div>
        </section>
      )}
      <form action={action} className="flex flex-col gap-4">
        <FieldSet>
          <FieldLegend variant="label">What can they see?</FieldLegend>
          <RadioGroup
            name="scope"
            defaultValue="report"
            className="grid grid-cols-[repeat(auto-fit,minmax(240px,1fr))] gap-2.5"
          >
            {SCOPES.map(([value, title, sub]) => (
              <FieldLabel key={value} htmlFor={`scope-${value}`}>
                <Field orientation="horizontal">
                  <RadioGroupItem value={value} id={`scope-${value}`} />
                  <FieldContent>
                    <FieldTitle>{title}</FieldTitle>
                    <FieldDescription>{sub}</FieldDescription>
                  </FieldContent>
                </Field>
              </FieldLabel>
            ))}
          </RadioGroup>
        </FieldSet>
        <div className="flex flex-wrap items-end gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="share-days">Expires after</Label>
            <NativeSelect id="share-days" name="days" defaultValue="14">
              <NativeSelectOption value="1">1 day</NativeSelectOption>
              <NativeSelectOption value="7">7 days</NativeSelectOption>
              <NativeSelectOption value="14">14 days</NativeSelectOption>
              <NativeSelectOption value="30">30 days</NativeSelectOption>
              <NativeSelectOption value="90">90 days</NativeSelectOption>
            </NativeSelect>
          </div>
          <Button type="submit" disabled={pending} className="press">
            Create link
          </Button>
        </div>
        <span className="text-xs text-muted-foreground">
          There’s no “never expires”. 90 days is the longest.
        </span>
        {state?.error && <FieldError>{state.error}</FieldError>}
      </form>
    </>
  );
}
