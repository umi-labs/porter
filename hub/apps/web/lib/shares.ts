import { createHash, randomBytes } from "node:crypto";
import { project, shareLink } from "@porter-hub/db";
import { and, desc, eq, gt, isNull } from "drizzle-orm";
import { z } from "zod";
import type { Db } from "./permissions";
import { type Actor, access, audit, HttpError, parse } from "./projects";

export const DEFAULT_DAYS = 14;
export const MAX_DAYS = 90;
export const scopes = ["report", "project-readonly"] as const;
export type Scope = (typeof scopes)[number];

export const shareSchema = z.object({
  scope: z.enum(scopes),
  days: z.coerce.number().int().min(1).max(MAX_DAYS).default(DEFAULT_DAYS),
});

export const hashToken = (token: string) => createHash("sha256").update(token).digest("hex");

export type SharePage = "index" | "report" | "overview" | "issues" | "urls";
/** What each scope may reach under /s/[token]. Neither scope ever reaches item data, raw/, submissions, the snapshot or media. */
const matrix: Record<Scope, readonly SharePage[]> = {
  report: ["index", "report"],
  "project-readonly": ["index", "report", "overview", "issues", "urls"],
};
export const shareAllows = (scope: Scope, page: SharePage) => matrix[scope].includes(page);

export async function createShare(db: Db, actor: Actor, slug: string, input: unknown) {
  const { project: p } = await access(db, actor, slug, "share");
  const { scope, days } = parse(shareSchema, input);
  const token = randomBytes(32).toString("base64url");
  const expiresAt = new Date(Date.now() + days * 86_400_000);
  return db.transaction(async (tx) => {
    const [row] = await tx
      .insert(shareLink)
      .values({ projectId: p.id, tokenHash: hashToken(token), scope, expiresAt, createdBy: actor.id })
      .returning({ id: shareLink.id });
    await audit(tx, actor, p.id, "share.create", row.id, { scope, expiresAt });
    // The only time the token exists in the clear.
    return { id: row.id, token, scope, expiresAt };
  });
}

export async function listShares(db: Db, actor: Actor, slug: string) {
  const { project: p } = await access(db, actor, slug, "share");
  return db
    .select({
      id: shareLink.id,
      scope: shareLink.scope,
      expiresAt: shareLink.expiresAt,
      revokedAt: shareLink.revokedAt,
      createdAt: shareLink.createdAt,
    })
    .from(shareLink)
    .where(eq(shareLink.projectId, p.id))
    .orderBy(desc(shareLink.createdAt));
}

export async function revokeShare(db: Db, actor: Actor, slug: string, id: string) {
  const { project: p } = await access(db, actor, slug, "share");
  const linkId = parse(z.uuid(), id);
  return db.transaction(async (tx) => {
    const gone = await tx
      .update(shareLink)
      .set({ revokedAt: new Date() })
      .where(and(eq(shareLink.id, linkId), eq(shareLink.projectId, p.id), isNull(shareLink.revokedAt)))
      .returning({ id: shareLink.id });
    if (gone.length === 0) throw new HttpError(404, "Share link not found");
    await audit(tx, actor, p.id, "share.revoke", linkId);
    return { id: linkId };
  });
}

/** The link's project and scope, or null. Unknown, expired and revoked are indistinguishable to the caller. */
export async function resolveShare(db: Db, token: string) {
  if (token.length > 100) return null;
  const [row] = await db
    .select({ project, scope: shareLink.scope })
    .from(shareLink)
    .innerJoin(project, eq(project.id, shareLink.projectId))
    .where(
      and(
        eq(shareLink.tokenHash, hashToken(token)),
        isNull(shareLink.revokedAt),
        gt(shareLink.expiresAt, new Date()),
        isNull(project.archivedAt),
      ),
    );
  return row ?? null;
}
