import { connect, invitation, organization } from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { eq } from "drizzle-orm";
import { afterAll, beforeAll, expect, test } from "vitest";
import { inviteStaff } from "./invite-staff";

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
beforeAll(async () => {
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
});
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
});

test("invite-staff on an empty hub: creates the staff org and one pending invitation; a re-run replaces it", async () => {
  const a = await inviteStaff(conn.db, "First@Example.com");
  const b = await inviteStaff(conn.db, "first@example.com");
  const [org] = await conn.db.select().from(organization).where(eq(organization.slug, "umi"));
  expect(b.organizationId).toBe(org.id);
  const rows = await conn.db.select().from(invitation);
  const status = Object.fromEntries(rows.map((r) => [r.id, r.status]));
  expect(status).toEqual({ [a.id]: "canceled", [b.id]: "pending" });
  expect(b.email).toBe("first@example.com");
  expect(b.expiresAt.getTime()).toBeGreaterThan(Date.now());
});
