import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { drizzle } from "drizzle-orm/postgres-js";
import { migrate } from "drizzle-orm/postgres-js/migrator";
import postgres from "postgres";

/** MIGRATIONS_DIR: the bundled web server can't find this file's folder, so the image sets it. */
export const migrationsFolder = () =>
  // Not `new URL("./migrations", import.meta.url)`: webpack would try to bundle the folder.
  process.env.MIGRATIONS_DIR || join(dirname(fileURLToPath(import.meta.url)), "migrations");

/** Any fixed number shared by every process that migrates this database. */
const MIGRATION_LOCK = 0x706f7274; // "port"

/**
 * Applies pending migrations. Idempotent, and safe with several replicas starting at once: a session advisory
 * lock (max: 1 keeps lock and migration on one connection) makes them take turns; the later ones find nothing to do.
 */
export async function runMigrations(url = process.env.DATABASE_URL) {
  if (!url) throw new Error("DATABASE_URL is not set");
  const client = postgres(url, { max: 1, onnotice: () => {} });
  try {
    await client`select pg_advisory_lock(${MIGRATION_LOCK})`;
    await migrate(drizzle(client), { migrationsFolder: migrationsFolder() });
  } finally {
    await client.end(); // ends the session, which releases the lock
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  runMigrations().then(
    () => console.log("migrations applied"),
    (e) => {
      console.error(e);
      process.exit(1);
    },
  );
}
