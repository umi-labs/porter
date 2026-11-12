"use client";

import { type FormEvent, useState } from "react";
import { buttonClass } from "../../../components/ui/button";

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
    <main className="mx-auto flex min-h-dvh max-w-[420px] flex-col justify-center gap-5 px-4 py-10">
      <form onSubmit={signUp} className="flex flex-col gap-3 rounded-xl border border-border bg-bg p-6">
        <h1 className="m-0 text-title font-semibold">Create your staff account</h1>
        <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
          Name
          <input name="name" autoComplete="name" required className="font-normal" />
        </label>
        <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
          Email (the invited address)
          <input name="email" type="email" autoComplete="username" required className="font-normal" />
        </label>
        <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
          Password (12+ characters)
          <input
            name="password"
            type="password"
            autoComplete="new-password"
            minLength={12}
            required
            className="font-normal"
          />
        </label>
        <button type="submit" className={buttonClass("dark")}>
          Create account
        </button>
        {message && (
          <p role="status" className="m-0 rounded-md bg-surface p-3 text-[14px]">
            {message}
          </p>
        )}
      </form>
    </main>
  );
}
