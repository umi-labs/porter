import { execFileSync } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  auditLog,
  connect,
  mediaObject,
  organization,
  project,
  projectVersion,
  run,
  runEvent,
  runSecret,
  upload,
  user,
  versionMedia,
} from "@porter-hub/db";
import { runMigrations } from "@porter-hub/db/migrate";
import { freshDb } from "@porter-hub/db/test-helper";
import { encryptSecret } from "@porter-hub/porter";
import { createMultipart, getStream, keys, putFile } from "@porter-hub/storage/s3";
import { extractTarZst, packTarZst } from "@porter-hub/storage/tar";
import { and, asc, eq, sql } from "drizzle-orm";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { claim, reclaimLost } from "./claim";
import { execute, outcome, type Settings } from "./execute";
import { sweepUploads } from "./sweep";

process.env.HUB_SECRET_KEY = randomBytes(32).toString("base64");

const repo = resolve(fileURLToPath(import.meta.url), "../../../../..");
const PORTER = join(repo, "target/debug/porter");
const WXR = join(repo, "crates/porter-engine/tests/fixtures/wxr/basic.xml");
const WP_DB_NOTE = "wp-db connector skipped on the hub; run it locally and push";

let fresh: Awaited<ReturnType<typeof freshDb>>;
let conn: ReturnType<typeof connect>;
let pid: string;
let tmp: string;
let s: Settings;
let v1: typeof projectVersion.$inferSelect;
const MEDIA = Buffer.from("a picture, honestly");
const MEDIA_SHA = createHash("sha256").update(MEDIA).digest("hex");

const sha = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");

/** porter init + a wxr connector (+ extra toml), packed and stored as the next version. */
async function storeVersion(extraToml = "", media: string[] = [], wxr = true) {
  const dir = mkdtempSync(join(tmp, "fixture-"));
  execFileSync(PORTER, ["--json", "--project", dir, "init", "--adapter", "wordpress", "demo"]);
  copyFileSync(WXR, join(dir, "export.xml"));
  const wxrBlock = wxr ? `\n[[connectors]]\nkind = "wxr"\npath = "export.xml"\n` : "";
  writeFileSync(
    join(dir, "porter.toml"),
    `${readFileSync(join(dir, "porter.toml"), "utf8")}${wxrBlock}${extraToml}`,
  );
  const out = join(tmp, `${randomUUID()}.tar.zst`);
  await packTarZst(dir, out);
  const id = randomUUID();
  await putFile(keys.snapshot(pid, id), out, sha(out), statSync(out).size);
  const [{ n }] = await conn.db
    .select({ n: sql<number>`coalesce(max(${projectVersion.number}), 0)` })
    .from(projectVersion)
    .where(eq(projectVersion.projectId, pid));
  const [v] = await conn.db
    .insert(projectVersion)
    .values({
      projectId: pid,
      number: Number(n) + 1,
      snapshotKey: keys.snapshot(pid, id),
      snapshotSha256: sha(out),
      size: statSync(out).size,
    })
    .returning();
  if (media.length)
    await conn.db
      .insert(versionMedia)
      .values(media.map((h) => ({ versionId: v.id, projectId: pid, sha256: h })));
  return v;
}

/** Queues a run on `base`, claims it and executes it to the end. */
async function runOn(
  base: { id: string },
  stages: string[],
  opts: { bin?: string; whileRunning?: (runId: string) => Promise<void> } = {},
) {
  const [q] = await conn.db
    .insert(run)
    .values({ projectId: pid, baseVersionId: base.id, stages, requestedBy: "dev" })
    .returning();
  const r = await claim(conn.db);
  expect(r?.id).toBe(q.id);
  if (!r) throw new Error("not claimed");
  const done = execute(conn.db, r, { ...s, bin: opts.bin ?? s.bin });
  await opts.whileRunning?.(r.id);
  await done;
  const [after] = await conn.db.select().from(run).where(eq(run.id, r.id));
  const evs = await conn.db
    .select()
    .from(runEvent)
    .where(eq(runEvent.runId, r.id))
    .orderBy(asc(runEvent.seq));
  return { run: after, events: evs, list: evs.map((e) => e.event as Record<string, unknown>) };
}

/** A shell script standing in for porter: runs `body`, then (unless it exits) the real porter. */
function fakePorter(body: string) {
  const p = join(tmp, `porter-${randomUUID()}.sh`);
  // The worker also calls `--version` and `status`; those go straight to porter.
  const passThrough = `case "$*" in *--version*|*" status") exec "${PORTER}" "$@";; esac`;
  writeFileSync(p, `#!/bin/sh\n${passThrough}\n${body}\nexec "${PORTER}" "$@"\n`);
  chmodSync(p, 0o755);
  return p;
}

function filesContaining(dir: string, needle: string): string[] {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((e) => e.isFile())
    .map((e) => join(e.parentPath, e.name))
    .filter((f) => readFileSync(f).includes(needle));
}

async function snapshotOf(versionId: string) {
  const [v] = await conn.db.select().from(projectVersion).where(eq(projectVersion.id, versionId));
  const dir = mkdtempSync(join(tmp, "result-"));
  await extractTarZst(await getStream(v.snapshotKey), dir);
  return { v, dir };
}

beforeAll(async () => {
  if (!existsSync(PORTER))
    execFileSync("cargo", ["build", "-p", "porter-cli"], { cwd: repo, stdio: "inherit" });
  tmp = mkdtempSync(join(tmpdir(), "worker-test-"));
  s = { bin: PORTER, scratchDir: join(tmp, "scratch"), killAfterMs: 5000, cancelPollMs: 100 };
  fresh = await freshDb();
  await runMigrations(fresh.url);
  conn = connect(fresh.url);
  const now = new Date();
  await conn.db.insert(user).values({
    id: "dev",
    name: "dev",
    email: "dev@x.test",
    emailVerified: true,
    createdAt: now,
    updatedAt: now,
  });
  await conn.db.insert(organization).values({ id: "org", name: "Umi", slug: "umi", createdAt: now });
  const [p] = await conn.db.insert(project).values({ orgId: "org", name: "Acme", slug: "acme" }).returning();
  pid = p.id;
  const m = join(tmp, "media.bin");
  writeFileSync(m, MEDIA);
  await putFile(keys.media(pid, MEDIA_SHA), m, MEDIA_SHA, MEDIA.length);
  await conn.db.insert(mediaObject).values({ projectId: pid, sha256: MEDIA_SHA, size: MEDIA.length });
  v1 = await storeVersion("", [MEDIA_SHA]);
}, 600_000);
afterAll(async () => {
  await conn?.client.end();
  await fresh?.drop();
  if (tmp) await rm(tmp, { recursive: true, force: true });
});

describe("worker", () => {
  test("events parsed and stored in order; result committed as a new version", async () => {
    const { run: r, events, list } = await runOn(v1, ["analyse"]);
    expect(r.state).toBe("finished");
    expect(r.exitCode).toBe(0);
    expect(events.map((e) => e.seq)).toEqual(events.map((_, i) => i + 1));
    const names = list.map((e) => e.event);
    expect(names[0]).toBe("stage-started");
    expect(names).toContain("item-result");
    expect(names.indexOf("stage-finished")).toBeGreaterThan(names.indexOf("issue-opened"));
    expect(list.every((e) => String(e.event).startsWith("hub-") || e.v === 1)).toBe(true);
    expect(names.at(-1)).toBe("hub-result");

    // Result: latest+1, linked, original porter.toml, project.db + raw/, no media/, media still referenced.
    expect(r.resultVersionId).toBeTruthy();
    const { v, dir } = await snapshotOf(r.resultVersionId as string);
    expect(v.number).toBe(v1.number + 1);
    expect(v.message).toBe(`remote run ${r.id} of v${v1.number}`);
    expect(existsSync(join(dir, "project.db"))).toBe(true);
    expect(existsSync(join(dir, "raw"))).toBe(true);
    expect(existsSync(join(dir, "media"))).toBe(false);
    expect((v.summary as { event?: string }).event).toBe("status");
    const media = await conn.db.select().from(versionMedia).where(eq(versionMedia.versionId, v.id));
    expect(media.map((x) => x.sha256)).toEqual([MEDIA_SHA]);
    // The scratch dir is gone.
    expect(existsSync(join(s.scratchDir, r.id))).toBe(false);
    const audit = await conn.db.select().from(auditLog).where(eq(auditLog.targetId, r.id));
    expect(audit.map((a) => a.action)).toContain("run.finish");
  });

  test("malformed line recorded and run continues; unknown events stored", async () => {
    const v = v1;
    const bin = fakePorter(
      `echo 'not json at all'\necho '{"v":1,"event":"progress","stage":"bogus"}'\necho '{"v":1,"event":"future-thing","x":1}'`,
    );
    const { run: r, list } = await runOn(v, ["analyse"], { bin });
    expect(r.state).toBe("finished");
    expect(list[0]).toEqual({ event: "hub-malformed", line: "not json at all" });
    expect(list[1]).toEqual({ event: "hub-malformed", line: '{"v":1,"event":"progress","stage":"bogus"}' });
    expect(list[2]).toEqual({ v: 1, event: "future-thing", x: 1 });
    expect(list[3].event).toBe("stage-started");
  });

  test("porter crash -> failed with stderr", async () => {
    const v = v1;
    // schema check with no destination configured: porter exits 1 and says why on stderr.
    const { run: r, list } = await runOn(v, ["schema-check"]);
    expect(r.state).toBe("failed");
    expect(r.exitCode).toBe(1);
    expect(r.error).toBe("porter could not run (exit 1)");
    const err = list.find((e) => e.event === "hub-stderr");
    expect(err?.text).toContain("destination.adapter is missing");
    // porter ran, so the result is still kept.
    expect(r.resultVersionId).toBeTruthy();

    // A binary that can't start at all: failed, no result.
    const gone = await runOn(v, ["analyse"], { bin: join(tmp, "no-such-porter") });
    expect(gone.run.state).toBe("failed");
    expect(gone.run.error).toMatch(/^porter could not start: /);
    expect(gone.run.resultVersionId).toBeNull();
  });

  test("lease expiry reclaims", async () => {
    const v = v1;
    const [r] = await conn.db
      .insert(run)
      .values({ projectId: pid, baseVersionId: v.id, stages: ["analyse"], state: "running" })
      .returning();
    await conn.db.execute(
      sql`update run set heartbeat_at = now() - interval '30 seconds' where id = ${r.id}`,
    );
    expect(await reclaimLost(conn.db)).toEqual([]); // still within its 60 s lease
    await conn.db.execute(
      sql`update run set heartbeat_at = now() - interval '61 seconds' where id = ${r.id}`,
    );
    expect(await reclaimLost(conn.db)).toEqual([r.id]);
    const [after] = await conn.db.select().from(run).where(eq(run.id, r.id));
    expect(after.state).toBe("failed");
    expect(after.error).toBe("worker lost");
    const evs = await conn.db.select().from(runEvent).where(eq(runEvent.runId, r.id));
    expect(evs.map((e) => e.event)).toEqual([
      expect.objectContaining({ event: "hub-note", kind: "worker-lost" }),
    ]);
  });

  test("wp-db stripped with note; the result keeps the original porter.toml", async () => {
    const wpdb = `\n[[connectors]]\nkind = "wp-db"\ndump = "site.sql"\n`;
    const v = await storeVersion(wpdb);
    const bin = fakePorter(
      `grep -c 'wp-db' "$HOME/project/porter.toml" | sed 's/.*/{"v":1,"event":"test-wpdb","n":&}/'`,
    );
    const { run: r, list } = await runOn(v, ["analyse"], { bin });
    expect(list[0]).toEqual({ event: "hub-note", kind: "access-gap", message: WP_DB_NOTE });
    expect(list[1]).toEqual({ v: 1, event: "test-wpdb", n: 0 }); // porter never saw it
    expect(r.state).toBe("finished");
    const { dir } = await snapshotOf(r.resultVersionId as string);
    expect(readFileSync(join(dir, "porter.toml"), "utf8")).toContain('kind = "wp-db"');

    // Only wp-db: fail before spawning porter.
    const only = await storeVersion(wpdb, [], false);
    const failed = await runOn(only, ["analyse"]);
    expect(failed.run.state).toBe("failed");
    expect(failed.run.error).toBe(WP_DB_NOTE);
    expect(failed.list).toEqual([{ event: "hub-note", kind: "access-gap", message: WP_DB_NOTE }]);
    expect(failed.run.resultVersionId).toBeNull();
  });

  test("secrets only in env: the spawned env has them, no file in the scratch dir does", async () => {
    const v = v1;
    const value = `s3cr3t${randomBytes(8).toString("hex")}`;
    await conn.db
      .insert(runSecret)
      .values({ projectId: pid, name: "MY_TOKEN", ...encryptSecret(pid, "MY_TOKEN", value) });
    process.env.WORKER_LEAK_CHECK = "leaked"; // the worker's own env must not reach porter
    const bin = fakePorter(
      [
        `echo "{\\"v\\":1,\\"event\\":\\"test-env\\",\\"token\\":\\"$MY_TOKEN\\",\\"leak\\":\\"$WORKER_LEAK_CHECK\\",\\"names\\":\\"$(env | cut -d= -f1 | sort | tr '\\n' ' ')\\"}"`,
        `echo "{\\"v\\":1,\\"event\\":\\"test-grep\\",\\"files\\":$(grep -rl "$MY_TOKEN" "$HOME" | wc -l | tr -d ' ')}"`,
      ].join("\n"),
    );
    const { run: r, list } = await runOn(v, ["analyse"], { bin });
    delete process.env.WORKER_LEAK_CHECK;
    await conn.db.delete(runSecret).where(eq(runSecret.projectId, pid));
    expect(r.state).toBe("finished");
    const envEv = list.find((e) => e.event === "test-env");
    expect(envEv?.token).toBe(value);
    expect(envEv?.leak).toBe("");
    // sh adds PWD, SHLVL and _ itself.
    const names = String(envEv?.names)
      .trim()
      .split(" ")
      .filter((n) => !["PWD", "SHLVL", "_"].includes(n));
    expect(names.sort()).toEqual(["HOME", "LANG", "MY_TOKEN", "PATH"]);
    expect(list.find((e) => e.event === "test-grep")?.files).toBe(0);
    // ...and not in the stored result either.
    const { dir } = await snapshotOf(r.resultVersionId as string);
    expect(filesContaining(dir, value)).toEqual([]);
  });

  test("cancel sends SIGINT; exit 3 -> cancelled, result kept", async () => {
    const v = v1;
    const bin = fakePorter(
      `trap 'kill $p; exit 3' INT\necho '{"v":1,"event":"stage-started","stage":"analyse","run":"x"}'\nsleep 20 >/dev/null 2>&1 &\np=$!\nwait $p`,
    );
    const { run: r } = await runOn(v, ["analyse"], {
      bin,
      whileRunning: async (id) => {
        for (let i = 0; i < 100; i++) {
          const evs = await conn.db.select().from(runEvent).where(eq(runEvent.runId, id));
          if (evs.length) break;
          await new Promise((res) => setTimeout(res, 50));
        }
        await conn.db.update(run).set({ cancelRequested: true }).where(eq(run.id, id));
      },
    });
    expect(r.state).toBe("cancelled");
    expect(r.exitCode).toBe(3);
    expect(r.resultVersionId).toBeTruthy();
  });

  test("exit mapping", () => {
    expect(outcome({ code: 0, signal: null }, false)).toMatchObject({ state: "finished", exitCode: 0 });
    expect(outcome({ code: 0, signal: null }, false, true)).toMatchObject({ state: "finished", exitCode: 2 });
    expect(outcome({ code: 2, signal: null }, false)).toMatchObject({ state: "finished", exitCode: 2 });
    expect(outcome({ code: 3, signal: null }, true)).toMatchObject({ state: "cancelled", exitCode: 3 });
    expect(outcome({ code: 3, signal: null }, false)).toMatchObject({
      state: "failed",
      error: "interrupted",
    });
    expect(outcome({ code: null, signal: "SIGKILL" }, false)).toMatchObject({ state: "failed" });
    expect(outcome({ code: null, signal: "SIGKILL" }, true)).toMatchObject({ state: "cancelled" });
    expect(outcome({ code: 1, signal: null }, false)).toMatchObject({ state: "failed", exitCode: 1 });
  });

  test("a newer version pushed meanwhile: result still appended, with a warning", async () => {
    const v = v1;
    const { run: r, list } = await runOn(v, ["analyse"]);
    const note = list.find((e) => e.event === "hub-note" && e.kind === "newer-version");
    expect(note?.message).toMatch(/^v\d+ was pushed while this run was going/);
    const { v: res } = await snapshotOf(r.resultVersionId as string);
    const [{ n }] = await conn.db
      .select({ n: sql<number>`max(${projectVersion.number})` })
      .from(projectVersion)
      .where(eq(projectVersion.projectId, pid));
    expect(res.number).toBe(Number(n));
  });
});

describe("sweeps", () => {
  test("stuck verifying uploads are verified; abandoned uploads are aborted, with audit rows", async () => {
    const body = Buffer.from("client dump");
    const f = join(tmp, "dump.sql");
    writeFileSync(f, body);
    const h = createHash("sha256").update(body).digest("hex");
    const id1 = randomUUID();
    await putFile(keys.upload(pid, id1), f, h, body.length);
    const id2 = randomUUID();
    const mp = await createMultipart(keys.upload(pid, id2));
    const id3 = randomUUID(); // recent: left alone
    const old = (ms: number) => new Date(Date.now() - ms);
    await conn.db.insert(upload).values([
      {
        id: id1,
        projectId: pid,
        filename: "dump.sql",
        size: body.length,
        sha256: h,
        objectKey: keys.upload(pid, id1),
        state: "verifying",
        updatedAt: old(11 * 60_000),
      },
      {
        id: id2,
        projectId: pid,
        filename: "big.sql",
        size: 1,
        objectKey: keys.upload(pid, id2),
        multipartId: mp,
        state: "uploading",
        createdAt: old(8 * 24 * 3_600_000),
      },
      {
        id: id3,
        projectId: pid,
        filename: "new.sql",
        size: 1,
        objectKey: keys.upload(pid, id3),
        state: "uploading",
      },
    ]);
    // updated_at has $onUpdate, but insert keeps the given value.
    await sweepUploads(conn.db);
    const rows = await conn.db.select().from(upload).where(eq(upload.projectId, pid));
    const state = Object.fromEntries(rows.map((r) => [r.id, r.state]));
    expect(state[id1]).toBe("ready");
    expect(state[id2]).toBe("aborted");
    expect(state[id3]).toBe("uploading");
    const audit = await conn.db
      .select()
      .from(auditLog)
      .where(and(eq(auditLog.projectId, pid), eq(auditLog.targetType, "upload")));
    expect(audit.map((a) => [a.targetId, a.action]).sort()).toEqual(
      [
        [id1, "upload.ready"],
        [id2, "upload.abort"],
      ].sort(),
    );
  });
});
