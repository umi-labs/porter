import postgres from "postgres";
import { afterAll, beforeAll, expect, test } from "vitest";
import { runMigrations } from "./migrate";
import { freshDb } from "./test-helper";

let db: Awaited<ReturnType<typeof freshDb>>;
beforeAll(async () => {
  db = await freshDb();
});
afterAll(() => db.drop());

const expected = [
  // Better Auth
  "user",
  "session",
  "account",
  "verification",
  "organization",
  "member",
  "invitation",
  "device_code",
  // porter
  "project",
  "project_member",
  "project_invitation",
  "project_version",
  "media_object",
  "run",
  "run_event",
  "share_link",
  "upload",
  "run_secret",
  "audit_log",
];

test("schema migrates on a fresh db", async () => {
  await runMigrations(db.url);
  const sql = postgres(db.url);
  const rows = await sql`select table_name from information_schema.tables where table_schema='public'`;
  const have = rows.map((r) => r.table_name as string);
  for (const t of expected) expect(have, `missing table ${t}`).toContain(t);

  // share links store only a hash; secrets only ciphertext
  const cols = async (t: string) =>
    (await sql`select column_name from information_schema.columns where table_name=${t}`).map(
      (r) => r.column_name,
    );
  expect(await cols("share_link")).toContain("token_hash");
  expect(await cols("share_link")).not.toContain("token");
  expect(await cols("run_secret")).toEqual(expect.arrayContaining(["ciphertext", "nonce"]));
  await sql.end();
});

test("migrating twice is a no-op", async () => {
  await runMigrations(db.url);
});

test("only one active run per project", async () => {
  const sql = postgres(db.url);
  await sql`insert into organization (id,name,slug,created_at) values ('o','O','o',now())`;
  const [p] = await sql`insert into project (org_id,name,slug) values ('o','P','p') returning id`;
  await sql`insert into run (project_id) values (${p.id})`;
  await expect(sql`insert into run (project_id) values (${p.id})`).rejects.toThrow(
    /run_one_active_per_project/,
  );
  await sql.end();
});

test("several replicas migrating one fresh db at once: all succeed, each migration applied once", async () => {
  const other = await freshDb();
  try {
    await Promise.all([runMigrations(other.url), runMigrations(other.url), runMigrations(other.url)]);
    const sql = postgres(other.url, { max: 1 });
    const [{ n }] = await sql`select count(*)::int as n from drizzle.__drizzle_migrations`;
    await sql.end();
    const journal = await import("./migrations/meta/_journal.json", { with: { type: "json" } });
    expect(n).toBe(journal.default.entries.length);
  } finally {
    await other.drop();
  }
});
