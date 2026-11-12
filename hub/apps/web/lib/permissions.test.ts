import { connect, member, organization, project, projectMember, user } from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { type Action, can, type Role, roleCan } from "./permissions";

const ALL: Action[] = [
  "view",
  "view_raw",
  "push",
  "pull",
  "run",
  "share",
  "upload",
  "manage_members",
  "manage_secrets",
];
const matrix: Record<Role, Action[]> = {
  owner: ALL,
  developer: ["view", "view_raw", "push", "pull", "run", "share", "upload", "manage_secrets"],
  viewer: ["view", "view_raw", "pull"],
  client: ["view", "upload"],
};

describe("role x action matrix", () => {
  for (const [role, allowed] of Object.entries(matrix) as [Role, Action[]][]) {
    test(role, () => {
      for (const a of ALL) expect(roleCan(role, a), `${role} ${a}`).toBe(allowed.includes(a));
    });
  }
  test("non-members can do nothing", () => {
    for (const a of ALL) expect(roleCan(null, a), a).toBe(false);
  });
});

describe("can() against the database", () => {
  let fresh: Awaited<ReturnType<typeof freshDb>>;
  let conn: ReturnType<typeof connect>;
  let projectId: string;
  beforeAll(async () => {
    fresh = await freshDb();
    await runMigrations(fresh.url);
    conn = connect(fresh.url);
    const now = new Date();
    for (const id of ["dev", "staff-only"]) {
      await conn.db
        .insert(user)
        .values({ id, name: id, email: `${id}@x.test`, emailVerified: true, createdAt: now, updatedAt: now });
    }
    await conn.db.insert(organization).values({ id: "umi-org", name: "Umi", slug: "umi", createdAt: now });
    await conn.db
      .insert(member)
      .values({ id: "m1", organizationId: "umi-org", userId: "staff-only", role: "member", createdAt: now });
    const [p] = await conn.db.insert(project).values({ orgId: "umi-org", name: "P", slug: "p" }).returning();
    projectId = p.id;
    await conn.db.insert(projectMember).values({ projectId, userId: "dev", role: "developer" });
  });
  afterAll(async () => {
    await conn?.client.end();
    await fresh?.drop();
  });

  test("uses the project role", async () => {
    expect(await can(conn.db, { id: "dev" }, { id: projectId }, "push")).toBe(true);
    expect(await can(conn.db, { id: "dev" }, { id: projectId }, "manage_members")).toBe(false);
  });
  test("staff-org membership gives no project access", async () => {
    for (const a of ALL) expect(await can(conn.db, { id: "staff-only" }, { id: projectId }, a)).toBe(false);
  });
});
