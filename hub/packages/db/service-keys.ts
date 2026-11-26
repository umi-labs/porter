import { createHash, randomBytes, randomUUID } from "node:crypto";
import { and, eq, isNull } from "drizzle-orm";
import { type connect, ensureStaffOrg } from "./index";
import { member, serviceKey, user } from "./schema";

type Db = ReturnType<typeof connect>["db"];

/** Service keys look like this, so they never collide with Better Auth's session bearer tokens. */
export const SERVICE_KEY_PREFIX = "phk_";
export const hashServiceKey = (key: string) => createHash("sha256").update(key).digest("hex");

/**
 * Mints a key for another service (e.g. "Foundry hub"). The key acts as a bot user of its own, a member of the
 * staff org, so every existing permission check applies to it. Returns the key: it is shown once, never stored.
 */
export async function mintServiceKey(db: Db, name: string, createdBy?: string) {
  const orgId = await ensureStaffOrg(db);
  const key = `${SERVICE_KEY_PREFIX}${randomBytes(32).toString("base64url")}`;
  const now = new Date();
  const botId = `service-${randomUUID()}`;
  await db.transaction(async (tx) => {
    await tx.insert(user).values({
      id: botId,
      name,
      // Not a real mailbox: the bot can never sign in, only use its key.
      email: `${botId}@service.porter.invalid`,
      emailVerified: false,
      createdAt: now,
      updatedAt: now,
    });
    await tx
      .insert(member)
      .values({ id: randomUUID(), organizationId: orgId, userId: botId, role: "member", createdAt: now });
    await tx.insert(serviceKey).values({
      name,
      prefix: key.slice(0, SERVICE_KEY_PREFIX.length + 6),
      keyHash: hashServiceKey(key),
      userId: botId,
      createdBy: createdBy ?? null,
    });
  });
  return { key, userId: botId };
}

/** The bot user behind a live key, or null. Marks the key used (at most once a minute). */
export async function resolveServiceKey(db: Db, key: string) {
  if (!key.startsWith(SERVICE_KEY_PREFIX)) return null;
  const [k] = await db
    .select({ id: serviceKey.id, userId: serviceKey.userId, lastUsedAt: serviceKey.lastUsedAt })
    .from(serviceKey)
    .where(and(eq(serviceKey.keyHash, hashServiceKey(key)), isNull(serviceKey.revokedAt)));
  if (!k) return null;
  if (!k.lastUsedAt || Date.now() - k.lastUsedAt.getTime() > 60_000)
    await db.update(serviceKey).set({ lastUsedAt: new Date() }).where(eq(serviceKey.id, k.id));
  return { id: k.userId };
}
