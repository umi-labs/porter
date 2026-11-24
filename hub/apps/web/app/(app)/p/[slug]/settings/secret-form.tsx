"use client";
import { useActionState } from "react";
import { Button } from "@/components/ui/button";
import { FieldError } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
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
        <div className="flex min-w-0 flex-[1_1_200px]">
          <Input
            aria-label="Name"
            name="name"
            placeholder="ENV_VAR_NAME"
            pattern="[A-Z][A-Z0-9_]{0,63}"
            required
            className="font-mono"
          />
        </div>
      )}
      <div className="flex min-w-0 flex-[1_1_200px]">
        <Input
          aria-label={name ? `New value for ${name}` : "Value"}
          name="value"
          type="password"
          autoComplete="off"
          placeholder={name ? "new value" : "value"}
          required
        />
      </div>
      <Button type="submit" variant={name ? "outline" : "default"} disabled={pending} className="press">
        {name ? "Replace" : "Add secret"}
      </Button>
      {state?.error && <FieldError className="w-full">{state.error}</FieldError>}
      {state?.saved && !name && <span className="text-muted-foreground">Saved {state.saved}.</span>}
    </form>
  );
}
