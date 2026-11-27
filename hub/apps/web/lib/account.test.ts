import { account, connect, user } from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { afterAll, beforeAll, expect, test } from "vitest";
import { signInWays } from "./account";

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;

beforeAll(async () => {
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  const now = new Date();
  await conn.db
    .insert(user)
    .values({ id: "u", name: "U", email: "u@umi.test", emailVerified: true, createdAt: now, updatedAt: now });
  await conn.db.insert(account).values([
    { id: "a1", accountId: "u", providerId: "credential", userId: "u", createdAt: now, updatedAt: now },
    {
      id: "a2",
      accountId: "g-1",
      providerId: "google",
      userId: "u",
      createdAt: new Date(+now + 1),
      updatedAt: now,
    },
  ]);
});
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
});

test("linked accounts first, then switched-on providers not linked yet", async () => {
  const env = {
    GOOGLE_CLIENT_ID: "g",
    FOUNDRY_OIDC_ISSUER: "https://f.test/api/auth",
    FOUNDRY_OIDC_CLIENT_ID: "porter",
    FOUNDRY_OIDC_CLIENT_SECRET: "s",
  };
  const ways = await signInWays(conn.db, "u", env);
  expect(ways.map((w) => [w.label, !!w.linkedAt])).toEqual([
    ["Email and password", true],
    ["Google", true],
    ["Foundry hub", false],
  ]);
  // Foundry not configured: not offered.
  expect((await signInWays(conn.db, "u", { GOOGLE_CLIENT_ID: "g" })).map((w) => w.id)).toEqual([
    "credential",
    "google",
  ]);
});
