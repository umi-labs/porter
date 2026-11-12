"use client";
import { useActionState } from "react";
import { putSecret } from "./actions";

type State = { saved?: string; error?: string } | null;

/** Sets a run secret (new, or a replacement when `name` is given). The value is never shown again. */
export function SecretForm({ slug, name }: { slug: string; name?: string }) {
  const [state, action, pending] = useActionState<State, FormData>(putSecret.bind(null, slug), null);
  return (
    <form action={action} style={{ display: "inline" }}>
      {name ? (
        <input type="hidden" name="name" value={name} />
      ) : (
        <input name="name" placeholder="ENV_VAR_NAME" pattern="[A-Z][A-Z0-9_]{0,63}" required />
      )}{" "}
      <input
        name="value"
        type="password"
        autoComplete="off"
        placeholder={name ? "new value" : "value"}
        required
      />{" "}
      <button type="submit" disabled={pending}>
        {name ? "Replace" : "Add secret"}
      </button>
      {state?.error && <span role="alert"> {state.error}</span>}
      {state?.saved && !name && <span> Saved {state.saved}.</span>}
    </form>
  );
}
