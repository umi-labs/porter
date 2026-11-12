import { runSecret } from "@porter-hub/db";
import { encryptSecret, secretNameProblem } from "@porter-hub/porter/secrets";
import { and, eq } from "drizzle-orm";
import { z } from "zod";
import type { Db } from "./permissions";
import { type Actor, access, audit, HttpError, parse } from "./projects";

/** Write-only: values go in encrypted and never come back out of the API. */
const value = z.object({
  value: z
    .string()
    .min(1)
    .max(8192)
    .refine((v) => !v.includes("\0"), "must not contain NUL"),
});

const checkName = (name: string) => {
  const problem = secretNameProblem(name);
  if (problem) throw new HttpError(400, `name: ${problem}`);
  return name;
};

/** Names only, with when each was last set. */
export async function listSecrets(db: Db, actor: Actor, slug: string) {
  const { project: p } = await access(db, actor, slug, "manage_secrets");
  const rows = await db
    .select({ name: runSecret.name, updatedAt: runSecret.updatedAt })
    .from(runSecret)
    .where(eq(runSecret.projectId, p.id))
    .orderBy(runSecret.name);
  return rows.map((r) => ({ name: r.name, set: true, updated_at: r.updatedAt }));
}

export async function putSecret(db: Db, actor: Actor, slug: string, name: string, input: unknown) {
  const { project: p } = await access(db, actor, slug, "manage_secrets");
  checkName(name);
  const enc = encryptSecret(p.id, name, parse(value, input).value);
  await db.transaction(async (tx) => {
    await tx
      .insert(runSecret)
      .values({ projectId: p.id, name, ...enc, createdBy: actor.id })
      .onConflictDoUpdate({
        target: [runSecret.projectId, runSecret.name],
        set: { ...enc, createdBy: actor.id, updatedAt: new Date() },
      });
    await audit(tx, actor, p.id, "secret.put", name);
  });
  return { name, set: true };
}

export async function deleteSecret(db: Db, actor: Actor, slug: string, name: string) {
  const { project: p } = await access(db, actor, slug, "manage_secrets");
  checkName(name);
  await db.transaction(async (tx) => {
    const [r] = await tx
      .delete(runSecret)
      .where(and(eq(runSecret.projectId, p.id), eq(runSecret.name, name)))
      .returning({ id: runSecret.id });
    if (!r) throw new HttpError(404, "No such secret");
    await audit(tx, actor, p.id, "secret.delete", name);
  });
  return { name, set: false };
}
