import {
  auditLog,
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
import { and, eq } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { makeApi } from "./api";

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let api: ReturnType<typeof makeApi>;
let pid: string;

const USERS = ["owner", "owner2", "dev", "viewer", "client", "stranger", "staff", "nobody"];
const req = (as: string | null, method = "GET", body?: unknown) =>
  new Request("http://hub.test/x", {
    method,
    headers: { ...(as ? { "x-user": as } : {}), "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
const slug = { slug: "acme" };

beforeAll(async () => {
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  const now = new Date();
  for (const id of USERS) {
    await conn.db
      .insert(user)
      .values({ id, name: id, email: `${id}@x.test`, emailVerified: true, createdAt: now, updatedAt: now });
  }
  await conn.db.insert(organization).values({ id: "umi-org", name: "Umi", slug: "umi", createdAt: now });
  for (const id of ["owner", "owner2", "dev", "viewer", "staff"]) {
    await conn.db
      .insert(member)
      .values({ id: `m-${id}`, organizationId: "umi-org", userId: id, role: "member", createdAt: now });
  }
  const [p] = await conn.db
    .insert(project)
    .values({ orgId: "umi-org", name: "Acme", slug: "acme" })
    .returning();
  pid = p.id;
  await conn.db.insert(projectMember).values([
    { projectId: pid, userId: "owner", role: "owner" },
    { projectId: pid, userId: "dev", role: "developer" },
    { projectId: pid, userId: "viewer", role: "viewer" },
    { projectId: pid, userId: "client", role: "client" },
  ]);
  api = makeApi(conn.db, async (r) => {
    const id = r.headers.get("x-user");
    return id ? { id } : null;
  });
});
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
});

const status = async (p: Promise<Response>) => (await p).status;
const roleOf = async (userId: string) =>
  (
    await conn.db
      .select()
      .from(projectMember)
      .where(and(eq(projectMember.projectId, pid), eq(projectMember.userId, userId)))
  )[0]?.role;
const audited = async (action: string) =>
  (await conn.db.select().from(auditLog).where(eq(auditLog.action, action))).length;

describe("every per-project route", () => {
  // [name, call]. A non-member gets 404 (never 403) so the project's existence doesn't leak; a signed-out
  // caller gets 401.
  const routes: [string, (as: string | null) => Promise<Response>][] = [
    ["get project", (a) => api.getProject(req(a), slug)],
    ["list members", (a) => api.listMembers(req(a), slug)],
    ["add member", (a) => api.addMember(req(a, "POST", { email: "nobody@x.test", role: "viewer" }), slug)],
    ["set role", (a) => api.updateMember(req(a, "PATCH", { role: "viewer" }), { ...slug, userId: "dev" })],
    ["remove member", (a) => api.removeMember(req(a, "DELETE"), { ...slug, userId: "dev" })],
    ["list invitations", (a) => api.listInvitations(req(a), slug)],
    ["invite", (a) => api.createInvitation(req(a, "POST", { email: "c@x.test" }), slug)],
    [
      "revoke invitation",
      (a) => api.revokeInvitation(req(a, "DELETE"), { ...slug, id: crypto.randomUUID() }),
    ],
  ];
  for (const [name, call] of routes) {
    test(`${name}: signed out is 401, non-member 404 (also for staff with no project role)`, async () => {
      expect(await status(call(null))).toBe(401);
      expect(await status(call("stranger"))).toBe(404);
      expect(await status(call("staff"))).toBe(404);
    });
    test(`${name}: unknown project is 404`, async () => {
      const r = name.includes("project")
        ? api.getProject(req("owner"), { slug: "nope" })
        : api.listMembers(req("owner"), { slug: "nope" });
      expect(await status(r)).toBe(404);
    });
  }

  test("members without manage_members get 403 on member and invitation routes", async () => {
    for (const [name, call] of routes.slice(1)) {
      for (const as of ["dev", "viewer", "client"]) expect(await status(call(as)), `${name} ${as}`).toBe(403);
    }
  });
  test("any member can view the project", async () => {
    for (const as of ["owner", "dev", "viewer", "client"]) {
      expect(await status(api.getProject(req(as), slug))).toBe(200);
    }
  });
  test("the 404 for a non-member is identical for a missing and a real project", async () => {
    const real = await api.getProject(req("stranger"), slug);
    const missing = await api.getProject(req("stranger"), { slug: "nope" });
    expect(await real.json()).toEqual(await missing.json());
  });
});

describe("projects", () => {
  test("list shows only the caller's projects", async () => {
    expect(await (await api.listProjects(req("dev"))).json()).toMatchObject([
      { slug: "acme", role: "developer" },
    ]);
    expect(await (await api.listProjects(req("stranger"))).json()).toEqual([]);
    expect(await status(api.listProjects(req(null)))).toBe(401);
  });
  test("staff create a project and become its owner; audited", async () => {
    const res = await api.createProject(
      req("staff", "POST", { name: "Brand New", summary: "WP -> Payload" }),
    );
    expect(res.status).toBe(201);
    const body = await res.json();
    expect(body).toMatchObject({ slug: "brand-new", role: "owner" });
    expect(await status(api.getProject(req("staff"), { slug: "brand-new" }))).toBe(200);
    expect(await audited("project.create")).toBe(1);
  });
  test("duplicate slug is 409, bad input is 400", async () => {
    expect(await status(api.createProject(req("staff", "POST", { name: "Acme" })))).toBe(409);
    expect(await status(api.createProject(req("staff", "POST", { name: "" })))).toBe(400);
    expect(await status(api.createProject(req("staff", "POST", { name: "x", slug: "Bad Slug!" })))).toBe(400);
  });
  test("clients and non-staff cannot create (403)", async () => {
    for (const as of ["client", "stranger", "dev-not-staff"]) {
      expect(await status(api.createProject(req(as, "POST", { name: "Nope" }))), as).toBe(403);
    }
    expect(await status(api.createProject(req(null, "POST", { name: "Nope" })))).toBe(401);
  });
});

describe("members", () => {
  test("owner adds, changes, removes; every change is audited", async () => {
    const add = await api.addMember(req("owner", "POST", { email: "Nobody@X.test", role: "viewer" }), slug);
    expect(add.status).toBe(201);
    expect(await roleOf("nobody")).toBe("viewer");
    expect(
      await status(api.addMember(req("owner", "POST", { email: "nobody@x.test", role: "viewer" }), slug)),
    ).toBe(409);
    expect(
      await status(api.addMember(req("owner", "POST", { email: "ghost@x.test", role: "viewer" }), slug)),
    ).toBe(404);
    expect(await status(api.addMember(req("owner", "POST", { email: "x", role: "viewer" }), slug))).toBe(400);
    expect(await status(api.addMember(req("owner", "POST", { email: "a@x.test", role: "root" }), slug))).toBe(
      400,
    );

    expect(
      await status(
        api.updateMember(req("owner", "PATCH", { role: "developer" }), { ...slug, userId: "nobody" }),
      ),
    ).toBe(200);
    expect(await roleOf("nobody")).toBe("developer");
    expect(
      await status(api.updateMember(req("owner", "PATCH", { role: "viewer" }), { ...slug, userId: "ghost" })),
    ).toBe(404);

    expect(await status(api.removeMember(req("owner", "DELETE"), { ...slug, userId: "nobody" }))).toBe(200);
    expect(await roleOf("nobody")).toBeUndefined();
    expect(await audited("member.add")).toBe(1);
    expect(await audited("member.role")).toBe(1);
    expect(await audited("member.remove")).toBe(1);
    const list = await (await api.listMembers(req("owner"), slug)).json();
    expect(list.map((m: { userId: string }) => m.userId).sort()).toEqual([
      "client",
      "dev",
      "owner",
      "viewer",
    ]);
  });
  test("the last owner cannot be removed or demoted; with two owners one can go", async () => {
    expect(await status(api.removeMember(req("owner", "DELETE"), { ...slug, userId: "owner" }))).toBe(409);
    expect(
      await status(
        api.updateMember(req("owner", "PATCH", { role: "developer" }), { ...slug, userId: "owner" }),
      ),
    ).toBe(409);
    expect(await roleOf("owner")).toBe("owner");

    await api.addMember(req("owner", "POST", { email: "owner2@x.test", role: "owner" }), slug);
    expect(
      await status(
        api.updateMember(req("owner2", "PATCH", { role: "viewer" }), { ...slug, userId: "owner" }),
      ),
    ).toBe(200);
    // owner2 is now the only owner.
    expect(await status(api.removeMember(req("owner2", "DELETE"), { ...slug, userId: "owner2" }))).toBe(409);
    await conn.db
      .update(projectMember)
      .set({ role: "owner" })
      .where(and(eq(projectMember.projectId, pid), eq(projectMember.userId, "owner")));
    await conn.db
      .delete(projectMember)
      .where(and(eq(projectMember.projectId, pid), eq(projectMember.userId, "owner2")));
  });
  test("two owners racing to demote each other cannot leave zero owners", async () => {
    await api.addMember(req("owner", "POST", { email: "owner2@x.test", role: "owner" }), slug);
    const rs = await Promise.all([
      api.updateMember(req("owner", "PATCH", { role: "viewer" }), { ...slug, userId: "owner2" }),
      api.updateMember(req("owner2", "PATCH", { role: "viewer" }), { ...slug, userId: "owner" }),
    ]);
    expect(rs.map((r) => r.status).sort()).toContain(409);
    const owners = await conn.db
      .select()
      .from(projectMember)
      .where(and(eq(projectMember.projectId, pid), eq(projectMember.role, "owner")));
    expect(owners.length).toBeGreaterThanOrEqual(1);
    await conn.db
      .update(projectMember)
      .set({ role: "owner" })
      .where(and(eq(projectMember.projectId, pid), eq(projectMember.userId, "owner")));
    await conn.db
      .delete(projectMember)
      .where(and(eq(projectMember.projectId, pid), eq(projectMember.userId, "owner2")));
  });
});

describe("invitations", () => {
  test("owner invites a client by email (lowercased, expiring, audited), lists and revokes", async () => {
    const res = await api.createInvitation(req("owner", "POST", { email: "Client.New@X.test" }), slug);
    expect(res.status).toBe(201);
    const inv = await res.json();
    expect(inv).toMatchObject({ email: "client.new@x.test", role: "client" });
    expect(new Date(inv.expiresAt).getTime()).toBeGreaterThan(Date.now() + 6 * 86_400_000);
    const [row] = await conn.db.select().from(projectInvitation).where(eq(projectInvitation.id, inv.id));
    expect(row).toMatchObject({ projectId: pid, invitedBy: "owner", acceptedAt: null });

    expect(
      await status(api.createInvitation(req("owner", "POST", { email: "client.new@x.test" }), slug)),
    ).toBe(409);
    expect(await status(api.createInvitation(req("owner", "POST", { email: "nope" }), slug))).toBe(400);
    expect(
      await status(api.createInvitation(req("owner", "POST", { email: "a@x.test", role: "wizard" }), slug)),
    ).toBe(400);

    const list = await (await api.listInvitations(req("owner"), slug)).json();
    expect(list.map((i: { id: string }) => i.id)).toContain(inv.id);

    expect(await status(api.revokeInvitation(req("owner", "DELETE"), { ...slug, id: inv.id }))).toBe(200);
    expect(await status(api.revokeInvitation(req("owner", "DELETE"), { ...slug, id: inv.id }))).toBe(404);
    expect(await audited("invitation.create")).toBe(1);
    expect(await audited("invitation.revoke")).toBe(1);
  });
  test("an invitation of another project cannot be revoked through this one", async () => {
    const [other] = await conn.db
      .insert(project)
      .values({ orgId: "umi-org", name: "O", slug: "other" })
      .returning();
    const [i] = await conn.db
      .insert(projectInvitation)
      .values({ projectId: other.id, email: "z@x.test", expiresAt: new Date(Date.now() + 1e6) })
      .returning();
    expect(await status(api.revokeInvitation(req("owner", "DELETE"), { ...slug, id: i.id }))).toBe(404);
    expect(await status(api.revokeInvitation(req("owner", "DELETE"), { ...slug, id: "not-a-uuid" }))).toBe(
      400,
    );
  });
});
