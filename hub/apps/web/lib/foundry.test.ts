import { randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  connect,
  ensureStaffOrg,
  member,
  project,
  projectInvitation,
  projectMember,
  projectVersion,
  runSecret,
  serviceKey,
  user,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { mintServiceKey, resolveServiceKey } from "@porter-hub/db/service-keys";
import { freshDb } from "@porter-hub/db/test-helper";
import { decryptSecret } from "@porter-hub/porter/secrets";
import { and, desc, eq } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { foundryStatus, linkFoundry } from "./foundry";
import { HttpError } from "./projects";
import { versionDir } from "./snapshot-reader";

process.env.HUB_SECRET_KEY = randomBytes(32).toString("base64");
process.env.BETTER_AUTH_URL = "https://porter.test";

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let bot: { id: string };
let key: string;
const status = (p: Promise<unknown>) =>
  p.then(
    () => 200,
    (e) => (e instanceof HttpError ? e.status : Promise.reject(e)),
  );
const link = (body: object, as = bot) =>
  linkFoundry(conn.db, as, { ref: "turquoise/main-site", name: "Turquoise", ...body });

beforeAll(async () => {
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  const now = new Date();
  for (const id of ["dev", "outsider"])
    await conn.db
      .insert(user)
      .values({ id, name: id, email: `${id}@umi.test`, emailVerified: true, createdAt: now, updatedAt: now });
  const org = await ensureStaffOrg(conn.db);
  await conn.db
    .insert(member)
    .values({ id: "m-dev", organizationId: org, userId: "dev", role: "member", createdAt: now });
  const minted = await mintServiceKey(conn.db, "Foundry hub");
  key = minted.key;
  bot = { id: minted.userId };
});
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
});

describe("service keys", () => {
  test("a key resolves to its staff bot; revoked, unknown and non-phk keys don't", async () => {
    expect(key).toMatch(/^phk_/);
    expect(await resolveServiceKey(conn.db, key)).toEqual(bot);
    expect(await resolveServiceKey(conn.db, `${key}x`)).toBeNull();
    expect(await resolveServiceKey(conn.db, "not-a-service-key")).toBeNull();
    const other = await mintServiceKey(conn.db, "Old");
    await conn.db
      .update(serviceKey)
      .set({ revokedAt: new Date() })
      .where(eq(serviceKey.userId, other.userId));
    expect(await resolveServiceKey(conn.db, other.key)).toBeNull();
    // Only the hash is stored.
    const rows = await conn.db.select().from(serviceKey);
    expect(rows.some((r) => r.keyHash === key || r.prefix === key)).toBe(false);
  });
});

describe("linking a Foundry site", () => {
  test("only staff may link", async () => {
    expect(await status(link({}, { id: "outsider" }))).toBe(403);
    expect(await status(link({ ref: "Not A Ref" }))).toBe(400);
  });

  test("creates the project, adds members, invites strangers and sets the destination", async () => {
    const r = await link({
      foundry_url: "https://foundry.test/sites/turquoise/main-site",
      members: [
        { email: "dev@umi.test", role: "owner" },
        { email: "new.person@umi.test", role: "developer" },
      ],
      destination: { url: "https://www.turquoise.test", api_key: "payload-key-123" },
    });
    expect(r).toMatchObject({ slug: "turquoise-main-site", created: true, version: 1 });
    expect(r.url).toBe("https://porter.test/p/turquoise-main-site");

    const [p] = await conn.db.select().from(project).where(eq(project.slug, r.slug));
    expect(p.foundryRef).toBe("turquoise/main-site");
    expect(p.foundryUrl).toBe("https://foundry.test/sites/turquoise/main-site");
    const members = await conn.db.select().from(projectMember).where(eq(projectMember.projectId, p.id));
    expect(Object.fromEntries(members.map((m) => [m.userId, m.role]))).toEqual({
      [bot.id]: "owner",
      dev: "owner",
    });
    const inv = await conn.db.select().from(projectInvitation).where(eq(projectInvitation.projectId, p.id));
    expect(inv.map((i) => [i.email, i.role])).toEqual([["new.person@umi.test", "developer"]]);

    const [v] = await conn.db
      .select()
      .from(projectVersion)
      .where(eq(projectVersion.projectId, p.id))
      .orderBy(desc(projectVersion.number));
    const toml = readFileSync(join(await versionDir(v), "porter.toml"), "utf8");
    expect(toml).toContain('url = "https://www.turquoise.test"');
    expect(toml).toContain('api_key_env = "PAYLOAD_API_KEY"');
    expect(toml).not.toContain("payload-key-123");
    const [sec] = await conn.db
      .select()
      .from(runSecret)
      .where(and(eq(runSecret.projectId, p.id), eq(runSecret.name, "PAYLOAD_API_KEY")));
    expect(decryptSecret(p.id, sec.name, sec)).toBe("payload-key-123");
  });

  test("linking again finds the same project; members already in stay as they are", async () => {
    const r = await link({ members: [{ email: "dev@umi.test", role: "viewer" }] });
    expect(r).toMatchObject({ slug: "turquoise-main-site", created: false, version: null });
    const [m] = await conn.db.select().from(projectMember).where(eq(projectMember.userId, "dev"));
    expect(m.role).toBe("owner");
  });

  test("a slug already taken by a hand-made project gets a suffix", async () => {
    await conn.db
      .insert(project)
      .values({ orgId: await ensureStaffOrg(conn.db), name: "Acme", slug: "acme" });
    const r = await link({ ref: "acme/acme", name: "Acme" });
    expect(r.slug).toBe("acme-2");
  });
});

describe("status for Foundry's panel", () => {
  test("unlinked, then linked without runs; non-members see nothing", async () => {
    expect(await foundryStatus(conn.db, bot, "nobody/here")).toEqual({ linked: false });
    expect(await status(foundryStatus(conn.db, bot, "../etc"))).toBe(400);
    const s = await foundryStatus(conn.db, bot, "turquoise/main-site");
    expect(s).toMatchObject({
      linked: true,
      slug: "turquoise-main-site",
      destination_host: "www.turquoise.test",
      version: { number: 1 },
      stages: null, // never run: no project.db yet
      issues: null,
      dry_run: false,
      ever_written: false,
      last_run: null,
    });
    expect(await status(foundryStatus(conn.db, { id: "outsider" }, "turquoise/main-site"))).toBe(404);
  });
});
