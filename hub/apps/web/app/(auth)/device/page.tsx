"use client";

import { useEffect, useState } from "react";
import { AuthCard } from "@/components/porter/auth-card";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { authClient } from "@/lib/auth-client";
import { cn } from "@/lib/utils";

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
    <AuthCard
      title="Approve porter CLI sign-in"
      description="Check the code matches the one in your terminal. Only approve a sign-in you started."
    >
      <Field>
        <FieldLabel htmlFor="code">Code shown in your terminal</FieldLabel>
        <Input
          id="code"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          required
          autoComplete="off"
          spellCheck={false}
          className="h-12 text-center font-mono text-xl! tracking-[0.15em] uppercase"
        />
      </Field>
      <div className="flex flex-wrap gap-2">
        <Button size="lg" className="press h-10 flex-1" onClick={() => decide(true)}>
          Approve
        </Button>
        <Button variant="outline" size="lg" className="press h-10 flex-1" onClick={() => decide(false)}>
          Deny
        </Button>
      </div>
      {message && (
        <p
          role="status"
          className={cn("m-0 rounded-md p-3", ok === false ? "bg-err-soft text-err" : "bg-muted")}
        >
          {message}
        </p>
      )}
    </AuthCard>
  );
}
