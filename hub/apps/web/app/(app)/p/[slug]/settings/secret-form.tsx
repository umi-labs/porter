"use client";
import { useActionState } from "react";
import { buttonClass } from "../../../../../components/ui/button";
import { putSecret } from "./actions";

type State = { saved?: string; error?: string } | null;

/** Sets a run secret (new, or a replacement when `name` is given). The value is never shown again. */
export function SecretForm({ slug, name }: { slug: string; name?: string }) {
  const [state, action, pending] = useActionState<State, FormData>(putSecret.bind(null, slug), null);
  return (
    <form action={action} className="flex flex-wrap items-center gap-2">
      {name ? (
        <input type="hidden" name="name" value={name} />
      ) : (
        <label className="flex min-w-0 flex-[1_1_200px]">
          <span className="sr-only">Name</span>
          <input
            name="name"
            placeholder="ENV_VAR_NAME"
            pattern="[A-Z][A-Z0-9_]{0,63}"
            required
            className="w-full font-mono"
          />
        </label>
      )}
      <label className="flex min-w-0 flex-[1_1_200px]">
        <span className="sr-only">{name ? `New value for ${name}` : "Value"}</span>
        <input
          name="value"
          type="password"
          autoComplete="off"
          placeholder={name ? "new value" : "value"}
          required
          className="w-full"
        />
      </label>
      <button type="submit" disabled={pending} className={buttonClass(name ? "secondary" : "dark")}>
        {name ? "Replace" : "Add secret"}
      </button>
      {state?.error && (
        <span role="alert" className="text-err">
          {state.error}
        </span>
      )}
      {state?.saved && !name && <span className="text-muted">Saved {state.saved}.</span>}
    </form>
  );
}
