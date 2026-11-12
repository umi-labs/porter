import { randomUUID } from "node:crypto";
import {
  account,
  auditLog,
  type connect,
  ensureStaffOrg,
  getDb,
  invitation,
  member,
  organization as org,
  projectInvitation,
  projectMember,
  schema,
  staffOrgSlug,
  user,
} from "@porter-hub/db";
import { betterAuth } from "better-auth";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { APIError, createAuthMiddleware } from "better-auth/api";
import { bearer, deviceAuthorization, magicLink, organization } from "better-auth/plugins";
import type { BetterAuthPlugin } from "better-auth/types";
import { and, eq, gt, inArray, isNull, sql } from "drizzle-orm";
import { createTransport } from "nodemailer";

export type Mail = { to: string; subject: string; text: string };
type Db = ReturnType<typeof connect>["db"];
type GoogleOverrides = { clientId?: string; clientSecret?: string; verifyIdToken?: () => Promise<boolean> };

/** The CLI's device-authorization client id. */
export const CLI_CLIENT_ID = "porter-cli";

/** True only for exactly `local@<domain>` (case-insensitive): no lookalikes, no subdomains. */
export function isStaffEmail(email: string, domain: string) {
  const at = email.indexOf("@");
  return (
    !!domain &&
    at > 0 &&
    at === email.lastIndexOf("@") &&
    email.slice(at + 1).toLowerCase() === domain.toLowerCase()
  );
}

function smtpMailer() {
  const env = process.env;
  const transport = createTransport({
    host: env.SMTP_HOST,
    port: Number(env.SMTP_PORT ?? 587),
    auth: env.SMTP_USER ? { user: env.SMTP_USER, pass: env.SMTP_PASSWORD } : undefined,
  });
  return async (m: Mail) => {
    await transport.sendMail({ from: env.SMTP_FROM, ...m });
  };
}

const lower = (
  col: typeof projectInvitation.email | typeof invitation.email | typeof user.email,
  v: string,
) => sql`lower(${col}) = ${v.toLowerCase()}`;

const pendingProjectInvite = (email: string) =>
  and(
    lower(projectInvitation.email, email),
    isNull(projectInvitation.acceptedAt),
    gt(projectInvitation.expiresAt, new Date()),
  );

/** Magic links go only to invited emails and existing client users. */
async function mayGetMagicLink(db: Db, email: string) {
  const [invited, client] = await Promise.all([
    db
      .select({ id: projectInvitation.id })
      .from(projectInvitation)
      .where(pendingProjectInvite(email))
      .limit(1),
    db
      .select({ id: user.id })
      .from(user)
      .innerJoin(projectMember, eq(projectMember.userId, user.id))
      .where(and(lower(user.email, email), eq(projectMember.role, "client")))
      .limit(1),
  ]);
  return invited.length + client.length > 0;
}

// The one staff org (org membership there = staff); shared with the worker's invite-staff script.
export { ensureStaffOrg, staffOrgSlug };

/** Adds a verified staff-domain user who signed in with Google to the staff org, once. */
async function provisionGoogleStaff(db: Db, userId: string, domain: string) {
  const [u] = await db.select().from(user).where(eq(user.id, userId));
  if (!u?.emailVerified || !isStaffEmail(u.email, domain)) return;
  const [google] = await db
    .select({ id: account.id })
    .from(account)
    .where(and(eq(account.userId, userId), eq(account.providerId, "google")))
    .limit(1);
  if (!google) return;
  const orgId = await ensureStaffOrg(db);
  // The unique (org, user) index makes this safe against two simultaneous first sign-ins.
  await db
    .insert(member)
    .values({ id: randomUUID(), organizationId: orgId, userId, role: "member", createdAt: new Date() })
    .onConflictDoNothing({ target: [member.organizationId, member.userId] });
}

/**
 * The password route for staff: a verified user with a pending, unexpired staff-org invitation (invite-staff)
 * becomes a staff-org member on sign-in, as Better Auth's accept-invitation would.
 */
async function acceptStaffInvite(db: Db, userId: string) {
  const [u] = await db.select().from(user).where(eq(user.id, userId));
  if (!u?.emailVerified) return;
  await db.transaction(async (tx) => {
    const accepted = await tx
      .update(invitation)
      .set({ status: "accepted" })
      .where(
        and(
          inArray(
            invitation.organizationId,
            tx.select({ id: org.id }).from(org).where(eq(org.slug, staffOrgSlug())),
          ),
          lower(invitation.email, u.email),
          eq(invitation.status, "pending"),
          gt(invitation.expiresAt, new Date()),
        ),
      )
      .returning();
    if (!accepted.length) return;
    await tx
      .insert(member)
      .values({
        id: randomUUID(),
        organizationId: accepted[0].organizationId,
        userId,
        role: accepted[0].role ?? "member",
        createdAt: new Date(),
      })
      .onConflictDoNothing({ target: [member.organizationId, member.userId] });
  });
}

/** Staff-org member, or an owner/developer on some project. */
async function canApproveDevice(db: Db, userId: string) {
  const [staff, dev] = await Promise.all([
    db
      .select({ id: member.id })
      .from(member)
      .innerJoin(org, eq(org.id, member.organizationId))
      .where(and(eq(member.userId, userId), eq(org.slug, staffOrgSlug())))
      .limit(1),
    db
      .select({ id: projectMember.userId })
      .from(projectMember)
      .where(and(eq(projectMember.userId, userId), inArray(projectMember.role, ["owner", "developer"])))
      .limit(1),
  ]);
  return staff.length + dev.length > 0;
}

/** Turns a verified user's pending project invitations into project roles. */
async function acceptProjectInvites(db: Db, userId: string) {
  const [u] = await db.select().from(user).where(eq(user.id, userId));
  if (!u?.emailVerified) return;
  await db.transaction(async (tx) => {
    const accepted = await tx
      .update(projectInvitation)
      .set({ acceptedAt: new Date() })
      .where(pendingProjectInvite(u.email))
      .returning();
    if (accepted.length === 0) return;
    const added = await tx
      .insert(projectMember)
      .values(accepted.map((i) => ({ projectId: i.projectId, userId: u.id, role: i.role })))
      .onConflictDoNothing()
      .returning({ projectId: projectMember.projectId });
    const inserted = new Set(added.map((a) => a.projectId));
    await tx.insert(auditLog).values(
      accepted.map((i) => ({
        projectId: i.projectId,
        actorId: u.id,
        action: "invitation.accept",
        targetType: "invitation",
        targetId: i.id,
        // alreadyMember: the user already had a role, which was kept.
        detail: { role: i.role, alreadyMember: !inserted.has(i.projectId) },
      })),
    );
  });
}

export function createAuth(
  deps: { db?: Db; sendMail?: (m: Mail) => Promise<void>; google?: GoogleOverrides } = {},
) {
  const env = process.env;
  if (env.NODE_ENV === "production") {
    for (const k of ["BETTER_AUTH_SECRET", "BETTER_AUTH_URL"]) {
      if (!env[k]) throw new Error(`${k} must be set in production`);
    }
    if ((env.BETTER_AUTH_SECRET ?? "").length < 32) {
      throw new Error("BETTER_AUTH_SECRET must be at least 32 characters in production");
    }
  }
  const db = deps.db ?? getDb();
  const sendMail = deps.sendMail ?? smtpMailer();
  const domain = (env.STAFF_EMAIL_DOMAIN ?? "").toLowerCase();

  const google = { clientId: env.GOOGLE_CLIENT_ID, clientSecret: env.GOOGLE_CLIENT_SECRET, ...deps.google };
  if (google.clientId && !domain) throw new Error("STAFF_EMAIL_DOMAIN must be set when Google sign-in is on");

  const staffDeviceApproval = {
    id: "porter-staff-device-approval",
    hooks: {
      before: [
        {
          matcher: (ctx) => ctx.path === "/device/approve",
          handler: createAuthMiddleware(async (ctx) => {
            // Through auth.api so a bearer token counts too (sibling hooks see the raw request).
            const s = await auth.api.getSession({ headers: ctx.headers ?? new Headers() });
            // Fails closed: no session, or not staff/developer, is refused before the handler runs.
            if (!s || !(await canApproveDevice(db, s.user.id))) {
              throw new APIError("FORBIDDEN", {
                message: "Only staff and developers can approve CLI sign-ins",
              });
            }
          }),
        },
      ],
    },
  } satisfies BetterAuthPlugin;

  const auth = betterAuth({
    secret: env.BETTER_AUTH_SECRET,
    baseURL: env.BETTER_AUTH_URL,
    database: drizzleAdapter(db, { provider: "pg", schema }),
    advanced: { useSecureCookies: env.NODE_ENV === "production" },
    emailAndPassword: { enabled: true, minPasswordLength: 12, requireEmailVerification: true },
    emailVerification: {
      sendOnSignUp: true,
      sendVerificationEmail: async ({ user: u, url }) => {
        await sendMail({
          to: u.email,
          subject: "Verify your Porter email",
          text: `Verify your email: ${url}`,
        });
      },
    },
    socialProviders: google.clientId
      ? {
          google: {
            ...google,
            clientId: google.clientId,
            clientSecret: google.clientSecret ?? "",
            // Google enforces the Workspace domain (hd claim); we check the email itself too.
            // Keep `domain` lowercased: Google emits hd in lowercase and the comparison is exact.
            hd: domain,
            mapProfileToUser: (p) => {
              if (p.email_verified !== true || !isStaffEmail(p.email, domain)) {
                throw new APIError("FORBIDDEN", { message: "Google sign-in is for staff accounts only" });
              }
              return {};
            },
          },
        }
      : {},
    hooks: {
      // Password sign-up is for staff: only emails with a pending invitation to the staff org.
      before: createAuthMiddleware(async (ctx) => {
        if (ctx.path !== "/sign-up/email") return;
        const email = String(ctx.body?.email ?? "");
        const [inv] = await db
          .select({ id: invitation.id })
          .from(invitation)
          .innerJoin(org, eq(org.id, invitation.organizationId))
          .where(
            and(
              eq(org.slug, staffOrgSlug()),
              lower(invitation.email, email),
              eq(invitation.status, "pending"),
              gt(invitation.expiresAt, new Date()),
            ),
          )
          .limit(1);
        if (!inv) throw new APIError("FORBIDDEN", { message: "Sign-up is by staff invitation only" });
      }),
    },
    databaseHooks: {
      user: {
        create: {
          // A magic link only ever creates a user who still has a pending project invitation.
          before: async (u, ctx) => {
            if (ctx?.path !== "/magic-link/verify") return;
            const [inv] = await db
              .select({ id: projectInvitation.id })
              .from(projectInvitation)
              .where(pendingProjectInvite(u.email))
              .limit(1);
            if (!inv) return false;
          },
        },
      },
      session: {
        create: {
          after: async (s) => {
            await provisionGoogleStaff(db, s.userId, domain);
            await acceptStaffInvite(db, s.userId);
            await acceptProjectInvites(db, s.userId);
          },
        },
      },
    },
    plugins: [
      // One staff org, created server-side (ensureStaffOrg); nobody creates orgs over HTTP.
      organization({ allowUserToCreateOrganization: false, requireEmailVerificationOnInvitation: true }),
      magicLink({
        sendMagicLink: async ({ email, url }) => {
          // Same response whether or not anyone is invited (no account enumeration); the mail is not
          // awaited so the response time does not tell either.
          if (!(await mayGetMagicLink(db, email))) return;
          sendMail({
            to: email,
            subject: "Sign in to Porter",
            text: `Sign in: ${url}\n\nThe link expires in 5 minutes.`,
          }).catch((e) => console.error("magic link mail failed:", e));
        },
      }),
      deviceAuthorization({ validateClient: (id) => id === CLI_CLIENT_ID, verificationUri: "/device" }),
      bearer(),
      staffDeviceApproval,
    ],
  });
  return auth;
}

let instance: ReturnType<typeof createAuth> | undefined;
/** The app's auth, built from env on first use. */
export const getAuth = () => {
  instance ??= createAuth();
  return instance;
};
