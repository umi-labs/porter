/**
 * `pnpm -C hub admin:invite-staff <email>`: creates the staff org if missing and a pending staff invitation, so
 * that email can sign up with a password (the fallback to Google). The invitation id is the sign-up secret: it is
 * printed (with the sign-up link) for the admin to pass on. Signing in with the verified email makes them staff.
 * In the worker container: `docker compose exec worker node_modules/.bin/tsx apps/worker/src/invite-staff.ts <email>`.
 */
import { randomUUID } from "node:crypto";
import { connect, ensureStaffOrg, invitation, user } from "@porter-hub/db";
import { and, eq, sql } from "drizzle-orm";

const DAYS = 7;
/** Invitations need an inviter; before any staff exists, it is this user, which has no way to sign in. */
const SYSTEM = { id: "porter-system", name: "Porter (system)", email: "system@porter.invalid" };

export async function inviteStaff(db: ReturnType<typeof connect>["db"], email: string) {
  const orgId = await ensureStaffOrg(db);
  const now = new Date();
  await db
    .insert(user)
    .values({ ...SYSTEM, emailVerified: false, createdAt: now, updatedAt: now })
    .onConflictDoNothing();
  // One live invitation per email: a re-run replaces the old one.
  await db
    .update(invitation)
    .set({ status: "canceled" })
    .where(
      and(
        eq(invitation.organizationId, orgId),
        sql`lower(${invitation.email}) = ${email.toLowerCase()}`,
        eq(invitation.status, "pending"),
      ),
    );
  const [inv] = await db
    .insert(invitation)
    .values({
      id: randomUUID(),
      organizationId: orgId,
      email: email.toLowerCase(),
      role: "member",
      status: "pending",
      expiresAt: new Date(now.getTime() + DAYS * 24 * 3_600_000),
      inviterId: SYSTEM.id,
    })
    .returning();
  return inv;
}

if (process.argv[1]?.endsWith("invite-staff.ts")) {
  const email = process.argv[2] ?? "";
  if (!/^[^@\s]+@[^@\s]+$/.test(email)) {
    console.error("usage: invite-staff <email>");
    process.exit(2);
  }
  const { db, client } = connect();
  inviteStaff(db, email)
    .then((inv) => {
      const base = (process.env.BETTER_AUTH_URL ?? "https://<hub>").replace(/\/$/, "");
      const body = JSON.stringify({
        email: inv.email,
        password: "<12+ characters>",
        name: "<name>",
        invitationId: inv.id,
      });
      console.log(`invited ${inv.email} to the staff org until ${inv.expiresAt.toISOString()}.`);
      console.log(`invitation id: ${inv.id} (the sign-up secret: send it to them privately)`);
      console.log(`sign up at: ${base}/sign-up?invitation=${inv.id}`);
      console.log(`or: curl -sS ${base}/api/auth/sign-up/email -H 'content-type: application/json' \\`);
      console.log(`       -H 'origin: ${base}' -d '${body}'`);
      console.log("Then verify the email (link in the mail) and sign in with the password.");
    })
    .catch((e) => {
      console.error(e instanceof Error ? e.message : e);
      process.exitCode = 1;
    })
    .finally(() => client.end());
}
