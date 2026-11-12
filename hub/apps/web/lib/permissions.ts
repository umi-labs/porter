import { type connect, projectMember } from "@porter-hub/db";
import { and, eq } from "drizzle-orm";

export type Db = ReturnType<typeof connect>["db"];
export type Role = "owner" | "developer" | "viewer" | "client";
export type Action =
  | "view"
  | "view_raw"
  | "push"
  | "pull"
  | "run"
  | "share"
  | "upload"
  | "manage_members"
  | "manage_secrets";

/** Role x action matrix. Clients never see raw data or issue details; staff-org membership grants nothing here. */
const allowed: Record<Role, readonly Action[]> = {
  owner: ["view", "view_raw", "push", "pull", "run", "share", "upload", "manage_members", "manage_secrets"],
  developer: ["view", "view_raw", "push", "pull", "run", "share", "upload", "manage_secrets"],
  viewer: ["view", "view_raw", "pull"],
  client: ["view", "upload"],
};

/** A null role (not a member) can do nothing. */
export const roleCan = (role: Role | null, action: Action) => !!role && allowed[role].includes(action);

/** The user's role on the project, or null if they are not a member. */
export async function roleOf(db: Db, user: { id: string }, project: { id: string }) {
  const [m] = await db
    .select({ role: projectMember.role })
    .from(projectMember)
    .where(and(eq(projectMember.projectId, project.id), eq(projectMember.userId, user.id)));
  return m?.role ?? null;
}

export async function can(db: Db, user: { id: string }, project: { id: string }, action: Action) {
  return roleCan(await roleOf(db, user, project), action);
}
