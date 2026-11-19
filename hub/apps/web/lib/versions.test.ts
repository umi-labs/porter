import { createHash, randomUUID } from "node:crypto";
import {
  auditLog,
  connect,
  mediaObject,
  member,
  organization,
  project,
  projectMember,
  projectVersion,
  run,
  user,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { and, eq } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { makeApi } from "./api";
import { hexToBase64 } from "./storage";

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let api: ReturnType<typeof makeApi>;
let pid: string;
let otherPid: string;

const USERS = ["owner", "dev", "dev2", "viewer", "client", "stranger"];
const req = (as: string | null, body?: unknown) =>
  new Request("http://hub.test/x", {
    method: body === undefined ? "GET" : "POST",
    headers: { ...(as ? { "x-user": as } : {}), "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
const slug = { slug: "acme" };
const sha = (b: Buffer | string) => createHash("sha256").update(b).digest("hex");
const json = async (r: Response) => (await r.json()) as Record<string, any>;

/** Real init, then fetch the media PUT urls the way the CLI does (init only returns `missing`). */
async function initV(r: Request) {
  const body = (await r.json()) as Record<string, unknown>;
  body.snapshot_sha256 ??= sha("snap");
  const res = await api.initVersion(req(r.headers.get("x-user"), body), slug);
  if (res.status !== 200) return res;
  const init = (await res.json()) as { upload_id: string; missing: string[] };
  const media_put_urls: Record<string, string> = {};
  for (let i = 0; i < init.missing.length; i += 1000) {
    const m = await api.uploadMediaUrls(
      req(r.headers.get("x-user"), { hashes: init.missing.slice(i, i + 1000) }),
      { ...slug, upload_id: init.upload_id },
    );
    Object.assign(media_put_urls, ((await m.json()) as { urls: Record<string, string> }).urls);
  }
  return Response.json({ ...init, media_put_urls });
}

const put = (url: string, body: Buffer, hash?: string) =>
  fetch(url, {
    method: "PUT",
    body: new Uint8Array(body),
    headers: { "if-none-match": "*", ...(hash ? { "x-amz-checksum-sha256": hexToBase64(hash) } : {}) },
  });

/** init -> upload snapshot + missing media -> returns what commit needs. */
async function push(as: string, base: number | null, media: Buffer[], snapshot = Buffer.from("snap")) {
  const hashes = media.map(sha);
  const init = await json(await initV(req(as, { base_version: base, media_hashes: hashes })));
  expect((await put(init.snapshot_put_url, snapshot, sha(snapshot))).status).toBe(200);
  for (const [i, h] of hashes.entries()) {
    if (init.media_put_urls[h]) expect((await put(init.media_put_urls[h], media[i], h)).status).toBe(200);
  }
  return { init, hashes };
}
const commit = (as: string, upload_id: string, base_version: number | null, extra = {}) =>
  api.commitVersion(
    req(as, { upload_id, base_version, porter_version: "1.2.3", summary: { ok: 1 }, ...extra }),
    slug,
  );

beforeAll(async () => {
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  const now = new Date();
  for (const id of USERS) {
    await conn.db
      .insert(user)
      .values({ id, name: id, email: `${id}@x.test`, emailVerified: true, createdAt: now, updatedAt: now });
  }
  await conn.db.insert(organization).values({ id: "umi-org", name: "Umi", slug: "umi", createdAt: now });
  await conn.db
    .insert(member)
    .values({ id: "m-o", organizationId: "umi-org", userId: "owner", role: "member", createdAt: now });
  const [p, p2] = await conn.db
    .insert(project)
    .values([
      { orgId: "umi-org", name: "Acme", slug: "acme" },
      { orgId: "umi-org", name: "Other", slug: "other" },
    ])
    .returning();
  pid = p.id;
  otherPid = p2.id;
  await conn.db.insert(projectMember).values([
    { projectId: pid, userId: "owner", role: "owner" },
    { projectId: pid, userId: "dev", role: "developer" },
    { projectId: pid, userId: "dev2", role: "developer" },
    { projectId: pid, userId: "viewer", role: "viewer" },
    { projectId: pid, userId: "client", role: "client" },
    { projectId: otherPid, userId: "dev", role: "developer" },
  ]);
  api = makeApi(conn.db, async (r) => {
    const id = r.headers.get("x-user");
    return id ? { id } : null;
  });
});
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
});

describe("permissions", () => {
  const routes: [string, string[], (as: string | null) => Promise<Response>][] = [
    ["init", ["viewer", "client"], (a) => initV(req(a, { base_version: null, media_hashes: [] }))],
    [
      "commit",
      ["viewer", "client"],
      (a) =>
        api.commitVersion(
          req(a, { upload_id: randomUUID(), base_version: null, porter_version: "1", summary: {} }),
          slug,
        ),
    ],
    ["list versions", ["client"], (a) => api.listVersions(req(a), slug)],
    ["get version", ["client"], (a) => api.getVersion(req(a), { ...slug, n: "1" })],
    ["media urls", ["client"], (a) => api.mediaUrls(req(a, { hashes: [sha("x")] }), slug)],
  ];
  for (const [name, forbidden, call] of routes) {
    test(`${name}: 401 signed out, 404 non-member, 403 for ${forbidden.join("/")}`, async () => {
      expect((await call(null)).status).toBe(401);
      expect((await call("stranger")).status).toBe(404);
      for (const r of forbidden) expect((await call(r)).status).toBe(403);
    });
  }
  test("a viewer can pull but not push", async () => {
    expect((await api.listVersions(req("viewer"), slug)).status).toBe(200);
  });
});

describe("push and pull", () => {
  const a = Buffer.from("media A");
  const b = Buffer.from("media B");

  test("only missing media requested, and the full flow round-trips", async () => {
    const first = await push("dev", null, [a]);
    expect(first.init.missing).toEqual([sha(a)]);
    expect(Object.keys(first.init.media_put_urls)).toEqual([sha(a)]);
    const done = await commit("dev", first.init.upload_id, null, { message: "first" });
    expect(done.status).toBe(201);
    expect((await json(done)).number).toBe(1);

    // Second push already has A, so only B is requested.
    const init2 = await json(
      await initV(
        req("dev", {
          base_version: 1,
          snapshot_sha256: sha("snapshot two"),
          media_hashes: [sha(a), sha(b), sha(b)],
        }),
      ),
    );
    expect(init2.missing).toEqual([sha(b)]);
    expect(Object.keys(init2.media_put_urls)).toEqual([sha(b)]);
    // Nothing to upload at all.
    const init3 = await json(await initV(req("dev", { base_version: 1, media_hashes: [sha(a)] })));
    expect(init3.missing).toEqual([]);
    expect(init3.media_put_urls).toEqual({});

    const snap = Buffer.from("snapshot two");
    expect((await put(init2.snapshot_put_url, snap, sha(snap))).status).toBe(200);
    expect((await put(init2.media_put_urls[sha(b)], b, sha(b))).status).toBe(200);
    expect((await commit("dev", init2.upload_id, 1)).status).toBe(201);

    // Pull: version 2 has both media hashes and a working snapshot URL.
    const v = await json(await api.getVersion(req("viewer"), { ...slug, n: "2" }));
    expect(v).toMatchObject({ number: 2, porter_version: "1.2.3", summary: { ok: 1 }, size: snap.length });
    expect(v.media_hashes.sort()).toEqual([sha(a), sha(b)].sort());
    expect(Buffer.from(await (await fetch(v.snapshot_get_url)).arrayBuffer())).toEqual(snap);
    const urls = await json(await api.mediaUrls(req("viewer", { hashes: [sha(a), sha("nope")] }), slug));
    expect(Buffer.from(await (await fetch(urls.urls[sha(a)])).arrayBuffer())).toEqual(a);
    expect(urls.missing).toEqual([sha("nope")]);

    const list = (await (await api.listVersions(req("viewer"), slug)).json()) as { number: number }[];
    expect(list.map((x) => x.number)).toEqual([2, 1]);
    expect((await api.getVersion(req("viewer"), { ...slug, n: "9" })).status).toBe(404);
    expect((await api.getVersion(req("viewer"), { ...slug, n: "abc" })).status).toBe(400);

    // Per-project key prefixes; media is stored once and audited.
    const rows = await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid));
    for (const r of rows)
      expect(r.snapshotKey).toMatch(new RegExp(`^projects/${pid}/snapshots/[0-9a-f-]+\\.tar\\.zst$`));
    expect(await conn.db.select().from(mediaObject).where(eq(mediaObject.projectId, pid))).toHaveLength(2);
    expect(await conn.db.select().from(auditLog).where(eq(auditLog.action, "version.commit"))).toHaveLength(
      2,
    );
  });

  test("commit refuses stale base", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const x = await push("dev", latest, []);
    const y = await push("dev2", latest, []);
    expect((await commit("dev", x.init.upload_id, latest)).status).toBe(201);
    const stale = await commit("dev2", y.init.upload_id, latest);
    expect(stale.status).toBe(409);
    expect(await json(stale)).toMatchObject({ latest: latest + 1 });
    // And init with a stale base is refused before any upload.
    const early = await initV(req("dev2", { base_version: latest, media_hashes: [] }));
    expect(early.status).toBe(409);
    expect((await json(early)).latest).toBe(latest + 1);
  });

  test("init is refused while a run is queued or running, naming the run", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const [r] = await conn.db
      .insert(run)
      .values({ projectId: pid, stages: ["analyse"] })
      .returning();
    try {
      for (const state of ["queued", "running"] as const) {
        await conn.db.update(run).set({ state }).where(eq(run.id, r.id));
        const res = await initV(req("dev", { base_version: latest, media_hashes: [] }));
        expect(res.status).toBe(409);
        const b = await json(res);
        expect(b).toMatchObject({ run_id: r.id });
        expect(b.latest).toBeUndefined();
        expect(b.error).toMatch(/run is queued or running/);
      }
      await conn.db.update(run).set({ state: "finished" }).where(eq(run.id, r.id));
      expect((await initV(req("dev", { base_version: latest, media_hashes: [] }))).status).toBe(200);
    } finally {
      await conn.db.delete(run).where(eq(run.id, r.id));
    }
  });

  test("commit is refused if a run was queued after init; the init survives for a retry", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const x = await push("dev", latest, []);
    const [r] = await conn.db
      .insert(run)
      .values({ projectId: pid, stages: ["analyse"] })
      .returning();
    try {
      const res = await commit("dev", x.init.upload_id, latest);
      expect(res.status).toBe(409);
      expect(await json(res)).toMatchObject({ run_id: r.id });
    } finally {
      await conn.db.delete(run).where(eq(run.id, r.id));
    }
    expect((await commit("dev", x.init.upload_id, latest)).status).toBe(201);
  });

  test("concurrent commits from the same base: exactly one wins", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const x = await push("dev", latest, []);
    const y = await push("dev2", latest, []);
    const rs = await Promise.all([
      commit("dev", x.init.upload_id, latest),
      commit("dev2", y.init.upload_id, latest),
    ]);
    expect(rs.map((r) => r.status).sort()).toEqual([201, 409]);
  });

  test("base_version null is the first push; 0 is not valid", async () => {
    expect((await initV(req("dev", { base_version: 0, media_hashes: [] }))).status).toBe(400);
    expect((await initV(req("dev", { media_hashes: [] }))).status).toBe(400);
  });

  test("commit rejects an unfinished upload", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const m = Buffer.from("never uploaded");
    const init = await json(await initV(req("dev", { base_version: latest, media_hashes: [sha(m)] })));
    // No snapshot yet.
    expect((await commit("dev", init.upload_id, latest)).status).toBe(422);
    await put(init.snapshot_put_url, Buffer.from("snap"), sha("snap"));
    // Snapshot but no media.
    const r = await commit("dev", init.upload_id, latest);
    expect(r.status).toBe(422);
    expect((await json(r)).missing).toEqual([sha(m)]);
  });

  test("an upload_id is bound to its project and user", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const x = await push("dev", latest, []);
    expect((await commit("dev2", x.init.upload_id, latest)).status).toBe(404);
    expect((await commit("dev", randomUUID(), latest)).status).toBe(404);
    // dev is a developer on "other" too; the id still belongs to acme.
    const viaOther = await api.commitVersion(
      req("dev", { upload_id: x.init.upload_id, base_version: null, porter_version: "1", summary: {} }),
      { slug: "other" },
    );
    expect(viaOther.status).toBe(404);
    expect((await commit("dev", x.init.upload_id, latest)).status).toBe(201);
    // Spent: can't be replayed.
    expect((await commit("dev", x.init.upload_id, latest + 1)).status).toBe(404);
  });

  test("validates hashes and caps summary and batch sizes", async () => {
    const bad = ["A".repeat(64), "abc", "g".repeat(64), `${"a".repeat(63)}`];
    for (const h of bad) {
      expect((await initV(req("dev", { base_version: null, media_hashes: [h] }))).status).toBe(400);
      expect((await api.mediaUrls(req("dev", { hashes: [h] }), slug)).status).toBe(400);
    }
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const x = await push("dev", latest, []);
    const big = await commit("dev", x.init.upload_id, latest, { summary: { blob: "x".repeat(256 * 1024) } });
    expect(big.status).toBe(413);
    const many = Array.from({ length: 1001 }, (_, i) => sha(String(i)));
    expect((await api.mediaUrls(req("dev", { hashes: many }), slug)).status).toBe(400);
    expect((await api.mediaUrls(req("dev", { hashes: many.slice(0, 1000) }), slug)).status).toBe(200);
  });
});

describe("integrity and expiry", () => {
  test("S3 rejects media whose body doesn't match the hash, or a PUT without the checksum header", async () => {
    const m = Buffer.from("real content");
    const init = await json(
      await initV(
        req("owner", {
          base_version: (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
            .length,
          media_hashes: [sha(m)],
        }),
      ),
    );
    const url = init.media_put_urls[sha(m)];
    const tampered = await put(url, Buffer.from("tampered"), sha(m));
    expect(tampered.status).toBe(400);
    expect(await tampered.text()).toContain("XAmzContentChecksumMismatch");
    // The uploader can't pick its own checksum: a header for the tampered body breaks the signature.
    expect((await put(url, Buffer.from("tampered"), sha("tampered"))).status).toBe(403);
    expect((await put(url, m)).status).toBe(400);
    expect((await put(url, m, sha(m))).status).toBe(200);
    // Write-once: a second PUT (even identical bytes) is refused.
    expect((await put(url, m, sha(m))).status).toBe(412);
  });

  test("the snapshot is pinned by snapshot_sha256 and can't be replaced after upload", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const init = await json(
      await initV(req("owner", { base_version: latest, snapshot_sha256: sha("good"), media_hashes: [] })),
    );
    const bad = await put(init.snapshot_put_url, Buffer.from("evil"), sha("good"));
    expect(bad.status).toBe(400);
    expect(await bad.text()).toContain("XAmzContentChecksumMismatch");
    expect((await put(init.snapshot_put_url, Buffer.from("good"), sha("good"))).status).toBe(200);
    expect((await put(init.snapshot_put_url, Buffer.from("evil"), sha("good"))).status).toBe(412);
    expect((await put(init.snapshot_put_url, Buffer.from("good"), sha("good"))).status).toBe(412);
    expect((await commit("owner", init.upload_id, latest)).status).toBe(201);
    const v = await json(await api.getVersion(req("owner"), { ...slug, n: String(latest + 1) }));
    expect(v.snapshot_sha256).toBe(sha("good"));
    expect(await (await fetch(v.snapshot_get_url)).text()).toBe("good");
  });

  test("snapshot_sha256 is required and must be a lowercase hash", async () => {
    for (const snapshot_sha256 of [undefined, "A".repeat(64), "abc"]) {
      const r = await api.initVersion(
        req("owner", { base_version: null, media_hashes: [], snapshot_sha256 }),
        slug,
      );
      expect(r.status).toBe(400);
    }
  });

  test("commit must use the base_version given at init", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const x = await push("dev", latest, []);
    const r = await commit("dev", x.init.upload_id, latest + 1);
    expect(r.status).toBe(409);
  });

  test("init returns only the snapshot url; media urls come per upload, in batches, for missing hashes only", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid)))
      .length;
    const m = Buffer.from("batched");
    const res = await json(
      await api.initVersion(
        req("dev", {
          base_version: latest,
          snapshot_sha256: sha("snap"),
          media_hashes: [sha(m), sha("existing?")],
        }),
        slug,
      ),
    );
    expect(Object.keys(res).sort()).toEqual(["missing", "snapshot_put_url", "upload_id"]);
    const call = (as: string, id: string, hashes: string[], slg = slug) =>
      api.uploadMediaUrls(req(as, { hashes }), { ...slg, upload_id: id });
    expect((await call("dev", res.upload_id, [sha(m)])).status).toBe(200);
    expect((await call("dev", res.upload_id, [sha("other")])).status).toBe(400);
    expect((await call("dev2", res.upload_id, [sha(m)])).status).toBe(404);
    expect((await call("viewer", res.upload_id, [sha(m)])).status).toBe(403);
    expect((await call("stranger", res.upload_id, [sha(m)])).status).toBe(404);
    expect((await call("dev", res.upload_id, [sha(m)], { slug: "other" })).status).toBe(404);
    expect((await call(null as unknown as string, res.upload_id, [sha(m)])).status).toBe(401);
    const many = Array.from({ length: 1001 }, (_, i) => sha(`z${i}`));
    expect((await call("dev", res.upload_id, many)).status).toBe(400);
  });

  test("a large first push commits via a listing, not per-object HEADs", async () => {
    const latest = (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, otherPid)))
      .length;
    const media = Array.from({ length: 520 }, (_, i) => Buffer.from(`bulk ${i}`));
    const hashes = media.map(sha);
    const init = await json(
      await api.initVersion(
        req("dev", {
          base_version: latest ? latest : null,
          snapshot_sha256: sha("snap"),
          media_hashes: hashes,
        }),
        { slug: "other" },
      ),
    );
    expect(init.missing).toHaveLength(520);
    const urls: Record<string, string> = {};
    for (let i = 0; i < 520; i += 1000) {
      const r = await json(
        await api.uploadMediaUrls(req("dev", { hashes: hashes.slice(i, i + 1000) }), {
          slug: "other",
          upload_id: init.upload_id,
        }),
      );
      Object.assign(urls, r.urls);
    }
    await put(init.snapshot_put_url, Buffer.from("snap"), sha("snap"));
    // Leave one out: commit must name it.
    for (const [i, h] of hashes.entries()) if (i !== 7) await put(urls[h], media[i], h);
    const miss = await api.commitVersion(
      req("dev", { upload_id: init.upload_id, base_version: null, porter_version: "1", summary: {} }),
      { slug: "other" },
    );
    expect(miss.status).toBe(422);
    expect((await json(miss)).missing).toEqual([hashes[7]]);
    await put(urls[hashes[7]], media[7], hashes[7]);
    const ok = await api.commitVersion(
      req("dev", { upload_id: init.upload_id, base_version: null, porter_version: "1", summary: {} }),
      { slug: "other" },
    );
    expect(ok.status).toBe(201);
  });

  test("presigned urls expire", async () => {
    const m = Buffer.from("expiry");
    const { init } = await push(
      "owner",
      (await conn.db.select().from(projectVersion).where(eq(projectVersion.projectId, pid))).length,
      [m],
    );
    const v = await json(await api.getVersion(req("owner"), { ...slug, n: "1" }));
    const urls = await json(await api.mediaUrls(req("owner", { hashes: [sha("media A")] }), slug));
    for (const u of [
      init.snapshot_put_url,
      init.media_put_urls[sha(m)],
      v.snapshot_get_url,
      urls.urls[sha("media A")],
    ]) {
      expect(new URL(u).searchParams.get("X-Amz-Expires")).toBe("900");
    }
  });
});

test("audit rows exist for commits by the right actor", async () => {
  const rows = await conn.db
    .select()
    .from(auditLog)
    .where(and(eq(auditLog.action, "version.commit"), eq(auditLog.projectId, pid)));
  expect(rows.length).toBeGreaterThan(2);
  expect(rows.every((r) => ["owner", "dev", "dev2"].includes(r.actorId ?? ""))).toBe(true);
});
