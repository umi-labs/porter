import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";
import * as schema from "./schema";

export * from "./schema";
export { schema };

export function connect(url = process.env.DATABASE_URL, options: postgres.Options<Record<string, never>> = {}) {
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
