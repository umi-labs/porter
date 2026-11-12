"use client";
import { useActionState, useEffect, useState } from "react";
import { Button, buttonClass } from "../../../../../components/ui/button";
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
          className="flex flex-col gap-3.5 rounded-xl border-2 border-accent bg-bg p-5"
        >
          <div className="flex flex-col">
            <h3 id="share-once" className="m-0 text-heading font-semibold">
              Copy this link now. It won’t be shown again.
            </h3>
            <span className="text-[14px] text-muted">
              We only keep a fingerprint of it. If you lose it, revoke it and make a new one.
            </span>
          </div>
          <div className="flex flex-wrap gap-2">
            <label className="flex min-w-0 flex-[1_1_360px]">
              <span className="sr-only">Share link</span>
              <input
                readOnly
                value={url}
                onFocus={(e) => e.currentTarget.select()}
                className="w-full min-w-0 bg-surface font-mono text-[14px]"
              />
            </label>
            <Button
              variant="primary"
              onClick={() =>
                navigator.clipboard?.writeText(url).then(
                  () => setCopied(true),
                  () => {},
                )
              }
            >
              {copied ? "Copied" : "Copy link"}
            </Button>
          </div>
          <span className="text-[14px] text-muted">Expires {new Date(state.expiresAt).toUTCString()}</span>
          <div className="flex justify-end">
            <Button onClick={() => setShown(null)}>I’ve copied it</Button>
          </div>
        </section>
      )}
      <form action={action} className="flex flex-col gap-4">
        <fieldset className="m-0 flex flex-col gap-2 border-0 p-0">
          <legend className="mb-2 p-0 text-[14px] font-semibold">What can they see?</legend>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(240px,1fr))] gap-2.5">
            {SCOPES.map(([value, title, sub]) => (
              <label
                key={value}
                className="flex cursor-pointer items-start gap-2.5 rounded-lg border border-border-strong p-3.5 has-checked:border-2 has-checked:border-accent has-checked:bg-accent-bg"
              >
                <input
                  type="radio"
                  name="scope"
                  value={value}
                  defaultChecked={value === "report"}
                  className="mt-1"
                />
                <span className="flex flex-col">
                  <span className="font-semibold">{title}</span>
                  <span className="text-[14px] text-muted">{sub}</span>
                </span>
              </label>
            ))}
          </div>
        </fieldset>
        <div className="flex flex-wrap items-end gap-3">
          <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
            Expires after
            <select name="days" defaultValue="14" className="font-normal">
              <option value="1">1 day</option>
              <option value="7">7 days</option>
              <option value="14">14 days</option>
              <option value="30">30 days</option>
              <option value="90">90 days</option>
            </select>
          </label>
          <button type="submit" disabled={pending} className={buttonClass("dark")}>
            Create link
          </button>
        </div>
        <span className="text-small text-muted">There’s no “never expires”. 90 days is the longest.</span>
        {state?.error && (
          <p role="alert" className="m-0 text-err">
            {state.error}
          </p>
        )}
      </form>
    </>
  );
}
