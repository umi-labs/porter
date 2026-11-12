"use client";

import { type FormEvent, useState } from "react";
import { Button, buttonClass } from "../../../components/ui/button";
import { authClient } from "../../../lib/auth-client";

export default function SignInPage() {
  const [message, setMessage] = useState("");
  const [usePassword, setUsePassword] = useState(false);

  async function password(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const f = new FormData(e.currentTarget);
    const { error } = await authClient.signIn.email({
      email: String(f.get("email")),
      password: String(f.get("password")),
      callbackURL: "/",
    });
    setMessage(error?.message ?? "");
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

  return (
    <main className="mx-auto flex min-h-dvh max-w-[420px] flex-col justify-center gap-5 px-4 py-10">
      <div className="flex flex-col gap-5 rounded-xl border border-border bg-bg p-6">
        <h1 className="m-0 text-title font-semibold">Sign in to Porter</h1>

        <Button
          variant="secondary"
          onClick={() => authClient.signIn.social({ provider: "google", callbackURL: "/" })}
        >
          Continue with Google <span className="font-normal text-muted">· Umi staff</span>
        </Button>

        <div className="flex items-center gap-3 text-small text-muted">
          <span className="h-px flex-1 bg-border" />
          {usePassword ? "or with a password" : "or, if you were invited"}
          <span className="h-px flex-1 bg-border" />
        </div>

        {usePassword ? (
          <form onSubmit={password} className="flex flex-col gap-3">
            <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
              Email
              <input name="email" type="email" autoComplete="username" required className="font-normal" />
            </label>
            <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
              Password
              <input
                name="password"
                type="password"
                autoComplete="current-password"
                required
                className="font-normal"
              />
            </label>
            <button type="submit" className={buttonClass("dark")}>
              Sign in
            </button>
          </form>
        ) : (
          <form onSubmit={magic} className="flex flex-col gap-3">
            <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
              Email
              <input
                name="email"
                type="email"
                autoComplete="email"
                placeholder="you@company.com"
                required
                className="font-normal"
              />
            </label>
            <button type="submit" className={buttonClass("dark")}>
              Email me a sign-in link
            </button>
          </form>
        )}
        <button
          type="button"
          className="min-h-11 self-start text-[14px] text-accent hover:underline"
          onClick={() => {
            setUsePassword(!usePassword);
            setMessage("");
          }}
        >
          {usePassword ? "Email me a link instead" : "Use a password instead"}
        </button>

        {message && (
          <p role="status" className="m-0 rounded-md bg-surface p-3 text-[14px]">
            {message}
          </p>
        )}
      </div>
    </main>
  );
}
