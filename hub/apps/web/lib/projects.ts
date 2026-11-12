import {
  auditLog,
  member,
  organization,
  project,
  projectInvitation,
  projectMember,
  user,
} from "@porter-hub/db";
import { and, desc, eq, gt, isNull, sql } from "drizzle-orm";
import { z } from "zod";
import { ensureStaffOrg, staffOrgSlug } from "./auth";
import { type Action, type Db, type Role, roleCan } from "./permissions";

export class HttpError extends Error {
  constructor(
    public status: number,
    message: string,
    /** Extra JSON fields for the response body, e.g. `{ latest }` on a 409. */
    public extra: object = {},
  ) {
    super(message);
  }
}

export type Actor = { id: string };
const roles = ["owner", "developer", "viewer", "client"] as const;
const slugRe = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const email = z.string().trim().toLowerCase().pipe(z.email().max(254));

export const schemas = {
  createProject: z.object({
    name: z.string().trim().min(1).max(120),
    slug: z.string().max(60).regex(slugRe).optional(),
    summary: z.string().trim().max(200).optional(),
  }),
  addMember: z.object({ email, role: z.enum(roles) }),
  setRole: z.object({ role: z.enum(roles) }),
  invite: z.object({ email, role: z.enum(roles).default("client") }),
  id: z.uuid(),
};

export function parse<T>(schema: z.ZodType<T>, input: unknown): T {
  const r = schema.safeParse(input);
  if (!r.success)
    throw new HttpError(400, r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`).join("; "));
  return r.data;
}

const slugify = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60)
    .replace(/-+$/, "");

export type Tx = Parameters<Parameters<Db["transaction"]>[0]>[0];
export const audit = (
  tx: Tx | Db,
  actor: Actor,
  projectId: string,
  action: string,
  targetId: string,
  detail?: object,
) =>
  tx
    .insert(auditLog)
    .values({ projectId, actorId: actor.id, action, targetType: action.split(".")[0], targetId, detail });

/**
 * Loads the project and checks the caller may do `action`. Non-members get 404 (so existence doesn't
 * leak); members lacking the action get 403. `archivedOk` is for calls that only stop things (cancel a
 * run, abort an upload): they must work on an archived project too.
 */
export async function access(
  db: Db,
  actor: Actor,
  slug: string,
  action: Action,
  { archivedOk = false }: { archivedOk?: boolean } = {},
) {
  const [row] = await db
    .select({ project, role: projectMember.role })
    .from(project)
    .innerJoin(
      projectMember,
      and(eq(projectMember.projectId, project.id), eq(projectMember.userId, actor.id)),
    )
    .where(eq(project.slug, slug));
  if (!row) throw new HttpError(404, "Project not found");
  if (!roleCan(row.role, action)) throw new HttpError(403, "You don't have permission to do that");
  // An archived project is waiting for retention to delete it: nothing new goes in.
  if (row.project.archivedAt && !archivedOk && WRITES.includes(action))
    throw new HttpError(409, "The project is archived");
  return row;
}
const WRITES: readonly Action[] = ["push", "run", "upload"];

export async function listProjects(db: Db, actor: Actor) {
  return db
    .select({
      id: project.id,
      name: project.name,
      slug: project.slug,
      summary: project.summary,
      role: projectMember.role,
    })
    .from(projectMember)
    .innerJoin(project, eq(project.id, projectMember.projectId))
    .where(and(eq(projectMember.userId, actor.id), isNull(project.archivedAt)))
    .orderBy(project.name);
}

export async function getProject(db: Db, actor: Actor, slug: string) {
  const { project: p, role } = await access(db, actor, slug, "view");
  return { id: p.id, name: p.name, slug: p.slug, summary: p.summary, role };
}

/** Staff = a member of the staff org. Only staff create projects. */
export async function isStaff(db: Db, actor: Actor) {
  const [staff] = await db
    .select({ id: member.id })
    .from(member)
    .innerJoin(organization, eq(organization.id, member.organizationId))
    .where(and(eq(member.userId, actor.id), eq(organization.slug, staffOrgSlug())));
  return !!staff;
}

export async function createProject(db: Db, actor: Actor, input: z.infer<typeof schemas.createProject>) {
  if (!(await isStaff(db, actor))) throw new HttpError(403, "Only staff can create projects");
  const slug = input.slug ?? slugify(input.name);
  if (!slug) throw new HttpError(400, "name: needs letters or digits to make a slug");
  const orgId = await ensureStaffOrg(db);
  try {
    return await db.transaction(async (tx) => {
      const [p] = await tx
        .insert(project)
        .values({ orgId, name: input.name, slug, summary: input.summary, createdBy: actor.id })
        .returning();
      await tx.insert(projectMember).values({ projectId: p.id, userId: actor.id, role: "owner" });
      await audit(tx, actor, p.id, "project.create", p.id, { slug });
      return { id: p.id, name: p.name, slug: p.slug, summary: p.summary, role: "owner" as Role };
    });
  } catch (e) {
    const code =
      (e as { code?: string; cause?: { code?: string } }).cause?.code ?? (e as { code?: string }).code;
    if (code === "23505") throw new HttpError(409, `A project with the slug "${slug}" already exists`);
    throw e;
  }
}

export async function listMembers(db: Db, actor: Actor, slug: string) {
  const { project: p } = await access(db, actor, slug, "manage_members");
  return db
    .select({ userId: user.id, name: user.name, email: user.email, role: projectMember.role })
    .from(projectMember)
    .innerJoin(user, eq(user.id, projectMember.userId))
    .where(eq(projectMember.projectId, p.id))
    .orderBy(user.email);
}

/** Locks the project's owner rows so concurrent demotions can't both pass the last-owner check. */
async function assertNotLastOwner(tx: Tx, projectId: string, userId: string) {
  const owners = await tx
    .select({ userId: projectMember.userId })
    .from(projectMember)
    .where(and(eq(projectMember.projectId, projectId), eq(projectMember.role, "owner")))
    .for("update");
  if (owners.length === 1 && owners[0].userId === userId) {
    throw new HttpError(409, "A project needs at least one owner");
  }
}

export async function addMember(
  db: Db,
  actor: Actor,
  slug: string,
  input: z.infer<typeof schemas.addMember>,
) {
  const { project: p } = await access(db, actor, slug, "manage_members");
  const [u] = await db.select({ id: user.id }).from(user).where(sql`lower(${user.email}) = ${input.email}`);
  if (!u) throw new HttpError(404, "No user with that email has signed up; invite them instead");
  return db.transaction(async (tx) => {
    const added = await tx
      .insert(projectMember)
      .values({ projectId: p.id, userId: u.id, role: input.role })
      .onConflictDoNothing()
      .returning();
    if (added.length === 0) throw new HttpError(409, "Already a member");
    await audit(tx, actor, p.id, "member.add", u.id, { role: input.role });
    return { userId: u.id, email: input.email, role: input.role };
  });
}

export async function setRole(db: Db, actor: Actor, slug: string, userId: string, role: Role) {
  const { project: p } = await access(db, actor, slug, "manage_members");
  return db.transaction(async (tx) => {
    if (role !== "owner") await assertNotLastOwner(tx, p.id, userId);
    const [before] = await tx
      .select({ role: projectMember.role })
      .from(projectMember)
      .where(and(eq(projectMember.projectId, p.id), eq(projectMember.userId, userId)));
    if (!before) throw new HttpError(404, "Not a member of this project");
    await tx
      .update(projectMember)
      .set({ role })
      .where(and(eq(projectMember.projectId, p.id), eq(projectMember.userId, userId)));
    await audit(tx, actor, p.id, "member.role", userId, { from: before.role, to: role });
    return { userId, role };
  });
}

export async function removeMember(db: Db, actor: Actor, slug: string, userId: string) {
  const { project: p } = await access(db, actor, slug, "manage_members");
  return db.transaction(async (tx) => {
    await assertNotLastOwner(tx, p.id, userId);
    const gone = await tx
      .delete(projectMember)
      .where(and(eq(projectMember.projectId, p.id), eq(projectMember.userId, userId)))
      .returning();
    if (gone.length === 0) throw new HttpError(404, "Not a member of this project");
    await audit(tx, actor, p.id, "member.remove", userId, { role: gone[0].role });
    return { userId };
  });
}

const pending = (projectId: string) =>
  and(
    eq(projectInvitation.projectId, projectId),
    isNull(projectInvitation.acceptedAt),
    gt(projectInvitation.expiresAt, new Date()),
  );

export async function listInvitations(db: Db, actor: Actor, slug: string) {
  const { project: p } = await access(db, actor, slug, "manage_members");
  return db
    .select({
      id: projectInvitation.id,
      email: projectInvitation.email,
      role: projectInvitation.role,
      expiresAt: projectInvitation.expiresAt,
    })
    .from(projectInvitation)
    .where(pending(p.id))
    .orderBy(desc(projectInvitation.createdAt));
}

export const INVITE_DAYS = 7;

export async function createInvitation(
  db: Db,
  actor: Actor,
  slug: string,
  input: z.infer<typeof schemas.invite>,
) {
  const { project: p } = await access(db, actor, slug, "manage_members");
  return db.transaction(async (tx) => {
    const [existing] = await tx
      .select({ id: user.id })
      .from(user)
      .innerJoin(projectMember, and(eq(projectMember.userId, user.id), eq(projectMember.projectId, p.id)))
      .where(sql`lower(${user.email}) = ${input.email}`);
    if (existing) throw new HttpError(409, "Already a member; change their role instead");
    const [dupe] = await tx
      .select({ id: projectInvitation.id })
      .from(projectInvitation)
      .where(and(pending(p.id), eq(projectInvitation.email, input.email)));
    if (dupe) throw new HttpError(409, "That email already has a pending invitation");
    const [inv] = await tx
      .insert(projectInvitation)
      .values({
        projectId: p.id,
        email: input.email,
        role: input.role,
        invitedBy: actor.id,
        expiresAt: new Date(Date.now() + INVITE_DAYS * 86_400_000),
      })
      .returning();
    await audit(tx, actor, p.id, "invitation.create", inv.id, { email: inv.email, role: inv.role });
    return { id: inv.id, email: inv.email, role: inv.role, expiresAt: inv.expiresAt };
  });
}

export async function revokeInvitation(db: Db, actor: Actor, slug: string, id: string) {
  const { project: p } = await access(db, actor, slug, "manage_members");
  return db.transaction(async (tx) => {
    const gone = await tx
      .delete(projectInvitation)
      .where(and(eq(projectInvitation.id, id), pending(p.id)))
      .returning();
    if (gone.length === 0) throw new HttpError(404, "Invitation not found");
    await audit(tx, actor, p.id, "invitation.revoke", id, { email: gone[0].email });
    return { id };
  });
}
