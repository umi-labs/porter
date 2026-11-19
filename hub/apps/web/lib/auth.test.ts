import {
  auditLog,
  connect,
  invitation,
  member,
  organization,
  project,
  projectInvitation,
  projectMember,
  user,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { and, eq, sql } from "drizzle-orm";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test, vi } from "vitest";
import { createAuth, type Mail } from "./auth";

const DOMAIN = "umidigital.co.uk";
let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let auth: ReturnType<typeof createAuth>;
const sent: Mail[] = [];
let projectId: string;
/** A non-staff org (the project's client company); membership here must not make anyone staff. */
let otherOrgId: string;

const baseEnv = () => {
  vi.stubEnv("STAFF_EMAIL_DOMAIN", DOMAIN);
  vi.stubEnv("BETTER_AUTH_SECRET", "test-secret-test-secret-test-secret-0123");
  vi.stubEnv("BETTER_AUTH_URL", "http://localhost:3000");
};
beforeEach(baseEnv);
afterEach(() => {
  vi.unstubAllEnvs();
});

beforeAll(async () => {
  baseEnv();
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  auth = createAuth({
    db: conn.db,
    sendMail: async (m) => {
      sent.push(m);
    },
    // No real Google in tests: accept our unsigned id tokens; the profile still goes through the real
    // Google provider's decode + our staff-domain check.
    google: { clientId: "test", clientSecret: "test", verifyIdToken: async () => true },
  });
  const [o] = await conn.db
    .insert(organization)
    .values({ id: "acme-org", name: "Acme", slug: "acme-org", createdAt: new Date() })
    .returning();
  otherOrgId = o.id;
  const [p] = await conn.db
    .insert(project)
    .values({ orgId: otherOrgId, name: "Acme", slug: "acme" })
    .returning();
  projectId = p.id;
});

afterAll(async () => {
  vi.unstubAllEnvs();
  await conn?.client.end();
  await fresh?.drop();
});

const b64 = (o: object) => Buffer.from(JSON.stringify(o)).toString("base64url");
const idToken = (claims: Record<string, unknown>) =>
  `${b64({ alg: "RS256", typ: "JWT" })}.${b64({ iss: "https://accounts.google.com", aud: "test", ...claims })}.sig`;

function googleSignIn(email: string, extra: Record<string, unknown> = {}) {
  const token = idToken({
    sub: `g-${email}`,
    email,
    email_verified: true,
    name: "Staff",
    hd: DOMAIN,
    ...extra,
  });
  return auth.api.signInSocial({ body: { provider: "google", idToken: { token } }, headers: new Headers() });
}

const userByEmail = async (email: string) =>
  (await conn.db.select().from(user).where(eq(user.email, email.toLowerCase())))[0];

const bearer = (token: string) => new Headers({ authorization: `Bearer ${token}` });

const staffOrgMembers = async (email: string) => {
  const u = await userByEmail(email);
  if (!u) return [];
  return conn.db
    .select({ role: member.role })
    .from(member)
    .innerJoin(organization, eq(organization.id, member.organizationId))
    .where(and(eq(member.userId, u.id), eq(organization.slug, "umi")));
};

/** Invites `email` to the project and signs in via its magic link; returns the session token. */
async function clientSession(email: string) {
  sent.length = 0;
  await conn.db
    .insert(projectInvitation)
    .values({ projectId, email, expiresAt: new Date(Date.now() + 86_400_000) });
  await auth.api.signInMagicLink({ body: { email }, headers: new Headers() });
  const token = new URL(sent[0].text.match(/https?:\/\/\S+/)?.[0] ?? "").searchParams.get("token") ?? "";
  const res = (await auth.api.magicLinkVerify({ query: { token }, headers: new Headers() })) as {
    token: string;
  };
  return res.token;
}

describe("google", () => {
  test("google sign-in outside staff domain is rejected", async () => {
    const ok = await googleSignIn(`isaac@${DOMAIN}`);
    expect(ok).toMatchObject({ redirect: false, token: expect.any(String) });
    expect(await userByEmail(`isaac@${DOMAIN}`)).toBeDefined();

    // Case-insensitive on the domain.
    await expect(googleSignIn(`Mixed@UMIDIGITAL.CO.UK`)).resolves.toMatchObject({ redirect: false });

    const rejected: [string, Record<string, unknown>][] = [
      ["someone@gmail.com", { hd: undefined }],
      // Lookalike emails, even if the hd claim were right, and lookalike hd claims.
      [`evil@evil-${DOMAIN}`, {}],
      [`evil@x.${DOMAIN}`, {}],
      [`evil@${DOMAIN}.evil.com`, {}],
      [`evil@${DOMAIN}@gmail.com`, {}],
      [`evil2@evil-${DOMAIN}`, { hd: `evil-${DOMAIN}` }],
      [`evil2@x.${DOMAIN}`, { hd: `x.${DOMAIN}` }],
      // Right domain on the email but no Workspace hd claim (consumer account with that address).
      [`nohd@${DOMAIN}`, { hd: undefined }],
      // Right domain but Google has not verified the address.
      [`unverified@${DOMAIN}`, { email_verified: false }],
    ];
    for (const [email, extra] of rejected) {
      await expect(googleSignIn(email, extra), email).rejects.toThrow();
      expect(await userByEmail(email), email).toBeUndefined();
    }
  });

  test("a returning Google user is re-checked on every sign-in", async () => {
    await googleSignIn(`back@${DOMAIN}`);
    // Same Google account (sub), but Google now says the address is unverified / outside the domain.
    await expect(googleSignIn(`back@${DOMAIN}`, { email_verified: false })).rejects.toThrow();
    await expect(
      googleSignIn(`back@gmail.com`, { sub: `g-back@${DOMAIN}`, hd: undefined }),
    ).rejects.toThrow();
  });

  test("a Google staff user is auto-added to the staff org", async () => {
    expect(await staffOrgMembers(`isaac@${DOMAIN}`)).toEqual([{ role: "member" }]);
    // Signing in again does not add a second membership.
    await googleSignIn(`isaac@${DOMAIN}`);
    expect(await staffOrgMembers(`isaac@${DOMAIN}`)).toEqual([{ role: "member" }]);
    const orgs = await conn.db.select().from(organization).where(eq(organization.slug, "umi"));
    expect(orgs).toHaveLength(1);
  });

  test("a lookalike domain is not added to the staff org", async () => {
    await expect(googleSignIn(`evil3@evil-${DOMAIN}`)).rejects.toThrow();
    expect(await staffOrgMembers(`evil3@evil-${DOMAIN}`)).toEqual([]);
    // A verified non-staff user (a client) never becomes a staff-org member either.
    await clientSession("look@acme.example");
    expect(await staffOrgMembers("look@acme.example")).toEqual([]);
  });

  test("a client cannot create an org", async () => {
    const token = await clientSession("orgmaker@acme.example");
    await expect(
      auth.api.createOrganization({ body: { name: "Mine", slug: "mine" }, headers: bearer(token) }),
    ).rejects.toMatchObject({ statusCode: 403 });
    expect(await conn.db.select().from(organization).where(eq(organization.slug, "mine"))).toEqual([]);
  });
});

describe("magic link", () => {
  const verify = (url: string) => {
    const token = new URL(url).searchParams.get("token") ?? "";
    return auth.api.magicLinkVerify({ query: { token }, headers: new Headers() });
  };
  const ask = (email: string) => auth.api.signInMagicLink({ body: { email }, headers: new Headers() });

  test("magic link creates client user only via invitation", async () => {
    // Uninvited: identical response, nothing sent, no user.
    sent.length = 0;
    expect(await ask("stranger@acme.example")).toEqual({ status: true });
    expect(sent).toHaveLength(0);
    expect(await userByEmail("stranger@acme.example")).toBeUndefined();

    // Invited: link is sent; following it creates the user and grants the invited project role.
    await conn.db.insert(projectInvitation).values({
      projectId,
      email: "client@acme.example",
      role: "client",
      expiresAt: new Date(Date.now() + 86_400_000),
    });
    expect(await ask("Client@Acme.example")).toEqual({ status: true });
    expect(sent).toHaveLength(1);
    expect(sent[0].to).toBe("Client@Acme.example");
    const link = sent[0].text.match(/https?:\/\/\S+/)?.[0] ?? "";
    expect(link).toContain("/api/auth/magic-link/verify?token=");
    const res = await verify(link);
    expect(res).toMatchObject({ token: expect.any(String) });

    const u = await userByEmail("client@acme.example");
    expect(u).toBeDefined();
    const roles = await conn.db.select().from(projectMember).where(eq(projectMember.userId, u.id));
    expect(roles).toEqual([expect.objectContaining({ projectId, role: "client" })]);
    const [inv] = await conn.db
      .select()
      .from(projectInvitation)
      .where(eq(projectInvitation.email, "client@acme.example"));
    expect(inv.acceptedAt).not.toBeNull();
    // Accepting is audited, by the invitee, against the invitation.
    const log = await conn.db.select().from(auditLog).where(eq(auditLog.targetId, inv.id));
    expect(log).toEqual([
      expect.objectContaining({
        action: "invitation.accept",
        projectId,
        actorId: u.id,
        detail: { role: "client", alreadyMember: false },
      }),
    ]);

    // An existing client user can ask again.
    expect(await ask("client@acme.example")).toEqual({ status: true });
    expect(sent).toHaveLength(2);

    // Expired invitations do not count.
    await conn.db.insert(projectInvitation).values({
      projectId,
      email: "expired@acme.example",
      expiresAt: new Date(Date.now() - 1000),
    });
    expect(await ask("expired@acme.example")).toEqual({ status: true });
    expect(sent).toHaveLength(2);
  });

  test("a link whose invitation was revoked before use creates no user", async () => {
    sent.length = 0;
    const [inv] = await conn.db
      .insert(projectInvitation)
      .values({ projectId, email: "late@acme.example", expiresAt: new Date(Date.now() + 86_400_000) })
      .returning();
    await ask("late@acme.example");
    expect(sent).toHaveLength(1);
    await conn.db.delete(projectInvitation).where(eq(projectInvitation.id, inv.id));
    await expect(verify(sent[0].text.match(/https?:\/\/\S+/)?.[0] ?? "")).rejects.toThrow();
    expect(await userByEmail("late@acme.example")).toBeUndefined();
  });
});

describe("device authorization", () => {
  const grant = "urn:ietf:params:oauth:grant-type:device_code";
  const poll = (device_code: string) =>
    auth.api.deviceToken({ body: { grant_type: grant, device_code, client_id: "porter-cli" } });

  test("device flow issues a token after approval", async () => {
    await expect(auth.api.deviceCode({ body: { client_id: "someone-else" } })).rejects.toThrow();

    const code = await auth.api.deviceCode({ body: { client_id: "porter-cli" } });
    expect(code.verification_uri).toBe("http://localhost:3000/device");
    await expect(poll(code.device_code)).rejects.toMatchObject({
      body: expect.objectContaining({ error: "authorization_pending" }),
    });

    // A Google staff session (auto-added to the staff org) claims and approves the code.
    const staff = await googleSignIn(`dev@${DOMAIN}`);
    const staffToken = (staff as { token: string }).token;
    await auth.api.deviceVerify({ query: { user_code: code.user_code }, headers: bearer(staffToken) });
    await expect(
      auth.api.deviceApprove({ body: { userCode: code.user_code }, headers: bearer(staffToken) }),
    ).resolves.toEqual({ success: true });

    // The CLI waited its polling interval.
    await conn.db.execute(sql`update device_code set last_polled_at = now() - interval '1 minute'`);
    const tok = await poll(code.device_code);
    expect(tok).toMatchObject({ token_type: "Bearer", access_token: expect.any(String) });

    // The token works as a bearer token on the API.
    const s = await auth.api.getSession({ headers: bearer(tok.access_token) });
    expect(s?.user.email).toBe(`dev@${DOMAIN}`);
  });

  test("a client cannot approve a device code", async () => {
    const clientToken = await clientSession("client2@acme.example");

    const code = await auth.api.deviceCode({ body: { client_id: "porter-cli" } });
    await auth.api.deviceVerify({ query: { user_code: code.user_code }, headers: bearer(clientToken) });
    await expect(
      auth.api.deviceApprove({ body: { userCode: code.user_code }, headers: bearer(clientToken) }),
    ).rejects.toMatchObject({ statusCode: 403 });
    await conn.db.execute(sql`update device_code set last_polled_at = null`);
    await expect(poll(code.device_code)).rejects.toMatchObject({
      body: expect.objectContaining({ error: "authorization_pending" }),
    });
  });

  test("a member of a non-staff org cannot approve a device code", async () => {
    const token = await clientSession("acme-admin@acme.example");
    const u = await userByEmail("acme-admin@acme.example");
    await conn.db.insert(member).values({
      id: "m-acme",
      organizationId: otherOrgId,
      userId: u.id,
      role: "owner",
      createdAt: new Date(),
    });
    const code = await auth.api.deviceCode({ body: { client_id: "porter-cli" } });
    await auth.api.deviceVerify({ query: { user_code: code.user_code }, headers: bearer(token) });
    await expect(
      auth.api.deviceApprove({ body: { userCode: code.user_code }, headers: bearer(token) }),
    ).rejects.toMatchObject({ statusCode: 403 });
  });

  test("the approval hook rejects requests with no session", async () => {
    const code = await auth.api.deviceCode({ body: { client_id: "porter-cli" } });
    await expect(
      auth.api.deviceApprove({ body: { userCode: code.user_code }, headers: new Headers() }),
    ).rejects.toMatchObject({ statusCode: 403 });
    await expect(
      auth.api.deviceApprove({ body: { userCode: code.user_code }, headers: bearer("not-a-session") }),
    ).rejects.toMatchObject({ statusCode: 403 });
  });
});

describe("email and password", () => {
  const signUp = (email: string, password: string, invitationId?: string) =>
    auth.api.signUpEmail({
      body: { email, password, name: "Staff", ...(invitationId ? { invitationId } : {}) } as never,
      headers: new Headers(),
    });
  const signIn = (email: string) =>
    auth.api.signInEmail({ body: { email, password: "long-enough-password" }, headers: new Headers() });
  const invite = async (id: string, orgId: string, email: string) => {
    const inviter = await userByEmail(`isaac@${DOMAIN}`);
    await conn.db.execute(
      sql`insert into invitation (id, organization_id, email, role, status, expires_at, inviter_id)
          values (${id}, ${orgId}, ${email}, 'member', 'pending', now() + interval '1 day', ${inviter.id})`,
    );
  };
  const staffOrgId = async () =>
    (await conn.db.select().from(organization).where(eq(organization.slug, "umi")))[0].id;
  const statusOf = async (id: string) =>
    (await conn.db.select().from(invitation).where(eq(invitation.id, id)))[0].status;

  test("sign-up needs the id of a pending staff invitation for that email, and a 12+ char password", async () => {
    await expect(signUp("walkin@example.com", "long-enough-password")).rejects.toMatchObject({
      statusCode: 403,
    });
    expect(await userByEmail("walkin@example.com")).toBeUndefined();

    // An invitation to some other org does not open password sign-up, even with its id.
    await invite("inv0", otherOrgId, "sidedoor@example.com");
    await expect(signUp("sidedoor@example.com", "long-enough-password", "inv0")).rejects.toMatchObject({
      statusCode: 403,
    });

    await invite("inv1", await staffOrgId(), "contractor@example.com");
    // Knowing the invited address is not enough: a squatter without the id is refused.
    await expect(signUp("contractor@example.com", "long-enough-password")).rejects.toMatchObject({
      statusCode: 403,
    });
    await expect(signUp("contractor@example.com", "long-enough-password", "wrong-id")).rejects.toMatchObject({
      statusCode: 403,
    });
    expect(await userByEmail("contractor@example.com")).toBeUndefined();
    await expect(signUp("contractor@example.com", "short-pw", "inv1")).rejects.toThrow();
    await expect(signUp("contractor@example.com", "long-enough-password", "inv1")).resolves.toBeDefined();
    expect(sent.some((m) => m.to === "contractor@example.com")).toBe(true); // verification email
    expect(await statusOf("inv1")).toBe("signed-up");

    // Unverified: signing in is refused and makes no member. Verified: sign-in accepts that staff invitation.
    await expect(signIn("contractor@example.com")).rejects.toMatchObject({ statusCode: 403 });
    await conn.db.update(user).set({ emailVerified: true }).where(eq(user.email, "contractor@example.com"));
    await signIn("contractor@example.com");
    const contractor = await userByEmail("contractor@example.com");
    const memberships = await conn.db.select().from(member).where(eq(member.userId, contractor.id));
    expect(memberships.map((m) => m.organizationId)).toEqual([await staffOrgId()]);
    expect(await statusOf("inv1")).toBe("accepted");
    expect(await statusOf("inv0")).toBe("pending"); // the other org's invitation was not touched
  });

  test("a squatter holding an old invitation id for a different email is refused", async () => {
    await invite("inv-old", await staffOrgId(), "former@example.com");
    await invite("inv-target", await staffOrgId(), "boss@example.com");
    await expect(signUp("boss@example.com", "long-enough-password", "inv-old")).rejects.toMatchObject({
      statusCode: 403,
    });
    expect(await userByEmail("boss@example.com")).toBeUndefined();
    expect(await statusOf("inv-old")).toBe("pending");
    expect(await statusOf("inv-target")).toBe("pending");
  });

  test("sign-in accepts only the invitation the account signed up with, not another pending one", async () => {
    await invite("inv-a", await staffOrgId(), "pair@example.com");
    await signUp("pair@example.com", "long-enough-password", "inv-a");
    // An admin re-invites meanwhile: that pending invitation was never used to sign up.
    await invite("inv-b", await staffOrgId(), "pair@example.com");
    await conn.db.update(invitation).set({ status: "canceled" }).where(eq(invitation.id, "inv-a"));
    await conn.db.update(user).set({ emailVerified: true }).where(eq(user.email, "pair@example.com"));
    await signIn("pair@example.com");
    const u = await userByEmail("pair@example.com");
    expect(await conn.db.select().from(member).where(eq(member.userId, u.id))).toEqual([]);
    expect(await statusOf("inv-b")).toBe("pending");
  });
});

describe("environment", () => {
  // Env stubs are reset by beforeEach(baseEnv), so a failing assertion can't leak NODE_ENV=production.
  test("refuses to start in production without BETTER_AUTH_URL and BETTER_AUTH_SECRET", () => {
    vi.stubEnv("NODE_ENV", "production");
    vi.stubEnv("BETTER_AUTH_SECRET", "");
    expect(() => createAuth({ db: conn.db })).toThrow(/BETTER_AUTH_SECRET/);
    vi.stubEnv("BETTER_AUTH_SECRET", "too-short");
    expect(() => createAuth({ db: conn.db })).toThrow(/at least 32/);
    vi.stubEnv("BETTER_AUTH_SECRET", "x".repeat(40));
    vi.stubEnv("BETTER_AUTH_URL", "");
    expect(() => createAuth({ db: conn.db })).toThrow(/BETTER_AUTH_URL/);
  });
});
