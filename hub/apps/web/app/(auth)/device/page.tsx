"use client";

import { useEffect, useState } from "react";
import { Button } from "../../../components/ui/button";
import { authClient } from "../../../lib/auth-client";

// Error bodies carry `error_description` (RFC 8628) or `message` (our staff check).
const describe = (e: object) =>
  String(("error_description" in e && e.error_description) || ("message" in e && e.message) || "Failed.");

/** Where `porter hub login` sends you to approve the CLI. Needs a signed-in staff or developer. */
export default function DevicePage() {
  const [code, setCode] = useState("");
  const [message, setMessage] = useState("");
  const [ok, setOk] = useState<boolean | null>(null);

  useEffect(() => {
    setCode(new URLSearchParams(location.search).get("user_code") ?? "");
  }, []);

  async function decide(approve: boolean) {
    const userCode = code.trim().toUpperCase();
    setOk(null);
    // Claims the code for the signed-in user; approving or denying needs the claim.
    const check = await authClient.device({ query: { user_code: userCode } });
    if (check.error) {
      setOk(false);
      return setMessage(describe(check.error));
    }
    const res = approve
      ? await authClient.device.approve({ userCode })
      : await authClient.device.deny({ userCode });
    if (res.error) {
      setOk(false);
      return setMessage(describe(res.error));
    }
    setOk(true);
    setMessage(approve ? "Approved. Return to your terminal." : "Denied.");
  }

  return (
    <main className="mx-auto flex min-h-dvh max-w-[460px] flex-col justify-center gap-5 px-4 py-10">
      <div className="flex flex-col gap-5 rounded-xl border border-border bg-bg p-6">
        <div className="flex flex-col gap-1">
          <h1 className="m-0 text-title font-semibold">Approve porter CLI sign-in</h1>
          <span className="text-[14px] text-muted">
            Check the code matches the one in your terminal. Only approve a sign-in you started.
          </span>
        </div>
        <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
          Code shown in your terminal
          <input
            value={code}
            onChange={(e) => setCode(e.target.value)}
            required
            autoComplete="off"
            spellCheck={false}
            className="text-center font-mono text-title tracking-[0.15em] uppercase"
          />
        </label>
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" className="flex-1" onClick={() => decide(true)}>
            Approve
          </Button>
          <Button variant="secondary" className="flex-1" onClick={() => decide(false)}>
            Deny
          </Button>
        </div>
        {message && (
          <p
            role="status"
            className={`m-0 rounded-md p-3 text-[14px] ${ok === false ? "bg-err-bg text-err" : "bg-surface"}`}
          >
            {message}
          </p>
        )}
      </div>
    </main>
  );
}
