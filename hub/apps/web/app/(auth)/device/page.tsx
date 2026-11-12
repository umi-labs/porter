"use client";

import { useEffect, useState } from "react";
import { authClient } from "../../../lib/auth-client";

// Error bodies carry `error_description` (RFC 8628) or `message` (our staff check).
const describe = (e: object) =>
  String(("error_description" in e && e.error_description) || ("message" in e && e.message) || "Failed.");

/** Where `porter hub login` sends you to approve the CLI. Needs a signed-in staff or developer. */
export default function DevicePage() {
  const [code, setCode] = useState("");
  const [message, setMessage] = useState("");

  useEffect(() => {
    setCode(new URLSearchParams(location.search).get("user_code") ?? "");
  }, []);

  async function decide(approve: boolean) {
    const userCode = code.trim().toUpperCase();
    // Claims the code for the signed-in user; approving or denying needs the claim.
    const check = await authClient.device({ query: { user_code: userCode } });
    if (check.error) return setMessage(describe(check.error));
    const res = approve
      ? await authClient.device.approve({ userCode })
      : await authClient.device.deny({ userCode });
    if (res.error) return setMessage(describe(res.error));
    setMessage(approve ? "Approved. Return to your terminal." : "Denied.");
  }

  return (
    <main>
      <h1>Approve porter CLI sign-in</h1>
      <label>
        Code shown in your terminal <input value={code} onChange={(e) => setCode(e.target.value)} required />
      </label>
      <button type="button" onClick={() => decide(true)}>
        Approve
      </button>
      <button type="button" onClick={() => decide(false)}>
        Deny
      </button>
      {message && <p role="status">{message}</p>}
    </main>
  );
}
