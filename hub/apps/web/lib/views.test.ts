import { createHash, randomUUID } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { zstdCompressSync } from "node:zlib";
import {
  auditLog,
  connect,
  member,
  organization,
  project,
  projectMember,
  projectVersion,
  shareLink,
  user,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { eq } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { makeApi } from "./api";
import { HttpError } from "./projects";
import { REPORT_HEADERS, SHARE_HEADERS, serveReport } from "./report";
import * as sh from "./shares";
import { extractTarZst, issuesPage, listReports, overview, PAGE_SIZE, urlsPage } from "./snapshot-reader";
import { hexToBase64, keys, presignPut } from "./storage";
import { loadProject, loadShare } from "./views";

// ---- tar builder (ustar), so tests can also build hostile archives ----
function entry(
  name: string,
  body: Buffer | string = "",
  type = name.endsWith("/") ? "5" : "0",
  linkname = "",
) {
  const data = Buffer.from(body);
  const h = Buffer.alloc(512);
  h.write(name, 0, "utf8");
  h.write("0000644\0", 100);
  h.write("0000000\0", 108);
  h.write("0000000\0", 116);
  h.write(`${data.length.toString(8).padStart(11, "0")}\0`, 124);
  h.write("00000000000\0", 136);
  h.write("        ", 148);
  h.write(type, 156);
  h.write(linkname, 157);
  h.write("ustar\x0000", 257, "latin1");
  h.write(
    `${[...h]
      .reduce((a, b) => a + b, 0)
      .toString(8)
      .padStart(6, "0")}\0 `,
    148,
  );
  return Buffer.concat([h, data, Buffer.alloc((512 - (data.length % 512)) % 512)]);
}
const tarZst = (...entries: Buffer[]) => zstdCompressSync(Buffer.concat([...entries, Buffer.alloc(1024)]));

function makeProjectDb(path: string, opts: { issues?: number; noIssues?: boolean } = {}) {
  const db = new DatabaseSync(path);
  db.exec(`
    CREATE TABLE items (id TEXT PRIMARY KEY, kind TEXT NOT NULL, connector TEXT NOT NULL DEFAULT 'wp', source_id TEXT NOT NULL DEFAULT '', url TEXT, data TEXT NOT NULL, deleted_at TEXT);
    CREATE TABLE runs (id TEXT PRIMARY KEY, stage TEXT NOT NULL, porter_version TEXT NOT NULL DEFAULT '2', started_at TEXT NOT NULL, ended_at TEXT, summary TEXT, deleted_at TEXT);
    ${opts.noIssues ? "" : "CREATE TABLE issues (id TEXT PRIMARY KEY, run_id TEXT, kind TEXT NOT NULL, severity TEXT NOT NULL, item_id TEXT NOT NULL DEFAULT '', stage TEXT NOT NULL, message TEXT NOT NULL, details TEXT NOT NULL DEFAULT '{}', resolution TEXT NOT NULL, deleted_at TEXT);"}
  `);
  db.prepare("INSERT INTO items (id,kind,data) VALUES ('d1','document',?)").run(
    JSON.stringify({ title: "About us", SECRET_FIELD: "field-data-must-not-leak" }),
  );
  db.prepare("INSERT INTO items (id,kind,data) VALUES ('r1','route',?)").run(
    JSON.stringify({ url: "/about", http_status: 200, target: { document: "d1" } }),
  );
  db.prepare("INSERT INTO items (id,kind,data) VALUES ('r2','route',?)").run(
    JSON.stringify({ url: "/gone", http_status: 404, target: "none" }),
  );
  db.prepare(
    "INSERT INTO runs (id,stage,started_at,ended_at) VALUES ('1','analyse','2026-01-01',NULL)",
  ).run();
  db.prepare(
    "INSERT INTO runs (id,stage,started_at,ended_at) VALUES ('2','analyse','2026-01-02','2026-01-03')",
  ).run();
  if (!opts.noIssues) {
    const ins = db.prepare(
      "INSERT INTO issues (id,kind,severity,item_id,stage,message,resolution) VALUES (?,?,?,?,?,?,'open')",
    );
    for (let i = 0; i < (opts.issues ?? 3); i++)
      ins.run(
        String(i).padStart(5, "0"),
        "broken-link",
        "warning",
        i === 0 ? "d1" : "",
        "analyse",
        `issue ${i}`,
      );
  }
  db.close();
}

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let api: ReturnType<typeof makeApi>;
let pid: string;
let work: string;
const USERS = ["owner", "dev", "viewer", "client", "stranger"];
const req = (as: string | null, body?: unknown) =>
  new Request("http://hub.test/x", {
    method: body === undefined ? "GET" : "POST",
    headers: { ...(as ? { "x-user": as } : {}), "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
const slug = { slug: "acme" };
const status = async (p: Promise<unknown>) => {
  try {
    await p;
    return 200;
  } catch (e) {
    if (e instanceof HttpError) return e.status;
    throw e;
  }
};

async function pushSnapshot(number: number, archive: Buffer) {
  const id = randomUUID();
  const key = keys.snapshot(pid, id);
  const sha = createHash("sha256").update(archive).digest("hex");
  const r = await fetch(await presignPut(key, sha), {
    method: "PUT",
    body: new Uint8Array(archive),
    headers: { "if-none-match": "*", "x-amz-checksum-sha256": hexToBase64(sha) },
  });
  expect(r.status).toBe(200);
  await conn.db
    .insert(projectVersion)
    .values({ projectId: pid, number, snapshotKey: key, snapshotSha256: sha, size: archive.length });
}

beforeAll(async () => {
  work = mkdtempSync(join(tmpdir(), "hub-views-"));
  process.env.HUB_CACHE_DIR = join(work, "cache");
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  const now = new Date();
  for (const id of USERS)
    await conn.db
      .insert(user)
      .values({ id, name: id, email: `${id}@x.test`, emailVerified: true, createdAt: now, updatedAt: now });
  await conn.db.insert(organization).values({ id: "umi-org", name: "Umi", slug: "umi", createdAt: now });
  await conn.db
    .insert(member)
    .values({ id: "m-o", organizationId: "umi-org", userId: "owner", role: "member", createdAt: now });
  const [p] = await conn.db
    .insert(project)
    .values({ orgId: "umi-org", name: "Acme", slug: "acme" })
    .returning();
  pid = p.id;
  await conn.db.insert(projectMember).values([
    { projectId: pid, userId: "owner", role: "owner" },
    { projectId: pid, userId: "dev", role: "developer" },
    { projectId: pid, userId: "viewer", role: "viewer" },
    { projectId: pid, userId: "client", role: "client" },
  ]);
  api = makeApi(conn.db, async (r) => {
    const id = r.headers.get("x-user");
    return id ? { id } : null;
  });

  const src = join(work, "src");
  mkdirSync(src);
  makeProjectDb(join(src, "project.db"), { issues: 250 });
  await pushSnapshot(
    1,
    tarZst(
      entry("./"),
      entry("porter.toml", "secret = 1"),
      entry("project.db", readFileSync(join(src, "project.db"))),
      entry("raw/"),
      entry("raw/dump.sql", "RAW-SUBMISSIONS"),
      entry("reports/"),
      entry("reports/a.html", "<h1>report</h1>"),
      entry("reports/data.json", "{}"),
      entry("reports/notes.txt", "txt"),
    ),
  );
});
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
  rmSync(work, { recursive: true, force: true });
});

describe("safe extraction", () => {
  const dest = () => mkdtempSync(join(work, "x-"));
  test("extracts regular files and dirs", async () => {
    const d = dest();
    await extractTarZst(tarZst(entry("a/"), entry("a/b.txt", "hi")), d);
    expect(readFileSync(join(d, "a/b.txt"), "utf8")).toBe("hi");
  });
  test.each([
    ["a `..` path", entry("../evil.txt", "x")],
    ["a nested `..` path", entry("a/../../evil.txt", "x")],
    ["an absolute path", entry("/tmp/evil.txt", "x")],
    ["a symlink", entry("link", "", "2", "/etc/passwd")],
    ["a hardlink", entry("link", "", "1", "project.db")],
    ["a device", entry("dev", "", "3")],
  ])("rejects %s", async (_n, e) => {
    const d = dest();
    await expect(extractTarZst(tarZst(entry("ok.txt", "ok"), e), d)).rejects.toThrow();
    expect(existsSync(join(d, "..", "evil.txt"))).toBe(false);
  });
  test("caps extracted size", async () => {
    await expect(extractTarZst(tarZst(entry("big", Buffer.alloc(5000))), dest(), 1000)).rejects.toThrow();
  });
});

describe("report serving", () => {
  let snapDir: string;
  beforeAll(async () => {
    const v = await loadProject(conn.db, { id: "owner" }, "acme", "view");
    snapDir = v.snap?.dir as string;
  });
  test("lists only .html and .json under reports/", () => {
    expect(listReports(snapDir)).toEqual(["a.html", "data.json"]);
  });
  test("serves with the hardening headers", async () => {
    const r = await serveReport(snapDir, "a.html");
    expect(r.status).toBe(200);
    for (const [k, v] of Object.entries(REPORT_HEADERS)) expect(r.headers.get(k)).toBe(v);
    expect(r.headers.get("content-type")).toContain("text/html");
    expect(await r.text()).toBe("<h1>report</h1>");
    expect((await serveReport(snapDir, "data.json")).headers.get("content-type")).toBe("application/json");
  });
  test.each([
    "../project.db",
    "..%2Fproject.db",
    "reports/../project.db",
    "a.html/../../project.db",
    "/etc/passwd",
    "a.html\0.txt",
    "notes.txt",
    "project.db",
    "A.html",
    "",
  ])("path traversal / non-whitelisted name %j gives 404", async (name) => {
    expect((await serveReport(snapDir, name)).status).toBe(404);
  });
});

describe("snapshot queries", () => {
  let snapDir: string;
  beforeAll(async () => {
    snapDir = (await loadProject(conn.db, { id: "owner" }, "acme", "view")).snap?.dir as string;
  });
  test("overview", () => {
    const o = overview(snapDir);
    expect(o.stages).toMatchObject([{ stage: "analyse", endedAt: "2026-01-03", dryRun: false }]);
    expect(o.issueCounts).toEqual([{ kind: "broken-link", severity: "warning", n: 250 }]);
    expect(o.coverage).toEqual({ routes: 2, mapped: 1 });
  });
  test("issues are paged at 100", () => {
    expect(issuesPage(snapDir, 1, true)?.rows).toHaveLength(PAGE_SIZE);
    const last = issuesPage(snapDir, 3, true);
    expect(last).toMatchObject({ total: 250, pages: 3 });
    expect(last?.rows).toHaveLength(50);
  });
  test("share issue rows carry only kind, severity, item title (no message)", () => {
    const row = issuesPage(snapDir, 1, false)?.rows[0];
    expect(row).toEqual({
      kind: "broken-link",
      severity: "warning",
      itemTitle: "About us",
    });
    expect(JSON.stringify(issuesPage(snapDir, 1, false))).not.toContain("issue 0");
    expect(issuesPage(snapDir, 1, true)?.rows[0]).toMatchObject({ message: "issue 0" });
  });
  test("urls", () => {
    expect(urlsPage(snapDir, 1)?.rows).toEqual([
      { url: "/about", status: 200, target: "document" },
      { url: "/gone", status: 404, target: "none" },
    ]);
  });
  test("an older snapshot missing a table gives null, not a crash", () => {
    const d = mkdtempSync(join(work, "old-"));
    makeProjectDb(join(d, "project.db"), { noIssues: true });
    expect(issuesPage(d, 1, true)).toBeNull();
    expect(overview(d).issueCounts).toBeNull();
    expect(overview(d).stages).not.toBeNull();
    expect(urlsPage(d, 1)?.total).toBe(2);
  });
  test("a corrupt database propagates instead of reading as 'not available'", () => {
    const bad = mkdtempSync(join(work, "bad-"));
    writeFileSync(join(bad, "project.db"), "this is not a sqlite database, not even close".repeat(50));
    expect(() => overview(bad)).toThrow();
    expect(() => issuesPage(bad, 1, true)).toThrow();
  });
  test("no project.db yet (set up on the hub, never run) reads as nothing yet", () => {
    const o = overview(mkdtempSync(join(work, "none-")));
    expect(o.stages).toBeNull();
    expect(o.issueCounts).toBeNull();
  });
});

describe("client role sees no raw data", () => {
  test("client: overview/reports yes, issues/urls (view_raw) 403", async () => {
    expect(await status(loadProject(conn.db, { id: "client" }, "acme", "view"))).toBe(200);
    expect(await status(loadProject(conn.db, { id: "client" }, "acme", "view_raw"))).toBe(403);
    expect(await status(loadProject(conn.db, { id: "viewer" }, "acme", "view_raw"))).toBe(200);
    expect(await status(loadProject(conn.db, { id: "stranger" }, "acme", "view"))).toBe(404);
    expect(await status(loadProject(conn.db, { id: "stranger" }, "acme", "view_raw"))).toBe(404);
  });
});

describe("share links", () => {
  const create = async (as: string, body: unknown = { scope: "report" }) =>
    (await api.createShare(req(as, body), slug)) as Response;
  const mk = async (body: unknown = { scope: "report" }) =>
    (await (await create("dev", body)).json()) as { id: string; token: string; expiresAt: string };

  test("permissions: share needed; non-member 404; signed out 401", async () => {
    expect((await create("owner")).status).toBe(201);
    expect((await create("dev")).status).toBe(201);
    expect((await create("viewer")).status).toBe(403);
    expect((await create("client")).status).toBe(403);
    expect((await create("stranger")).status).toBe(404);
    expect((await api.createShare(req(null, { scope: "report" }), slug)).status).toBe(401);
    expect((await api.listShares(req("viewer"), slug)).status).toBe(403);
  });
  test("token is 32 random bytes base64url, default expiry 14 days, shown once", async () => {
    const l = await mk();
    expect(Buffer.from(l.token, "base64url")).toHaveLength(32);
    expect(l.token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    const days = (Date.parse(l.expiresAt) - Date.now()) / 86_400_000;
    expect(days).toBeGreaterThan(13.99);
    expect(days).toBeLessThan(14.01);
    const list = (await (await api.listShares(req("dev"), slug)).json()) as object[];
    expect(JSON.stringify(list)).not.toContain(l.token);
    expect(JSON.stringify(list)).not.toContain("token");
  });
  test("the token is not stored in plaintext", async () => {
    const l = await mk();
    const rows = await conn.db.select().from(shareLink);
    expect(rows.some((r) => JSON.stringify(r).includes(l.token))).toBe(false);
    expect(rows.find((r) => r.id === l.id)?.tokenHash).toBe(
      createHash("sha256").update(l.token).digest("hex"),
    );
    const audits = JSON.stringify(await conn.db.select().from(auditLog));
    expect(audits).not.toContain(l.token);
  });
  test("expiry is validated: 90 max, at least 1", async () => {
    expect((await create("dev", { scope: "report", days: 90 })).status).toBe(201);
    expect((await create("dev", { scope: "report", days: 91 })).status).toBe(400);
    expect((await create("dev", { scope: "report", days: 0 })).status).toBe(400);
    expect((await create("dev", { scope: "nope" })).status).toBe(400);
    expect((await create("dev", {})).status).toBe(400);
  });
  test("share link expires", async () => {
    const l = await mk();
    expect(await loadShare(conn.db, l.token, "index")).not.toBeNull();
    await conn.db
      .update(shareLink)
      .set({ expiresAt: new Date(Date.now() - 1000) })
      .where(eq(shareLink.id, l.id));
    expect(await loadShare(conn.db, l.token, "index")).toBeNull();
  });
  test("revoked link 404s, and revoke is audited", async () => {
    const l = await mk({ scope: "project-readonly" });
    expect(await loadShare(conn.db, l.token, "overview")).not.toBeNull();
    expect((await api.revokeShare(req("viewer"), { ...slug, id: l.id })).status).toBe(403);
    expect((await api.revokeShare(req("dev"), { ...slug, id: l.id })).status).toBe(200);
    expect(await loadShare(conn.db, l.token, "overview")).toBeNull();
    expect((await api.revokeShare(req("dev"), { ...slug, id: l.id })).status).toBe(404);
    const acts = (await conn.db.select().from(auditLog).where(eq(auditLog.targetId, l.id))).map(
      (a) => a.action,
    );
    expect(acts).toEqual(expect.arrayContaining(["share.create", "share.revoke"]));
  });
  test("unknown, expired and revoked tokens are indistinguishable", async () => {
    const a = await mk();
    const b = await mk();
    await conn.db
      .update(shareLink)
      .set({ expiresAt: new Date(Date.now() - 1000) })
      .where(eq(shareLink.id, a.id));
    await api.revokeShare(req("dev"), { ...slug, id: b.id });
    for (const t of [a.token, b.token, "nope", "x".repeat(500)])
      expect(await loadShare(conn.db, t, "index")).toBeNull();
  });
  test("report-scope link cannot reach issues or raw", async () => {
    const l = await mk({ scope: "report" });
    expect(await loadShare(conn.db, l.token, "index")).not.toBeNull();
    expect(await loadShare(conn.db, l.token, "report")).not.toBeNull();
    for (const p of ["overview", "issues", "urls"] as const)
      expect(await loadShare(conn.db, l.token, p)).toBeNull();
  });
  test("scope matrix", () => {
    const pages = ["index", "report", "overview", "issues", "urls"] as const;
    expect(pages.map((p) => sh.shareAllows("report", p))).toEqual([true, true, false, false, false]);
    expect(pages.map((p) => sh.shareAllows("project-readonly", p))).toEqual([true, true, true, true, true]);
  });
  test("a share never exposes field data, raw, submissions or the snapshot", async () => {
    const l = await mk({ scope: "project-readonly" });
    const v = await loadShare(conn.db, l.token, "issues");
    const dir = v?.snap?.dir as string;
    expect(JSON.stringify(issuesPage(dir, 1, false))).not.toMatch(/issue \d/); // no issue message in a share response
    const everything = JSON.stringify([
      overview(dir),
      issuesPage(dir, 1, false),
      urlsPage(dir, 1),
      listReports(dir),
    ]);
    expect(everything).not.toContain("field-data-must-not-leak");
    expect(everything).not.toContain("RAW-SUBMISSIONS");
    // Reports are whitelisted to reports/, so raw/ and the snapshot's other files are unreachable by name.
    for (const n of ["raw/dump.sql", "../raw/dump.sql", "project.db", "porter.toml"])
      expect((await serveReport(dir, n, SHARE_HEADERS)).status).toBe(404);
  });
  test("share headers are the required set", async () => {
    expect(SHARE_HEADERS).toEqual({
      "Referrer-Policy": "no-referrer",
      "X-Robots-Tag": "noindex",
      "Cache-Control": "private, no-store",
    });
    const r = await serveReport("/nonexistent", "a.html", SHARE_HEADERS);
    expect(r.headers.get("x-robots-tag")).toBe("noindex");
  });
  test("share config sets headers on /s/*", async () => {
    // @ts-expect-error plain .mjs config has no declaration file
    const cfg = (await import("../next.config.mjs")).default;
    const rules = await cfg.headers();
    const keys = Object.fromEntries(
      rules
        .find((r: { source: string }) => r.source === "/s/:path*")
        ?.headers.map((h: { key: string; value: string }) => [h.key, h.value]) ?? [],
    );
    expect(keys).toEqual(SHARE_HEADERS);
  });
  test("an archived project's links stop working", async () => {
    const l = await mk();
    await conn.db.update(project).set({ archivedAt: new Date() }).where(eq(project.id, pid));
    expect(await loadShare(conn.db, l.token, "index")).toBeNull();
    await conn.db.update(project).set({ archivedAt: null }).where(eq(project.id, pid));
  });
  test("the project with no version yet loads with snap null", async () => {
    const [p2] = await conn.db
      .insert(project)
      .values({ orgId: "umi-org", name: "E", slug: "empty" })
      .returning();
    await conn.db.insert(projectMember).values({ projectId: p2.id, userId: "owner", role: "owner" });
    expect((await loadProject(conn.db, { id: "owner" }, "empty", "view")).snap).toBeNull();
  });
});

test("the cache is reused: second load returns the same extracted dir", async () => {
  const a = await loadProject(conn.db, { id: "owner" }, "acme", "view");
  const b = await loadProject(conn.db, { id: "dev" }, "acme", "view");
  expect(a.snap?.dir).toBe(b.snap?.dir);
});
