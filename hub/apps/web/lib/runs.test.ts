import { createHash, randomBytes, randomUUID } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import {
  auditLog,
  connect,
  organization,
  project,
  projectMember,
  projectVersion,
  run,
  runEvent,
  runSecret,
  user,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { decryptSecret } from "@porter-hub/porter/secrets";
import { keys, putFile } from "@porter-hub/storage/s3";
import { packTarZst } from "@porter-hub/storage/tar";
import { and, eq, sql } from "drizzle-orm";
import { afterAll, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { makeApi } from "./api";
import { createListener, type Listener } from "./listen";

process.env.HUB_SECRET_KEY = randomBytes(32).toString("base64");

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let listener: Listener;
let api: ReturnType<typeof makeApi>;
let pid: string;
let emptyPid: string;
const USERS = ["owner", "dev", "viewer", "client", "stranger"];
const TOML = `[project]\nname = "x"\n[destination]\nadapter = "payload"\nurl = "https://CMS.example.com:8443/admin"\n`;

const req = (as: string | null, body?: unknown, method = body === undefined ? "GET" : "POST", headers = {}) =>
  new Request("http://hub.test/x", {
    method,
    headers: { ...(as ? { "x-user": as } : {}), "content-type": "application/json", ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
const slug = { slug: "acme" };
// biome-ignore lint/suspicious/noExplicitAny: test JSON
const json = async (r: Response) => (await r.json()) as Record<string, any>;
let openErrors: number | null = 0;
let dryRun = true;
const clearRuns = () => conn.db.delete(run).where(eq(run.projectId, pid));
const notify = (id: string) => conn.db.execute(sql`select pg_notify(${`run_${id}`}, 'x')`);

beforeAll(async () => {
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  listener = createListener(fresh.url);
  const now = new Date();
  for (const id of USERS)
    await conn.db
      .insert(user)
      .values({ id, name: id, email: `${id}@x.test`, emailVerified: true, createdAt: now, updatedAt: now });
  await conn.db.insert(organization).values({ id: "org", name: "Umi", slug: "umi", createdAt: now });
  const [p, e] = await conn.db
    .insert(project)
    .values([
      { orgId: "org", name: "Acme", slug: "acme" },
      { orgId: "org", name: "Empty", slug: "empty" },
    ])
    .returning();
  pid = p.id;
  emptyPid = e.id;
  await conn.db.insert(projectMember).values([
    { projectId: pid, userId: "owner", role: "owner" },
    { projectId: pid, userId: "dev", role: "developer" },
    { projectId: pid, userId: "viewer", role: "viewer" },
    { projectId: pid, userId: "client", role: "client" },
    { projectId: emptyPid, userId: "dev", role: "developer" },
  ]);
  await conn.db
    .insert(projectVersion)
    .values({ projectId: pid, number: 1, snapshotKey: "projects/x/snapshots/v1.tar.zst" });
  api = makeApi(
    conn.db,
    async (r) => (r.headers.get("x-user") ? { id: r.headers.get("x-user") as string } : null),
    {
      listener: () => listener,
      readToml: async () => TOML,
      applyFacts: async () => ({ openErrors, dryRun }),
    },
  );
});
afterAll(async () => {
  await listener?.close();
  await conn?.client.end();
  await fresh?.drop();
});
beforeEach(clearRuns);

describe("permissions", () => {
  test("run, cancel need `run`; list, get, events need `view_raw`", async () => {
    const statuses = async (fn: (as: string | null) => Promise<Response>) => {
      const out: Record<string, number> = {};
      for (const as of [null, ...USERS]) {
        await clearRuns();
        out[String(as)] = (await fn(as)).status;
      }
      return out;
    };
    expect(await statuses((as) => api.createRun(req(as, { stages: ["analyse"] }), slug))).toEqual({
      null: 401,
      owner: 201,
      dev: 201,
      viewer: 403,
      client: 403,
      stranger: 404,
    });
    expect(await statuses((as) => api.listRuns(req(as), slug))).toEqual({
      null: 401,
      owner: 200,
      dev: 200,
      viewer: 200,
      client: 403,
      stranger: 404,
    });
    const queued = async () =>
      (
        await conn.db
          .insert(run)
          .values({ projectId: pid, stages: ["analyse"], state: "finished" })
          .returning()
      )[0].id;
    expect(await statuses(async (as) => api.getRun(req(as), { ...slug, id: await queued() }))).toMatchObject({
      null: 401,
      viewer: 200,
      client: 403,
      stranger: 404,
    });
    expect(
      await statuses(async (as) => api.runEvents(req(as), { ...slug, id: await queued() })),
    ).toMatchObject({
      null: 401,
      viewer: 200,
      client: 403,
      stranger: 404,
    });
    const active = async () =>
      (
        await conn.db
          .insert(run)
          .values({ projectId: pid, stages: ["analyse"] })
          .returning()
      )[0].id;
    expect(await statuses(async (as) => api.cancelRun(req(as, {}), { ...slug, id: await active() }))).toEqual(
      {
        null: 401,
        owner: 200,
        dev: 200,
        viewer: 403,
        client: 403,
        stranger: 404,
      },
    );
  });

  test("secrets need `manage_secrets`", async () => {
    const s = { ...slug, name: "API_KEY" };
    for (const [as, code] of [
      [null, 401],
      ["viewer", 403],
      ["client", 403],
      ["stranger", 404],
      ["dev", 200],
    ] as const) {
      expect((await api.putSecret(req(as, { value: "v" }, "PUT"), s)).status).toBe(code);
      expect((await api.listSecrets(req(as), slug)).status).toBe(code);
    }
    expect((await api.deleteSecret(req("viewer", undefined, "DELETE"), s)).status).toBe(403);
    expect((await api.deleteSecret(req("owner", undefined, "DELETE"), s)).status).toBe(200);
  });
});

describe("run requests", () => {
  test("queued on the latest version; stages kept in pipeline order; audited", async () => {
    const r = await api.createRun(req("dev", { stages: ["verify", "analyse"] }), slug);
    expect(r.status).toBe(201);
    const body = await json(r);
    expect(body).toMatchObject({
      state: "queued",
      stages: ["analyse", "verify"],
      apply: false,
      base_version: 1,
    });
    const [a] = await conn.db.select().from(auditLog).where(eq(auditLog.targetId, body.id));
    expect(a.action).toBe("run.create");
    expect((await json(await api.getRun(req("viewer"), { ...slug, id: body.id }))).id).toBe(body.id);
    expect((await api.getRun(req("viewer"), { ...slug, id: "nope" })).status).toBe(400);
  });

  test("bad bodies are 400; no version is 409", async () => {
    for (const b of [
      {},
      { stages: [] },
      { stages: ["crawl"] },
      { stages: "everything" },
      { stages: "all", apply: "yes" },
    ])
      expect((await api.createRun(req("dev", b), slug)).status).toBe(400);
    const r = await api.createRun(req("dev", { stages: "all" }), { slug: "empty" });
    expect(r.status).toBe(409);
    expect((await json(r)).error).toMatch(/no version/);
  });

  test("apply requires the matching confirm_host", async () => {
    const go = (b: object) => api.createRun(req("dev", b), slug);
    expect((await go({ stages: ["load"], apply: true })).status).toBe(400);
    expect((await go({ stages: ["load"], apply: true, confirm_host: "evil.example.com" })).status).toBe(400);
    expect((await go({ stages: ["load"], apply: true, confirm_host: "cms.example.com" })).status).toBe(400); // port is part of the host
    expect(
      (await go({ stages: ["analyse"], apply: true, confirm_host: "cms.example.com:8443" })).status,
    ).toBe(400);
    const ok = await go({ stages: ["load"], apply: true, confirm_host: "cms.example.com:8443" });
    expect(ok.status).toBe(201);
    expect((await json(ok)).apply).toBe(true);
    await clearRuns();
    const noDest = makeApi(conn.db, async (r) => ({ id: r.headers.get("x-user") as string }), {
      readToml: async () => `[project]\nname = "x"\n`,
    });
    expect(
      (await noDest.createRun(req("dev", { stages: ["load"], apply: true, confirm_host: "x" }), slug)).status,
    ).toBe(409);
    // Without apply, load is a dry run and needs no host.
    expect((await go({ stages: ["load"] })).status).toBe(201);
  });

  test("apply with all stages is refused: porter run --apply skips the dry-run check", async () => {
    const r = await api.createRun(
      req("dev", { stages: "all", apply: true, confirm_host: "CMS.EXAMPLE.COM:8443" }),
      slug,
    );
    expect(r.status).toBe(400);
    expect((await json(r)).error).toBe("Apply load on its own after a dry run");
    expect(await conn.db.select().from(run).where(eq(run.projectId, pid))).toHaveLength(0);
    // All stages without apply is still fine (load runs as a dry run).
    expect((await api.createRun(req("dev", { stages: "all" }), slug)).status).toBe(201);
  });

  test("apply is refused while the version has open errors, giving the count", async () => {
    const go = () =>
      api.createRun(
        req("dev", { stages: ["load"], apply: true, confirm_host: "cms.example.com:8443" }),
        slug,
      );
    try {
      for (const [n, msg] of [
        [1, "v1 has 1 open error; fix them and push before applying"],
        [3, "v1 has 3 open errors; fix them and push before applying"],
      ] as const) {
        openErrors = n;
        const r = await go();
        expect(r.status).toBe(400);
        expect((await json(r)).error).toBe(msg);
      }
      expect(await conn.db.select().from(run).where(eq(run.projectId, pid))).toHaveLength(0);
      // A dry run is not gated: it is how the errors get found.
      expect((await api.createRun(req("dev", { stages: ["load"] }), slug)).status).toBe(201);
      await clearRuns();
      openErrors = null; // an older snapshot without an issues table
      expect((await go()).status).toBe(201);
    } finally {
      openErrors = 0;
    }
  });

  test("apply with a stage before load is refused up front; load (+ verify) alone is accepted", async () => {
    const go = (stages: string[] | "all") =>
      api.createRun(req("dev", { stages, apply: true, confirm_host: "cms.example.com:8443" }), slug);
    for (const up of ["analyse", "extract", "normalise", "schema-check"]) {
      const r = await go([up, "load"]);
      expect(r.status).toBe(400);
      expect((await json(r)).error).toBe(
        "apply with upstream stages would be refused at load (they re-run after the dry run); run them as a dry run first, then apply load alone",
      );
    }
    expect(await conn.db.select().from(run).where(eq(run.projectId, pid))).toHaveLength(0);
    expect((await go(["load", "verify"])).status).toBe(201);
    await clearRuns();
  });

  test("apply is refused until a dry-run load is on record", async () => {
    dryRun = false;
    try {
      const r = await api.createRun(
        req("dev", { stages: ["load"], apply: true, confirm_host: "cms.example.com:8443" }),
        slug,
      );
      expect(r.status).toBe(400);
      expect((await json(r)).error).toBe("Run a dry-run load first: v1 has none on record");
      expect(await conn.db.select().from(run).where(eq(run.projectId, pid))).toHaveLength(0);
      expect((await api.createRun(req("dev", { stages: ["load"] }), slug)).status).toBe(201);
    } finally {
      dryRun = true;
    }
  });

  test("a run on an archived project can still be cancelled", async () => {
    const q = await json(await api.createRun(req("dev", { stages: ["analyse"] }), slug));
    await conn.db.update(project).set({ archivedAt: new Date() }).where(eq(project.id, pid));
    try {
      const r = await api.cancelRun(req("dev", {}), { ...slug, id: q.id });
      expect(r.status).toBe(200);
      expect((await json(r)).state).toBe("cancelled");
    } finally {
      await conn.db.update(project).set({ archivedAt: null }).where(eq(project.id, pid));
    }
  });

  test("an archived project takes no new runs (409)", async () => {
    await conn.db.update(project).set({ archivedAt: new Date() }).where(eq(project.id, pid));
    try {
      const r = await api.createRun(req("dev", { stages: ["analyse"] }), slug);
      expect(r.status).toBe(409);
      expect((await json(r)).error).toBe("The project is archived");
    } finally {
      await conn.db.update(project).set({ archivedAt: null }).where(eq(project.id, pid));
    }
  });

  test("apply reads the destination host from the version's real snapshot", async () => {
    const dir = mkdtempSync(join(tmpdir(), "runs-snap-"));
    writeFileSync(join(dir, "porter.toml"), TOML);
    const pdb = new DatabaseSync(join(dir, "project.db"));
    pdb.exec(
      "CREATE TABLE runs (id TEXT, stage TEXT, dry_run INTEGER, ended_at TEXT, deleted_at TEXT); INSERT INTO runs VALUES ('1', 'load', 1, 'now', NULL)",
    );
    pdb.close();
    const out = join(dir, "..", `${randomUUID()}.tar.zst`);
    await packTarZst(dir, out);
    const sha = createHash("sha256").update(readFileSync(out)).digest("hex");
    const [p] = await conn.db
      .insert(project)
      .values({ orgId: "org", name: "Real", slug: "real" })
      .returning();
    await conn.db.insert(projectMember).values({ projectId: p.id, userId: "dev", role: "developer" });
    await putFile(keys.snapshot(p.id, "v1"), out, sha, statSync(out).size);
    await conn.db.insert(projectVersion).values({
      projectId: p.id,
      number: 1,
      snapshotKey: keys.snapshot(p.id, "v1"),
      snapshotSha256: sha,
    });
    // No readToml injected: versionDir extracts the snapshot and reads porter.toml.
    const real = makeApi(conn.db, async (r) => ({ id: r.headers.get("x-user") as string }));
    const go = (host: string) =>
      real.createRun(req("dev", { stages: ["load"], apply: true, confirm_host: host }), { slug: "real" });
    expect((await go("cms.example.com")).status).toBe(400);
    expect((await go("cms.example.com:8443")).status).toBe(201);
    rmSync(dir, { recursive: true, force: true });
    rmSync(out, { force: true });
  });

  test("one run per project: API 409 and the database index", async () => {
    const first = await json(await api.createRun(req("dev", { stages: "all" }), slug));
    const again = await api.createRun(req("owner", { stages: ["analyse"] }), slug);
    expect(again.status).toBe(409);
    expect((await json(again)).run_id).toBe(first.id);
    await expect(conn.db.insert(run).values({ projectId: pid, stages: ["analyse"] })).rejects.toThrow();
    await conn.db.update(run).set({ state: "running" }).where(eq(run.id, first.id));
    await expect(conn.db.insert(run).values({ projectId: pid, stages: ["analyse"] })).rejects.toThrow();
    await conn.db.update(run).set({ state: "finished" }).where(eq(run.id, first.id));
    expect((await api.createRun(req("owner", { stages: ["analyse"] }), slug)).status).toBe(201);
  });

  test("cancel: queued is cancelled at once, running is flagged, finished is 409", async () => {
    const q = await json(await api.createRun(req("dev", { stages: "all" }), slug));
    const c = await json(await api.cancelRun(req("dev", {}), { ...slug, id: q.id }));
    expect(c).toMatchObject({ state: "cancelled", cancel_requested: true });
    expect((await api.cancelRun(req("dev", {}), { ...slug, id: q.id })).status).toBe(409);
    const r = await json(await api.createRun(req("dev", { stages: "all" }), slug));
    await conn.db.update(run).set({ state: "running" }).where(eq(run.id, r.id));
    expect(await json(await api.cancelRun(req("owner", {}), { ...slug, id: r.id }))).toMatchObject({
      state: "running",
      cancel_requested: true,
    });
    const audits = await conn.db.select().from(auditLog).where(eq(auditLog.action, "run.cancel"));
    expect(audits.length).toBeGreaterThanOrEqual(2);
  });
});

describe("live events (SSE)", () => {
  /** Reads SSE messages until `n` have arrived (or the stream ends). */
  async function reader(res: Response) {
    const rd = (res.body as ReadableStream<Uint8Array>).getReader();
    const dec = new TextDecoder();
    let buf = "";
    let ended = false;
    const msgs: { id?: string; event?: string; data?: string }[] = [];
    const take = async (n: number) => {
      while (msgs.filter((m) => m.data).length < n && !ended) {
        const { value, done } = await rd.read();
        if (done) {
          ended = true;
          break;
        }
        buf += dec.decode(value);
        let i = buf.indexOf("\n\n");
        while (i >= 0) {
          const m: Record<string, string> = {};
          for (const line of buf.slice(0, i).split("\n")) {
            const k = line.indexOf(":");
            if (k > 0) m[line.slice(0, k)] = line.slice(k + 1).trim();
          }
          if (m.data) msgs.push(m);
          buf = buf.slice(i + 2);
          i = buf.indexOf("\n\n");
        }
      }
      return msgs.splice(0, n);
    };
    return { take, ended: () => ended, cancel: () => rd.cancel() };
  }

  test("replays after Last-Event-ID in seq order, streams new events, closes on a final state", async () => {
    const [r] = await conn.db
      .insert(run)
      .values({ projectId: pid, stages: ["analyse"], state: "running" })
      .returning();
    // Inserted out of order on purpose: the stream orders by seq.
    for (const seq of [4, 1, 5, 3, 2])
      await conn.db.insert(runEvent).values({ runId: r.id, seq, event: { v: 1, event: "progress", n: seq } });
    const res = await api.runEvents(req("viewer", undefined, "GET", { "last-event-id": "2" }), {
      ...slug,
      id: r.id,
    });
    expect(res.headers.get("content-type")).toContain("text/event-stream");
    const sse = await reader(res);
    expect((await sse.take(3)).map((m) => [m.id, JSON.parse(m.data as string).n])).toEqual([
      ["3", 3],
      ["4", 4],
      ["5", 5],
    ]);
    await conn.db.insert(runEvent).values({ runId: r.id, seq: 6, event: { v: 1, event: "progress", n: 6 } });
    await notify(r.id);
    expect((await sse.take(1)).map((m) => m.id)).toEqual(["6"]);
    await conn.db.update(run).set({ state: "finished", exitCode: 0 }).where(eq(run.id, r.id));
    await notify(r.id);
    const [end] = await sse.take(1);
    expect(end.event).toBe("end");
    expect(JSON.parse(end.data as string)).toMatchObject({ id: r.id, state: "finished", exit_code: 0 });
    await sse.take(1);
    expect(sse.ended()).toBe(true);

    // ?after= on a finished run: the rest, then end, then closed.
    const again = await reader(
      await api.runEvents(new Request(`http://hub.test/x?after=4`, { headers: { "x-user": "dev" } }), {
        ...slug,
        id: r.id,
      }),
    );
    const rest = await again.take(3);
    expect(rest.map((m) => m.id ?? m.event)).toEqual(["5", "6", "end"]);
  });

  test("unknown run is 404; another project's run is 404", async () => {
    expect((await api.runEvents(req("dev"), { ...slug, id: crypto.randomUUID() })).status).toBe(404);
    const [other] = await conn.db
      .insert(run)
      .values({ projectId: emptyPid, stages: ["analyse"] })
      .returning();
    expect((await api.runEvents(req("dev"), { ...slug, id: other.id })).status).toBe(404);
  });
});

describe("secrets", () => {
  test("write-only: names come back, values never; stored encrypted", async () => {
    const value = "sk_live_very_secret_value";
    const put = await api.putSecret(req("dev", { value }, "PUT"), {
      ...slug,
      name: "PORTER_PAYLOAD_API_KEY",
    });
    expect(put.status).toBe(200);
    expect(await put.text()).not.toContain(value);
    const list = await api.listSecrets(req("owner"), slug);
    const text = await list.text();
    expect(text).not.toContain(value);
    expect(JSON.parse(text)).toEqual([
      expect.objectContaining({ name: "PORTER_PAYLOAD_API_KEY", set: true }),
    ]);
    const [row] = await conn.db
      .select()
      .from(runSecret)
      .where(and(eq(runSecret.projectId, pid), eq(runSecret.name, "PORTER_PAYLOAD_API_KEY")));
    expect(row.ciphertext).not.toContain(value);
    expect(decryptSecret(pid, row.name, row)).toBe(value);
    // Bound to its project and name.
    expect(() => decryptSecret(emptyPid, row.name, row)).toThrow();
    expect(() => decryptSecret(pid, "OTHER", row)).toThrow();
    // Replace.
    await api.putSecret(req("dev", { value: "v2" }, "PUT"), { ...slug, name: "PORTER_PAYLOAD_API_KEY" });
    const [row2] = await conn.db.select().from(runSecret).where(eq(runSecret.id, row.id));
    expect(decryptSecret(pid, row2.name, row2)).toBe("v2");
    const audits = await conn.db
      .select()
      .from(auditLog)
      .where(eq(auditLog.targetId, "PORTER_PAYLOAD_API_KEY"));
    expect(JSON.stringify(audits)).not.toContain(value);
    expect(
      (await api.deleteSecret(req("dev", undefined, "DELETE"), { ...slug, name: "PORTER_PAYLOAD_API_KEY" }))
        .status,
    ).toBe(200);
    expect(
      (await api.deleteSecret(req("dev", undefined, "DELETE"), { ...slug, name: "PORTER_PAYLOAD_API_KEY" }))
        .status,
    ).toBe(404);
  });

  test("names must be env-var shaped and not reserved; values non-empty", async () => {
    for (const name of [
      "lower",
      "1ABC",
      "A-B",
      `A${"B".repeat(64)}`,
      "PATH",
      "HOME",
      "LANG",
      "NODE_OPTIONS",
      "LD_PRELOAD",
      "DYLD_INSERT_LIBRARIES",
      "HTTPS_PROXY",
      // Exact names (one per rule added in fix round 1).
      "SSLKEYLOGFILE",
      "OPENSSL_CONF",
      "GCONV_PATH",
      "GLIBC_TUNABLES",
      "HOSTALIASES",
      "RES_OPTIONS",
      "LOCALDOMAIN",
      "LOCPATH",
      "NLSPATH",
      "MALLOC_CHECK_",
      "ZDOTDIR",
      // Prefixes.
      "SSL_CERT_FILE",
      "OPENSSL_MODULES",
      "GCONV_X",
      "GLIBC_X",
      "GIT_SSH_COMMAND",
      "PYTHONPATH",
      "PERL5LIB",
      "RUBYOPT",
      "JAVA_TOOL_OPTIONS",
      "CURL_CA_BUNDLE",
      "REQUESTS_CA_BUNDLE",
      // The _PROXY suffix.
      "FTP_PROXY",
      "SOCKS_PROXY",
    ])
      expect((await api.putSecret(req("dev", { value: "v" }, "PUT"), { ...slug, name })).status).toBe(400);
    // Ordinary credential names stay allowed.
    for (const name of ["PORTER_WP_PASSWORD", "MY_API_KEY", "PROXY_TOKEN"])
      expect((await api.putSecret(req("dev", { value: "v" }, "PUT"), { ...slug, name })).status).toBe(200);
    for (const body of [{}, { value: "" }, { value: 5 }, { value: "a\0b" }])
      expect((await api.putSecret(req("dev", body, "PUT"), { ...slug, name: "OK_NAME" })).status).toBe(400);
  });
});
