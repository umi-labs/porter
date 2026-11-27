import { account, type connect } from "@porter-hub/db";
import { eq } from "drizzle-orm";

type Db = ReturnType<typeof connect>["db"];

const LABELS: Record<string, string> = {
  credential: "Email and password",
  google: "Google",
  foundry: "Foundry hub",
};

export type SignInWay = { id: string; label: string; linkedAt: Date | null };

/**
 * How a user can sign in: every account row they have, plus each sign-in provider this hub has
 * switched on that they haven't linked yet (linkedAt null). Signing in with one of those using the
 * same staff email links it.
 */
export async function signInWays(db: Db, userId: string, env: Record<string, string | undefined> = process.env): Promise<SignInWay[]> {
  const rows = await db
    .select({ id: account.providerId, linkedAt: account.createdAt })
    .from(account)
    .where(eq(account.userId, userId))
    .orderBy(account.createdAt);
  const on = [
    env.GOOGLE_CLIENT_ID && "google",
    env.FOUNDRY_OIDC_ISSUER && env.FOUNDRY_OIDC_CLIENT_ID && env.FOUNDRY_OIDC_CLIENT_SECRET && "foundry",
  ].filter((p): p is string => !!p);
  const linked = new Set(rows.map((r) => r.id));
  return [
    ...rows.map((r) => ({ ...r, label: LABELS[r.id] ?? r.id })),
    ...on.filter((p) => !linked.has(p)).map((id) => ({ id, label: LABELS[id], linkedAt: null })),
  ];
}
