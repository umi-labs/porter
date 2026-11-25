"use client";

import { type FormEvent, useState } from "react";
import { AuthCard } from "@/components/porter/auth-card";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";

/** Staff password sign-up. Needs the invitation id from `invite-staff` (the link it prints: /sign-up?invitation=<id>). */
export default function SignUpPage() {
  const [message, setMessage] = useState("");

  async function signUp(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const f = new FormData(e.currentTarget);
    const invitationId = new URLSearchParams(window.location.search).get("invitation") ?? "";
    const r = await fetch("/api/auth/sign-up/email", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        name: String(f.get("name")),
        email: String(f.get("email")),
        password: String(f.get("password")),
        invitationId,
      }),
    });
    const body = (await r.json().catch(() => ({}))) as { message?: string };
    setMessage(
      r.ok
        ? "Check your email for a verification link, then sign in with your password."
        : (body.message ?? `Sign-up failed (${r.status})`),
    );
  }

  return (
    <AuthCard title="Create your staff account">
      <form onSubmit={signUp}>
        <FieldGroup className="gap-3">
          <Field>
            <FieldLabel htmlFor="name">Name</FieldLabel>
            <Input id="name" name="name" autoComplete="name" required className="h-9" />
          </Field>
          <Field>
            <FieldLabel htmlFor="email">Email (the invited address)</FieldLabel>
            <Input id="email" name="email" type="email" autoComplete="username" required className="h-9" />
          </Field>
          <Field>
            <FieldLabel htmlFor="password">Password (12+ characters)</FieldLabel>
            <Input
              id="password"
              name="password"
              type="password"
              autoComplete="new-password"
              minLength={12}
              required
              className="h-9"
            />
          </Field>
          <Button type="submit" size="lg" className="press h-10">
            Create account
          </Button>
        </FieldGroup>
      </form>
      {message && (
        <p role="status" className="m-0 rounded-md bg-muted p-3">
          {message}
        </p>
      )}
    </AuthCard>
  );
}
