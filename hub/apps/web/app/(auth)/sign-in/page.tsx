"use client";

import { type FormEvent, useState } from "react";
import { authClient } from "../../../lib/auth-client";

export default function SignInPage() {
  const [message, setMessage] = useState("");

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
    <main>
      <h1>Sign in to Porter</h1>

      <section>
        <h2>Umi staff</h2>
        <button
          type="button"
          onClick={() => authClient.signIn.social({ provider: "google", callbackURL: "/" })}
        >
          Sign in with Google
        </button>
        <form onSubmit={password}>
          <label>
            Email <input name="email" type="email" autoComplete="username" required />
          </label>
          <label>
            Password <input name="password" type="password" autoComplete="current-password" required />
          </label>
          <button type="submit">Sign in</button>
        </form>
      </section>

      <section>
        <h2>Clients</h2>
        <form onSubmit={magic}>
          <label>
            Email <input name="email" type="email" autoComplete="email" required />
          </label>
          <button type="submit">Email me a sign-in link</button>
        </form>
      </section>

      {message && <p role="status">{message}</p>}
    </main>
  );
}
