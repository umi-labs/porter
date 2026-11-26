import { createHash, randomBytes, randomUUID } from "node:crypto";
import { mkdtempSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  connect,
  mediaObject,
  organization,
  project,
  projectMember,
  projectVersion,
  runSecret,
  upload,
  user,
  versionMedia,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { decryptSecret } from "@porter-hub/porter/secrets";
import { packTarZst } from "@porter-hub/storage/tar";
import { and, desc, eq } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { HttpError } from "./projects";
import { cancelRun, createRun } from "./runs";
import { getSetup, saveSetup } from "./setup";
import { versionDir } from "./snapshot-reader";
import { keys, putFile } from "./storage";

process.env.HUB_SECRET_KEY = randomBytes(32).toString("base64");

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let pid: string;
let tmp: string;
const sha = (b: Buffer | string) => createHash("sha256").update(b).digest("hex");
const as = (id: string) => ({ id });
const status = (p: Promise<unknown>) =>
  p.then(
    () => 200,
    (e) => (e instanceof HttpError ? e.status : Promise.reject(e)),
  );

async function addUpload(state: "ready" | "uploading", filename = "export.xml") {
  const id = randomUUID();
  await conn.db
    .insert(upload)
    .values({ id, projectId: pid, filename, objectKey: keys.upload(pid, id), state, purpose: "wxr" });
  return id;
}
const latest = async () =>
  (
    await conn.db
      .select()
      .from(projectVersion)
      .where(eq(projectVersion.projectId, pid))
      .orderBy(desc(projectVersion.number))
      .limit(1)
  )[0];

beforeAll(async () => {
  tmp = mkdtempSync(join(tmpdir(), "setup-test-"));
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  const now = new Date();
  for (const id of ["dev", "viewer", "client"])
    await conn.db
      .insert(user)
      .values({ id, name: id, email: `${id}@x.test`, emailVerified: true, createdAt: now, updatedAt: now });
  await conn.db.insert(organization).values({ id: "umi-org", name: "Umi", slug: "umi", createdAt: now });
  const [p] = await conn.db
    .insert(project)
    .values({ orgId: "umi-org", name: "Acme", slug: "acme" })
    .returning();
  pid = p.id;
  await conn.db.insert(projectMember).values([
    { projectId: pid, userId: "dev", role: "developer" },
    { projectId: pid, userId: "viewer", role: "viewer" },
    { projectId: pid, userId: "client", role: "client" },
  ]);
});
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
  if (tmp) await rm(tmp, { recursive: true, force: true });
});

describe("setup on the hub", () => {
  test("only people who can push may set up; bad choices are refused", async () => {
    const ok = { adapter: "wordpress", kind: "crawl-only", siteUrl: "https://old.example.com" };
    expect(await status(saveSetup(conn.db, as("viewer"), "acme", ok))).toBe(403);
    expect(await status(saveSetup(conn.db, as("client"), "acme", ok))).toBe(403);
    expect(await status(saveSetup(conn.db, as("dev"), "acme", { adapter: "wordpress", kind: "wp-db" }))).toBe(
      400,
    );
    expect(await status(saveSetup(conn.db, as("dev"), "acme", { adapter: "sanity", kind: "wxr" }))).toBe(400);
    expect(
      await status(
        saveSetup(conn.db, as("dev"), "acme", { ...ok, kind: "wxr", upload: await addUpload("uploading") }),
      ),
    ).toBe(400);
    expect(
      await status(saveSetup(conn.db, as("dev"), "acme", { ...ok, siteUrl: "https://u:p@old.example.com" })),
    ).toBe(400);
    expect(await latest()).toBeUndefined();
  });

  test("the first setup is v1, porter.toml only, and can be run", async () => {
    const up = await addUpload("ready");
    const { number } = await saveSetup(conn.db, as("dev"), "acme", {
      adapter: "wordpress",
      kind: "wxr",
      upload: up,
      destUrl: "https://cms.example.com",
      payloadKey: "key-123456",
    });
    expect(number).toBe(1);
    const dir = await versionDir(await latest());
    expect(readFileSync(join(dir, "porter.toml"), "utf8")).toContain(`uploads/${up}/export.xml`);
    const [s] = await conn.db.select().from(runSecret).where(eq(runSecret.projectId, pid));
    expect(s.name).toBe("PAYLOAD_API_KEY");
    expect(decryptSecret(pid, s.name, s)).toBe("key-123456");

    const g = await getSetup(conn.db, as("dev"), "acme");
    expect(g.current).toMatchObject({ adapter: "wordpress", kind: "wxr", upload: up });
    expect(g.hasPayloadKey).toBe(true);

    // Runnable from the dashboard; and no setup while that run is active.
    const r = await createRun(conn.db, as("dev"), "acme", { stages: "all" });
    expect(r.base_version).toBe(1);
    expect(
      await status(
        saveSetup(conn.db, as("dev"), "acme", { adapter: "umbraco", kind: "umbraco-json", upload: up }),
      ),
    ).toBe(409);
    await cancelRun(conn.db, as("dev"), "acme", r.id);
  });

  test("a re-setup keeps the rest of the snapshot, the mapping and the media", async () => {
    // A pushed-looking version: porter.toml with a mapping, a project.db and one media file.
    const dir = mkdtempSync(join(tmp, "pushed-"));
    writeFileSync(
      join(dir, "porter.toml"),
      `[project]\nname = "Acme"\n[source]\nadapter = "wordpress"\n[mapping.types.post]\ncollection = "posts"\n`,
    );
    writeFileSync(join(dir, "project.db"), "not really sqlite");
    const out = join(tmp, "pushed.tar.zst");
    await packTarZst(dir, out);
    const key = keys.snapshot(pid, randomUUID());
    await putFile(key, out, sha(readFileSync(out)), statSync(out).size);
    const m = sha("pic");
    await conn.db.insert(mediaObject).values({ projectId: pid, sha256: m, size: 3 });
    const [v2] = await conn.db
      .insert(projectVersion)
      .values({
        projectId: pid,
        number: 2,
        snapshotKey: key,
        snapshotSha256: sha(readFileSync(out)),
        size: 1,
      })
      .returning();
    await conn.db.insert(versionMedia).values({ versionId: v2.id, projectId: pid, sha256: m });

    const { number } = await saveSetup(conn.db, as("dev"), "acme", {
      adapter: "sanity",
      kind: "sanity-api",
      projectId: "abc123",
      dataset: "production",
      sanityToken: "sk-secret-token",
    });
    expect(number).toBe(3);
    const v3 = await latest();
    const d = await versionDir(v3);
    const toml = readFileSync(join(d, "porter.toml"), "utf8");
    expect(toml).toContain('collection = "posts"');
    expect(toml).toContain('kind = "sanity-api"');
    expect(toml).not.toContain("sk-secret-token");
    expect(readFileSync(join(d, "project.db"), "utf8")).toBe("not really sqlite");
    const media = await conn.db.select().from(versionMedia).where(eq(versionMedia.versionId, v3.id));
    expect(media.map((x) => x.sha256)).toEqual([m]);
    const [tok] = await conn.db
      .select()
      .from(runSecret)
      .where(and(eq(runSecret.projectId, pid), eq(runSecret.name, "PORTER_SANITY_TOKEN")));
    expect(decryptSecret(pid, tok.name, tok)).toBe("sk-secret-token");
  });

  test("WordPress REST: the pasted URL is kept for porter, the login goes to run secrets", async () => {
    await saveSetup(conn.db, as("dev"), "acme", {
      adapter: "wordpress",
      kind: "wp-rest",
      url: "https://hotel.example.com/wp-json/wp/v2/pages/",
      wpUser: "harry",
      wpPassword: "abcd efgh ijkl mnop",
    });
    const toml = readFileSync(join(await versionDir(await latest()), "porter.toml"), "utf8");
    expect(toml).toContain('url = "https://hotel.example.com/wp-json/wp/v2/pages/"');
    expect(toml).not.toContain("abcd efgh");
    const rows = await conn.db.select().from(runSecret).where(eq(runSecret.projectId, pid));
    const got = Object.fromEntries(rows.map((r) => [r.name, decryptSecret(pid, r.name, r)]));
    expect(got.PORTER_WP_USERNAME).toBe("harry");
    expect(got.PORTER_WP_PASSWORD).toBe("abcd efgh ijkl mnop");
    expect((await getSetup(conn.db, as("dev"), "acme")).hasWpLogin).toBe(true);
  });
});
