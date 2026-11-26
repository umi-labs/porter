"use client";

import { type FormEvent, useState } from "react";
import { AuthCard } from "@/components/porter/auth-card";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel, FieldSeparator } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { authClient } from "@/lib/auth-client";
import { cn } from "@/lib/utils";

/** `foundry`: Sign in with Foundry hub is configured (FOUNDRY_OIDC_*). */
export function SignInForm({ foundry }: { foundry: boolean }) {
  const [message, setMessage] = useState("");
  const [usePassword, setUsePassword] = useState(false);
  // Bumped on each failed password sign-in, so the shake replays.
  const [failures, setFailures] = useState(0);

  async function password(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const f = new FormData(e.currentTarget);
    const { error } = await authClient.signIn.email({
      email: String(f.get("email")),
      password: String(f.get("password")),
      callbackURL: "/",
    });
    setMessage(error?.message ?? "");
    if (error) setFailures((n) => n + 1);
  }

  async function magic(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const email = String(new FormData(e.currentTarget).get("email"));
    const { error } = await authClient.signIn.magicLink({ email, callbackURL: "/" });
    // Same wording whether or not the email is invited: the server doesn't say either.
    setMessage(
      error?.message ?? "If that email has been invited to a project, a sign-in link is on its way.",
    );
  }

  const bad = usePassword && failures > 0 && !!message;
  return (
    <AuthCard title="Sign in to Porter">
      <Button
        variant="outline"
        size="lg"
        className="press h-10 w-full"
        onClick={() => authClient.signIn.social({ provider: "google", callbackURL: "/" })}
      >
        Continue with Google <span className="font-normal text-muted-foreground">· Umi staff</span>
      </Button>
      {foundry && (
        <Button
          variant="outline"
          size="lg"
          className="press h-10 w-full"
          onClick={() => authClient.signIn.social({ provider: "foundry", callbackURL: "/" })}
        >
          Continue with Foundry hub <span className="font-normal text-muted-foreground">· Umi staff</span>
        </Button>
      )}

      <FieldSeparator>{usePassword ? "or with a password" : "or, if you were invited"}</FieldSeparator>

      {usePassword ? (
        <form onSubmit={password}>
          <FieldGroup className="gap-3">
            <Field>
              <FieldLabel htmlFor="email">Email</FieldLabel>
              <Input id="email" name="email" type="email" autoComplete="username" required className="h-9" />
            </Field>
            <Field data-invalid={bad || undefined}>
              <FieldLabel htmlFor="password">Password</FieldLabel>
              <Input
                key={failures}
                id="password"
                name="password"
                type="password"
                autoComplete="current-password"
                required
                aria-invalid={bad || undefined}
                className={cn("h-9", bad && "animate-shake")}
              />
            </Field>
            <Button type="submit" size="lg" className="press h-10">
              Sign in
            </Button>
          </FieldGroup>
        </form>
      ) : (
        <form onSubmit={magic}>
          <FieldGroup className="gap-3">
            <Field>
              <FieldLabel htmlFor="email">Email</FieldLabel>
              <Input
                id="email"
                name="email"
                type="email"
                autoComplete="email"
                placeholder="you@company.com"
                required
                className="h-9"
              />
            </Field>
            <Button type="submit" size="lg" className="press h-10">
              Email me a sign-in link
            </Button>
          </FieldGroup>
        </form>
      )}
      <Button
        variant="link"
        className="min-h-11 self-start px-0 text-sm"
        onClick={() => {
          setUsePassword(!usePassword);
          setMessage("");
          setFailures(0);
        }}
      >
        {usePassword ? "Email me a link instead" : "Use a password instead"}
      </Button>

      {message && (
        <p
          role="status"
          className={cn("m-0 rounded-md p-3", bad ? "bg-err-soft text-err" : "bg-muted text-foreground")}
        >
          {message}
        </p>
      )}
    </AuthCard>
  );
}
