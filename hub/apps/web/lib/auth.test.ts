import {
  connect,
  member,
  organization,
  project,
  projectInvitation,
  projectMember,
  user,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { eq, sql } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test, vi } from "vitest";
import { createAuth, type Mail } from "./auth";

const DOMAIN = "umidigital.co.uk";
let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let auth: ReturnType<typeof createAuth>;
const sent: Mail[] = [];
let projectId: string;
let orgId: string;

beforeAll(async () => {
  vi.stubEnv("STAFF_EMAIL_DOMAIN", DOMAIN);
  vi.stubEnv("BETTER_AUTH_SECRET", "test-secret-test-secret-test-secret-0123");
  vi.stubEnv("BETTER_AUTH_URL", "http://localhost:3000");
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
    .values({ id: "umi", name: "Umi", slug: "umi", createdAt: new Date() })
    .returning();
  orgId = o.id;
  const [p] = await conn.db.insert(project).values({ orgId, name: "Acme", slug: "acme" }).returning();
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

    // A staff session (org member) claims and approves the code.
    const staff = await googleSignIn(`dev@${DOMAIN}`);
    const staffToken = (staff as { token: string }).token;
    const staffUser = await userByEmail(`dev@${DOMAIN}`);
    await conn.db
      .insert(member)
      .values({ id: "m-dev", organizationId: orgId, userId: staffUser.id, createdAt: new Date() });
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
    sent.length = 0;
    await conn.db.insert(projectInvitation).values({
      projectId,
      email: "client2@acme.example",
      expiresAt: new Date(Date.now() + 86_400_000),
    });
    await auth.api.signInMagicLink({ body: { email: "client2@acme.example" }, headers: new Headers() });
    const token = new URL(sent[0].text.match(/https?:\/\/\S+/)?.[0] ?? "").searchParams.get("token") ?? "";
    const { token: clientToken } = (await auth.api.magicLinkVerify({
      query: { token },
      headers: new Headers(),
    })) as { token: string };

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
});

describe("email and password", () => {
  const signUp = (email: string, password: string) =>
    auth.api.signUpEmail({ body: { email, password, name: "Staff" }, headers: new Headers() });

  test("sign-up needs a pending staff invitation and a 12+ char password", async () => {
    await expect(signUp("walkin@example.com", "long-enough-password")).rejects.toMatchObject({
      statusCode: 403,
    });
    expect(await userByEmail("walkin@example.com")).toBeUndefined();

    const inviter = await userByEmail(`isaac@${DOMAIN}`);
    await conn.db.execute(
      sql`insert into invitation (id, organization_id, email, role, status, expires_at, inviter_id)
          values ('inv1', ${orgId}, 'contractor@example.com', 'member', 'pending', now() + interval '1 day', ${inviter.id})`,
    );
    await expect(signUp("contractor@example.com", "short-pw")).rejects.toThrow();
    await expect(signUp("contractor@example.com", "long-enough-password")).resolves.toBeDefined();
    expect(sent.some((m) => m.to === "contractor@example.com")).toBe(true); // verification email
  });
});

describe("environment", () => {
  test("refuses to start in production without BETTER_AUTH_URL and BETTER_AUTH_SECRET", () => {
    vi.stubEnv("NODE_ENV", "production");
    vi.stubEnv("BETTER_AUTH_SECRET", "");
    expect(() => createAuth({ db: conn.db })).toThrow(/BETTER_AUTH_SECRET/);
    vi.stubEnv("BETTER_AUTH_SECRET", "x".repeat(40));
    vi.stubEnv("BETTER_AUTH_URL", "");
    expect(() => createAuth({ db: conn.db })).toThrow(/BETTER_AUTH_URL/);
    vi.stubEnv("NODE_ENV", "test");
    vi.stubEnv("BETTER_AUTH_URL", "http://localhost:3000");
  });
});
