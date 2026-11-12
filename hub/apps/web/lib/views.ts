import type { Db } from "./permissions";
import { type Actor, access } from "./projects";
import { resolveShare, type SharePage, shareAllows } from "./shares";
import { latestSnapshot } from "./snapshot-reader";

/** Staff/client view of a project: checks `action` (404 non-member, 403 lacking it), then loads the latest snapshot (null if none yet). */
export async function loadProject(db: Db, actor: Actor, slug: string, action: "view" | "view_raw") {
  const { project, role } = await access(db, actor, slug, action);
  return { project, role, snap: await latestSnapshot(db, project.id) };
}

/** A share link's view of one page, or null (render 404) if the token is bad or its scope doesn't reach `page`. */
export async function loadShare(db: Db, token: string, page: SharePage) {
  const link = await resolveShare(db, token);
  if (!link || !shareAllows(link.scope, page)) return null;
  return { project: link.project, scope: link.scope, snap: await latestSnapshot(db, link.project.id) };
}
