"use client";
import { useActionState } from "react";
import { createShare } from "./actions";

type State = { token?: string; expiresAt?: string; error?: string } | null;

export function ShareForm({ slug }: { slug: string }) {
  const [state, action] = useActionState<State, FormData>(createShare.bind(null, slug), null);
  return (
    <>
      <form action={action}>
        <select name="scope" defaultValue="report">
          <option value="report">report (reports only)</option>
          <option value="project-readonly">project-readonly (overview, issues, URLs, reports)</option>
        </select>{" "}
        <label>
          Expires in <input name="days" type="number" min={1} max={90} defaultValue={14} required /> days
        </label>{" "}
        <button type="submit">Create share link</button>
      </form>
      {state?.error && <p role="alert">{state.error}</p>}
      {state?.token && (
        <p>
          Copy this link now; it is not shown again (expires {state.expiresAt}):
          <br />
          <code>{`${typeof window === "undefined" ? "" : window.location.origin}/s/${state.token}`}</code>
        </p>
      )}
    </>
  );
}
