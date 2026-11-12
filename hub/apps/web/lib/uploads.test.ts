import { createHash, randomBytes, randomUUID } from "node:crypto";
import {
  auditLog,
  connect,
  member,
  organization,
  project,
  projectMember,
  upload,
  user,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { verifyUpload } from "@porter-hub/storage";
import { eq } from "drizzle-orm";
import { afterAll, afterEach, beforeAll, describe, expect, test, vi } from "vitest";
import { makeApi } from "./api";
import { sha256Blob } from "./hash-stream";
import * as store from "./storage";
import { hexToBase64 } from "./storage";
import { MAX_OPEN_UPLOADS, PART_SIZE, sanitizeFilename } from "./uploads";

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let api: ReturnType<typeof makeApi>;
let pid: string;
let betaPid: string;

const USERS = ["owner", "dev", "viewer", "client", "client2", "stranger"];
const req = (as: string | null, body?: unknown) =>
  new Request("http://hub.test/x", {
    method: body === undefined ? "GET" : "POST",
    headers: { ...(as ? { "x-user": as } : {}), "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
const slug = { slug: "acme" };
const sha = (b: Buffer) => createHash("sha256").update(b).digest("hex");
const json = async (r: Response) => (await r.json()) as Record<string, any>;
const idp = (id: string) => ({ ...slug, id });

const create = (as: string, body: object) => api.createUpload(req(as, body), slug);
const partBody = (as: string, parts: { part_number: number; sha256: string }[]) => req(as, { parts });
/** Presigns part n for `data`'s sha256 (the way the browser does), then PUTs `body` (default: data) to it. */
const putPartRaw = async (as: string, id: string, n: number, data: Buffer, body = data, headers = true) => {
  const { urls } = await json(
    await api.uploadPartUrls(partBody(as, [{ part_number: n, sha256: sha(data) }]), idp(id)),
  );
  return fetch(urls[n], {
    method: "PUT",
    body: new Uint8Array(body),
    headers: headers ? { "x-amz-checksum-sha256": hexToBase64(sha(data)) } : {},
  });
};
const putPart = async (as: string, id: string, n: number, data: Buffer) => {
  const r = await putPartRaw(as, id, n, data);
  expect(r.status).toBe(200);
  expect(r.headers.get("etag")).toBeTruthy();
};
const settled = async (id: string, states: string[]) => {
  for (let i = 0; i < 100; i++) {
    const [u] = await conn.db.select().from(upload).where(eq(upload.id, id));
    if (states.includes(u.state)) return u;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`upload ${id} never reached ${states}`);
};
const actions = async (id: string) =>
  (await conn.db.select().from(auditLog).where(eq(auditLog.targetId, id))).map((a) => a.action);

/** 20 MiB: two parts (16 MiB + 4 MiB). */
const data = randomBytes(PART_SIZE + 4 * 1024 * 1024);
const parts = [data.subarray(0, PART_SIZE), data.subarray(PART_SIZE)];
/** Uploads every part and completes with `hash`. */
async function full(as: string, filename = "site.sql.gz", purpose = "dump", hash = sha(data)) {
  const cr = await create(as, { filename, size: data.length, purpose });
  expect(cr.status).toBe(201);
  const c = await json(cr);
  for (const [i, p] of parts.entries()) await putPart(as, c.id, i + 1, p);
  const done = await api.completeUpload(req(as, { sha256: hash }), idp(c.id));
  expect(done.status).toBe(202);
  return { id: c.id as string, done };
}

beforeAll(async () => {
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
  const [p, bp] = await conn.db
    .insert(project)
    .values([
      { orgId: "umi-org", name: "Acme", slug: "acme" },
      { orgId: "umi-org", name: "Beta", slug: "beta" },
    ])
    .returning();
  pid = p.id;
  betaPid = bp.id;
  await conn.db.insert(projectMember).values([
    { projectId: pid, userId: "owner", role: "owner" },
    { projectId: betaPid, userId: "owner", role: "owner" },
    { projectId: pid, userId: "dev", role: "developer" },
    { projectId: pid, userId: "viewer", role: "viewer" },
    { projectId: pid, userId: "client", role: "client" },
    { projectId: pid, userId: "client2", role: "client" },
  ]);
  api = makeApi(conn.db, async (r) => {
    const id = r.headers.get("x-user");
    return id ? { id } : null;
  });
});
afterEach(async () => {
  // Open uploads count against a per-user cap; don't let one test's leftovers break the next.
  await conn.db.update(upload).set({ state: "aborted" }).where(eq(upload.state, "uploading"));
});
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
});

describe("permissions", () => {
  const routes: [string, string[], (as: string | null) => Promise<Response>][] = [
    ["create", ["viewer"], (a) => create(a as string, { filename: "a.sql", size: 1, purpose: "dump" })],
    ["list", [], (a) => api.listUploads(req(a), slug)],
    ["get", [], (a) => api.getUpload(req(a), idp(randomUUID()))],
    [
      "parts",
      ["viewer"],
      (a) =>
        api.uploadPartUrls(req(a, { parts: [{ part_number: 1, sha256: sha(data) }] }), idp(randomUUID())),
    ],
    [
      "complete",
      ["viewer"],
      (a) => api.completeUpload(req(a, { sha256: "0".repeat(64) }), idp(randomUUID())),
    ],
    ["abort", ["viewer"], (a) => api.abortUpload(req(a, {}), idp(randomUUID()))],
    ["download", ["client", "viewer"], (a) => api.downloadUpload(req(a), idp(randomUUID()))],
  ];
  for (const [name, forbidden, call] of routes) {
    test(`${name}: 401 signed out, 404 non-member, 403 for ${forbidden.join("/") || "nobody"}`, async () => {
      if (name === "create") {
        expect((await api.createUpload(req(null, {}), slug)).status).toBe(401);
        expect((await create("stranger", { filename: "a.sql", size: 1, purpose: "dump" })).status).toBe(404);
      } else {
        expect((await call(null)).status).toBe(401);
        expect((await call("stranger")).status).toBe(404);
      }
      for (const r of forbidden) expect((await call(r)).status).toBe(403);
    });
  }
});

describe("create validation", () => {
  const cases: [string, string, number, number][] = [
    ["dump", "a.sql", 1, 201],
    ["dump", "A.SQL.GZ", 1, 201],
    ["dump", "a.tar.gz", 1, 201],
    ["dump", "a.xml", 1, 400],
    ["dump", "a.tar", 1, 400],
    ["wxr", "export.xml", 1, 201],
    ["wxr", "export.xml.gz", 1, 201],
    ["wxr", "export.sql", 1, 400],
    ["media-archive", "uploads.zip", 1, 201],
    ["media-archive", "uploads.tar", 1, 201],
    ["media-archive", "uploads.xml", 1, 400],
    ["other", "anything.exe", 1, 201],
  ];
  test.each(cases)("%s accepts by suffix: %s -> %i", async (purpose, filename, size, status) => {
    const r = await create("client", { filename, size, purpose });
    expect(r.status).toBe(status === 201 ? 201 : 400);
    if (status === 201) await api.abortUpload(req("client", {}), idp((await json(r)).id));
  });
  test("unknown purpose, zero size and bad json are 400", async () => {
    expect((await create("client", { filename: "a.sql", size: 1, purpose: "nope" })).status).toBe(400);
    expect((await create("client", { filename: "a.sql", size: 0, purpose: "dump" })).status).toBe(400);
    expect((await create("client", { filename: "a.sql", size: 1.5, purpose: "dump" })).status).toBe(400);
  });
  test("size over UPLOAD_MAX_BYTES is 413", async () => {
    process.env.UPLOAD_MAX_BYTES = "1000";
    try {
      expect((await create("client", { filename: "a.sql", size: 1001, purpose: "dump" })).status).toBe(413);
      const ok = await create("client", { filename: "a.sql", size: 1000, purpose: "dump" });
      expect(ok.status).toBe(201);
      await api.abortUpload(req("client", {}), idp((await json(ok)).id));
    } finally {
      delete process.env.UPLOAD_MAX_BYTES;
    }
  });
  test("20 GiB default limit", async () => {
    expect(
      (await create("client", { filename: "a.sql", size: 20 * 1024 ** 3 + 1, purpose: "dump" })).status,
    ).toBe(413);
  });
});

describe("filenames", () => {
  test("sanitised for metadata and never part of the object key", async () => {
    const evil = "../../etc/pa\\ss\u0000wd\n\u007f.sql";
    expect(sanitizeFilename(evil)).toBe("....etcpasswd.sql");
    expect(sanitizeFilename(`${"x".repeat(300)}.sql`)).toHaveLength(255);
    const r = await create("client", { filename: "secret-client-name.sql", size: 1, purpose: "dump" });
    const id = (await json(r)).id as string;
    const [u] = await conn.db.select().from(upload).where(eq(upload.id, id));
    expect(u.objectKey).toBe(`projects/${pid}/uploads/${id}`);
    expect(u.objectKey).not.toContain("secret");
    expect(u.filename).toBe("secret-client-name.sql");
    const e = await json(await create("client", { filename: evil, size: 1, purpose: "dump" }));
    const [e2] = await conn.db.select().from(upload).where(eq(upload.id, e.id));
    expect(e2.filename).toBe("....etcpasswd.sql");
    expect(e2.objectKey).not.toContain("passwd");
    expect((await create("client", { filename: "///", size: 1, purpose: "other" })).status).toBe(400);
  });
});

describe("hashing helper", () => {
  test("streams a Blob to the same sha256 as node:crypto, reporting progress", async () => {
    const seen: number[] = [];
    const h = await sha256Blob(new Blob([new Uint8Array(data)]), (n) => seen.push(n));
    expect(h).toBe(sha(data));
    expect(seen.at(-1)).toBe(data.length);
    expect(await sha256Blob(new Blob([]))).toBe(sha(Buffer.alloc(0)));
  });
});

describe("multipart flow", () => {
  test("resume after dropped part", async () => {
    const c = await json(
      await create("client", { filename: "site.sql", size: data.length, purpose: "dump" }),
    );
    expect(c.part_size).toBe(PART_SIZE);
    expect(c.part_count).toBe(2);
    await putPart("client", c.id, 1, parts[0]);
    // The connection dropped before part 2. The resuming client asks what is already there.
    const g = await json(await api.getUpload(req("client"), idp(c.id)));
    expect(g.state).toBe("uploading");
    expect(g.parts).toEqual([{ part_number: 1, size: PART_SIZE }]);
    // Completing now is refused, and names the part to send.
    const early = await api.completeUpload(req("client", { sha256: sha(data) }), idp(c.id));
    expect(early.status).toBe(409);
    expect((await json(early)).missing).toEqual([2]);
    expect((await settledState(c.id)).state).toBe("uploading");
    await putPart("client", c.id, 2, parts[1]);
    const done = await api.completeUpload(req("client", { sha256: sha(data) }), idp(c.id));
    expect(done.status).toBe(202);
    expect((await json(done)).state).toBe("verifying");
    expect((await settled(c.id, ["ready"])).state).toBe("ready");
    expect(await actions(c.id)).toEqual(
      expect.arrayContaining(["upload.create", "upload.complete", "upload.ready"]),
    );
  });

  test("part urls: batches of at most 100, within the part count, sha256 required", async () => {
    const c = await json(await create("client", { filename: "a.sql", size: data.length, purpose: "dump" }));
    const ask = (b: object) => api.uploadPartUrls(req("client", b), idp(c.id));
    const part = (n: number) => ({ part_number: n, sha256: sha(data) });
    expect((await ask({ parts: Array.from({ length: 101 }, (_, i) => part(i + 1)) })).status).toBe(400);
    expect((await ask({ parts: [part(3)] })).status).toBe(400);
    expect((await ask({ parts: [] })).status).toBe(400);
    expect((await ask({ part_numbers: [1] })).status).toBe(400); // checksums are mandatory
    expect((await ask({ parts: [{ part_number: 1, sha256: "nope" }] })).status).toBe(400);
    const ok = await json(await ask({ parts: [part(1), part(2)] }));
    expect(Object.keys(ok.urls)).toEqual(["1", "2"]);
    expect(ok.urls[1]).toContain("x-amz-checksum-sha256"); // signed header
  });

  test("a corrupted part is rejected by storage (per-part sha256)", async () => {
    const c = await json(await create("client", { filename: "a.sql", size: data.length, purpose: "dump" }));
    const bad = Buffer.from(parts[0]);
    bad[1234] ^= 0xff; // same length, one byte off
    expect((await putPartRaw("client", c.id, 1, parts[0], bad)).status).toBe(400);
    // No checksum header at all is refused too: the header is signed.
    expect((await putPartRaw("client", c.id, 1, parts[0], parts[0], false)).status).toBeGreaterThanOrEqual(
      400,
    );
    // A part of another length is refused: Content-Length is signed.
    expect(
      (await putPartRaw("client", c.id, 1, parts[0], parts[0].subarray(0, 1000))).status,
    ).toBeGreaterThanOrEqual(400);
    const g = await json(await api.getUpload(req("client"), idp(c.id)));
    expect(g.parts).toEqual([]);
    // The right bytes are accepted, and the retry overwrites nothing bad.
    await putPart("client", c.id, 1, parts[0]);
    expect((await json(await api.getUpload(req("client"), idp(c.id)))).parts).toEqual([
      { part_number: 1, size: PART_SIZE },
    ]);
    // ListParts carries the stored checksum, so complete can re-verify it.
    const [u] = await conn.db.select().from(upload).where(eq(upload.id, c.id));
    const [lp] = await store.listParts(u.objectKey, u.multipartId as string);
    expect(lp.checksumSha256).toBe(hexToBase64(sha(parts[0])));
  });

  test("complete refuses a part of the wrong size (no presign, straight to storage)", async () => {
    const c = await json(
      await create("client", { filename: "short.sql", size: data.length, purpose: "dump" }),
    );
    await putPart("client", c.id, 1, parts[0]);
    const [u] = await conn.db.select().from(upload).where(eq(upload.id, c.id));
    const short = parts[1].subarray(0, 1024 * 1024);
    const url = await store.presignPart(u.objectKey, u.multipartId as string, 2, sha(short), short.length);
    const r = await fetch(url, {
      method: "PUT",
      body: new Uint8Array(short),
      headers: { "x-amz-checksum-sha256": hexToBase64(sha(short)) },
    });
    expect(r.status).toBe(200);
    const done = await api.completeUpload(req("client", { sha256: sha(data) }), idp(c.id));
    expect(done.status).toBe(409);
    expect((await json(done)).bad_parts).toEqual([2]);
    expect((await settledState(c.id)).state).toBe("uploading"); // nothing assembled, can retry part 2
    await putPart("client", c.id, 2, parts[1]);
    expect((await api.completeUpload(req("client", { sha256: sha(data) }), idp(c.id))).status).toBe(202);
  });

  test("at most 5 open uploads per user per project", async () => {
    const ids: string[] = [];
    for (let i = 0; i < MAX_OPEN_UPLOADS; i++) {
      const r = await create("client2", { filename: `o${i}.sql`, size: 1, purpose: "dump" });
      expect(r.status).toBe(201);
      ids.push((await json(r)).id);
    }
    const over = await create("client2", { filename: "o.sql", size: 1, purpose: "dump" });
    expect(over.status).toBe(409);
    // Another user is unaffected, and finishing/aborting one frees a slot.
    expect((await create("client", { filename: "o.sql", size: 1, purpose: "dump" })).status).toBe(201);
    expect((await api.abortUpload(req("client2", {}), idp(ids[0]))).status).toBe(200);
    expect((await create("client2", { filename: "o.sql", size: 1, purpose: "dump" })).status).toBe(201);
  });

  test("hash mismatch marks upload failed", async () => {
    const { id, done } = await full("client", "bad.sql", "dump", "f".repeat(64));
    expect(done.status).toBe(202);
    const u = await settled(id, ["failed", "ready"]);
    expect(u.state).toBe("failed");
    expect(await store.head(u.objectKey)).toBeNull(); // the bad object is deleted
    const a = (await conn.db.select().from(auditLog).where(eq(auditLog.targetId, id))).find(
      (x) => x.action === "upload.failed",
    );
    expect(a?.detail).toMatchObject({
      reason: "sha256 mismatch",
      expected: "f".repeat(64),
      actual: sha(data),
    });
  });

  test("abort aborts the multipart upload; only the uploader or a pusher may abort", async () => {
    const c = await json(await create("client", { filename: "a.sql", size: data.length, purpose: "dump" }));
    await putPart("client", c.id, 1, parts[0]);
    expect((await api.abortUpload(req("client2", {}), idp(c.id))).status).toBe(404); // not theirs, not visible
    expect((await api.abortUpload(req("viewer", {}), idp(c.id))).status).toBe(403);
    const [u] = await conn.db.select().from(upload).where(eq(upload.id, c.id));
    const r = await api.abortUpload(req("dev", {}), idp(c.id)); // developer has push
    expect(r.status).toBe(200);
    expect((await json(r)).state).toBe("aborted");
    await expect(store.listParts(u.objectKey, u.multipartId as string)).rejects.toMatchObject({
      name: "NoSuchUpload",
    });
    expect((await api.abortUpload(req("dev", {}), idp(c.id))).status).toBe(409);
    expect(
      (await api.uploadPartUrls(req("client", { parts: [{ part_number: 1, sha256: sha(data) }] }), idp(c.id)))
        .status,
    ).toBe(409);
    expect(await actions(c.id)).toContain("upload.abort");
  });

  test("a client can abort their own upload", async () => {
    const c = await json(await create("client", { filename: "a.sql", size: 5, purpose: "dump" }));
    expect((await api.abortUpload(req("client", {}), idp(c.id))).status).toBe(200);
  });

  test("an open upload on an archived project can still be aborted, not created", async () => {
    const c = await json(await create("client", { filename: "a.sql", size: 5, purpose: "dump" }));
    await conn.db.update(project).set({ archivedAt: new Date() }).where(eq(project.id, pid));
    try {
      expect((await create("client", { filename: "b.sql", size: 5, purpose: "dump" })).status).toBe(409);
      expect((await api.abortUpload(req("client", {}), idp(c.id))).status).toBe(200);
    } finally {
      await conn.db.update(project).set({ archivedAt: null }).where(eq(project.id, pid));
    }
  });

  /** A `verifying` row pointing at `key`, as if complete had just run. */
  const verifyingRow = async (key: string, size: number, hash = sha(data)) => {
    const [row] = await conn.db
      .insert(upload)
      .values({
        projectId: pid,
        uploaderId: "client",
        filename: "x.sql",
        size,
        sha256: hash,
        objectKey: key,
        state: "verifying",
        purpose: "dump",
      })
      .returning();
    return row;
  };
  const objectOf = async () => {
    const { id } = await full("client");
    const u = await settled(id, ["ready"]);
    await conn.db.update(upload).set({ state: "aborted" }).where(eq(upload.id, id));
    return u.objectKey;
  };
  const stateOf = async (id: string) => (await settledState(id)).state;

  test("verifyUpload is the size authority: a wrong declared size fails and deletes", async () => {
    const key = await objectOf();
    const row = await verifyingRow(key, data.length + 1);
    expect(await verifyUpload(conn.db, store, row.id)).toBe("failed");
    expect(await stateOf(row.id)).toBe("failed");
    expect(await store.head(key)).toBeNull();
    const a = (await conn.db.select().from(auditLog).where(eq(auditLog.targetId, row.id))).find(
      (x) => x.action === "upload.failed",
    );
    expect(a?.detail).toMatchObject({
      reason: "size mismatch",
      declared: data.length + 1,
      actual: data.length,
    });
  });

  test("verifyUpload fails an object larger than the max", async () => {
    const key = await objectOf();
    const row = await verifyingRow(key, data.length);
    expect(await verifyUpload(conn.db, store, row.id, 1000)).toBe("failed");
    expect(await store.head(key)).toBeNull();
  });

  test("verifyUpload: a missing object is a real failure", async () => {
    const row = await verifyingRow(`projects/${pid}/uploads/${randomUUID()}`, data.length);
    expect(await verifyUpload(conn.db, store, row.id)).toBe("failed");
    expect(await actions(row.id)).toContain("upload.failed");
  });

  test("verifyUpload: a transport error leaves the row verifying and deletes nothing", async () => {
    const key = await objectOf();
    const row = await verifyingRow(key, data.length);
    const deleted: string[] = [];
    const flaky = {
      getStream: async () => {
        throw Object.assign(new Error("read ECONNRESET"), { code: "ECONNRESET" });
      },
      deleteObject: async (k: string) => void deleted.push(k),
    };
    await expect(verifyUpload(conn.db, flaky, row.id)).rejects.toThrow("ECONNRESET");
    // Also when it breaks mid-stream.
    const { Readable } = await import("node:stream");
    const midway = {
      ...flaky,
      getStream: async () =>
        Readable.from(
          (async function* () {
            yield Buffer.from("partial");
            throw new Error("socket hang up");
          })(),
        ),
    };
    await expect(verifyUpload(conn.db, midway, row.id)).rejects.toThrow("socket hang up");
    expect(deleted).toEqual([]);
    expect(await stateOf(row.id)).toBe("verifying");
    expect(await store.head(key)).not.toBeNull();
    // The sweep retries and succeeds.
    expect(await verifyUpload(conn.db, store, row.id)).toBe("ready");
  });

  test("a throw after the claim reports 502 with state verifying", async () => {
    const spy = vi.spyOn(store, "head").mockRejectedValueOnce(new Error("storage blip"));
    const c = await json(await create("client", { filename: "a.sql", size: data.length, purpose: "dump" }));
    for (const [i, p] of parts.entries()) await putPart("client", c.id, i + 1, p);
    const r = await api.completeUpload(req("client", { sha256: sha(data) }), idp(c.id));
    spy.mockRestore();
    expect(r.status).toBe(502);
    expect((await json(r)).state).toBe("verifying");
    expect(await stateOf(c.id)).toBe("verifying");
    // The sweep verifies it, size included.
    expect(await verifyUpload(conn.db, store, c.id)).toBe("ready");
  });

  test("verifyUpload is idempotent and leaves non-verifying uploads alone", async () => {
    const { id } = await full("client");
    await settled(id, ["ready"]);
    expect(await verifyUpload(conn.db, store, id)).toBe("ready");
    expect(await verifyUpload(conn.db, store, randomUUID())).toBeNull();
    expect((await actions(id)).filter((a) => a === "upload.ready")).toHaveLength(1);
  });
});

const settledState = async (id: string) => (await conn.db.select().from(upload).where(eq(upload.id, id)))[0];

describe("visibility and download", () => {
  let clientUp: string;
  let devUp: string;
  beforeAll(async () => {
    clientUp = (await full("client", "mine.sql")).id;
    devUp = (await full("dev", "devs.sql")).id;
    await settled(clientUp, ["ready"]);
    await settled(devUp, ["ready"]);
  });

  test("client can upload but not download others' uploads", async () => {
    const list = await json(await api.listUploads(req("client"), slug));
    const ids = (list as unknown as { id: string }[]).map((u) => u.id);
    expect(ids).toContain(clientUp);
    expect(ids).not.toContain(devUp);
    expect((await api.getUpload(req("client"), idp(devUp))).status).toBe(404);
    expect(
      (
        await api.uploadPartUrls(
          req("client", { parts: [{ part_number: 1, sha256: sha(data) }] }),
          idp(devUp),
        )
      ).status,
    ).toBe(404);
    expect((await api.downloadUpload(req("client"), idp(devUp))).status).toBe(403);
    // Not even their own.
    expect((await api.downloadUpload(req("client"), idp(clientUp))).status).toBe(403);
    // Another client sees neither.
    const l2 = (await (await api.listUploads(req("client2"), slug)).json()) as { id: string }[];
    expect(l2.map((u) => u.id)).not.toContain(clientUp);
    expect(l2.map((u) => u.id)).not.toContain(devUp);
    expect((await api.getUpload(req("client2"), idp(clientUp))).status).toBe(404);
  });

  test("staff with view see everything, and never the object key or multipart id", async () => {
    for (const as of ["owner", "dev", "viewer"]) {
      const l = (await (await api.listUploads(req(as), slug)).json()) as Record<string, unknown>[];
      const ids = l.map((u) => u.id);
      expect(ids).toEqual(expect.arrayContaining([clientUp, devUp]));
      expect(JSON.stringify(l)).not.toMatch(/object_key|objectKey|multipart|projects\//);
    }
  });

  test("push gets a 15-minute presigned GET with an attachment Content-Disposition", async () => {
    for (const as of ["owner", "dev"]) {
      const r = await json(await api.downloadUpload(req(as), idp(clientUp)));
      expect(r.expires_in).toBe(900);
      expect(r.url).toContain("X-Amz-Expires=900");
      const body = Buffer.from(await (await fetch(r.url)).arrayBuffer());
      expect(sha(body)).toBe(sha(data));
    }
    const res = await fetch((await json(await api.downloadUpload(req("owner"), idp(clientUp)))).url);
    expect(res.headers.get("content-disposition")).toBe("attachment; filename*=UTF-8''mine.sql");
    expect(await actions(clientUp)).toContain("upload.download");
  });

  test("download needs a ready upload", async () => {
    const c = await json(await create("client", { filename: "a.sql", size: 1, purpose: "dump" }));
    expect((await api.downloadUpload(req("owner"), idp(c.id))).status).toBe(409);
    await api.abortUpload(req("client", {}), idp(c.id));
  });

  test("another project's upload is 404 on every route, and another client's too", async () => {
    const b = await json(
      await api.createUpload(req("owner", { filename: "b.sql", size: 1, purpose: "dump" }), { slug: "beta" }),
    );
    expect(b.id).toBeTruthy();
    const here = idp(b.id); // acme's slug, beta's upload id
    const p1 = { parts: [{ part_number: 1, sha256: sha(data) }] };
    for (const r of [
      await api.getUpload(req("owner"), here),
      await api.uploadPartUrls(req("owner", p1), here),
      await api.completeUpload(req("owner", { sha256: sha(data) }), here),
      await api.abortUpload(req("owner", {}), here),
      await api.downloadUpload(req("owner"), here),
    ])
      expect(r.status).toBe(404);
    // The same on beta's own slug works, so the 404s above are the project scope and nothing else.
    expect((await api.getUpload(req("owner"), { slug: "beta", id: b.id })).status).toBe(200);
    const acme = (await (await api.listUploads(req("owner"), slug)).json()) as { id: string }[];
    expect(acme.map((u) => u.id)).not.toContain(b.id);
    // Another client, same project.
    expect((await api.getUpload(req("client2"), idp(clientUp))).status).toBe(404);
    expect((await api.abortUpload(req("client2", {}), idp(clientUp))).status).toBe(404);
    expect((await api.downloadUpload(req("client2"), idp(clientUp))).status).toBe(403);
    expect((await api.uploadPartUrls(req("client2", p1), idp(clientUp))).status).toBe(404);
    expect((await api.completeUpload(req("client2", { sha256: sha(data) }), idp(clientUp))).status).toBe(404);
  });

  test("unknown ids are 404, bad ids 400", async () => {
    expect((await api.getUpload(req("owner"), idp(randomUUID()))).status).toBe(404);
    expect((await api.getUpload(req("owner"), idp("nope"))).status).toBe(400);
  });
});
