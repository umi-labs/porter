import { randomBytes } from "node:crypto";
import postgres from "postgres";

const admin = process.env.TEST_ADMIN_URL ?? "postgres://porter:porter@localhost:55439/postgres";

/** Creates a fresh empty database; returns its URL and a cleanup fn. */
export async function freshDb() {
  const name = `t_${randomBytes(6).toString("hex")}`;
  const sql = postgres(admin, { max: 1 });
  await sql.unsafe(`create database ${name}`);
  await sql.end();
  const url = admin.replace(/\/[^/]*$/, `/${name}`);
  return {
    url,
    async drop() {
      const s = postgres(admin, { max: 1 });
      await s.unsafe(`drop database ${name} with (force)`);
      await s.end();
    },
  };
}
