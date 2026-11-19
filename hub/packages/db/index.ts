import { randomUUID } from "node:crypto";
import { eq } from "drizzle-orm";
import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";
import * as schema from "./schema";

export * from "./schema";
export { schema };

export function connect(
  url = process.env.DATABASE_URL,
  options: postgres.Options<Record<string, never>> = {},
) {
  if (!url) throw new Error("DATABASE_URL is not set");
  const client = postgres(url, options);
  return { db: drizzle(client, { schema }), client };
}

const g = globalThis as { __porterDb?: ReturnType<typeof connect> };
/** One pool per process; kept on globalThis so Next dev hot reload doesn't open a new one each time. */
export const getDb = () => {
  g.__porterDb ??= connect();
  return g.__porterDb.db;
};

/** The one staff org's slug (membership there = staff). */
export const staffOrgSlug = () => process.env.STAFF_ORG_SLUG || "umi";

/** Creates the staff org if missing (server-side only, idempotent); returns its id. */
export async function ensureStaffOrg(db: ReturnType<typeof connect>["db"]) {
  const slug = staffOrgSlug();
  await db
    .insert(schema.organization)
    .values({ id: randomUUID(), name: "Umi", slug, createdAt: new Date() })
    .onConflictDoNothing({ target: schema.organization.slug });
  const [o] = await db
    .select({ id: schema.organization.id })
    .from(schema.organization)
    .where(eq(schema.organization.slug, slug));
  return o.id;
}
