import { fileURLToPath } from "node:url";
import { drizzle } from "drizzle-orm/postgres-js";
import { migrate } from "drizzle-orm/postgres-js/migrator";
import postgres from "postgres";

export const migrationsFolder = fileURLToPath(new URL("./migrations", import.meta.url));

export async function runMigrations(url = process.env.DATABASE_URL) {
  if (!url) throw new Error("DATABASE_URL is not set");
  const client = postgres(url, { max: 1 });
  try {
    await migrate(drizzle(client), { migrationsFolder });
  } finally {
    await client.end();
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
