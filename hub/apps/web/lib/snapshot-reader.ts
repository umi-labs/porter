import { createHash, randomUUID } from "node:crypto";
import { mkdirSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { zstdDecompressSync } from "node:zlib";
import { projectVersion } from "@porter-hub/db";
import { desc, eq } from "drizzle-orm";
import type { Db } from "./permissions";
import { getBuffer } from "./storage";

// node:sqlite (built in, no native build) and zlib's zstd both need Node >= 22.15; the Dockerfile must use at least that.

export const PAGE_SIZE = 100;
const cacheDir = () => process.env.HUB_CACHE_DIR || join(tmpdir(), "porter-hub-cache");
const maxBytes = () => Number(process.env.HUB_SNAPSHOT_MAX_BYTES) || 2 * 1024 ** 3;

const octal = (b: Buffer) => {
  if (b[0] & 0x80) return Number(b.readBigUInt64BE(b.length - 8) & 0x7fffffffffffffffn); // base-256
  return Number.parseInt(b.toString("latin1").replace(/\0.*$/s, "").trim() || "0", 8);
};
const cstr = (b: Buffer) => b.toString("utf8").replace(/\0.*$/s, "");

/** Throws unless `name` is a relative path with no `..`; returns it without a leading ./ or trailing /. "" is the archive root ("./"). */
function safeName(name: string) {
  const n = name.replace(/^(\.\/)+/, "").replace(/\/+$/, "");
  if (
    name.startsWith("/") ||
    n.includes("\0") ||
    n.split("/").some((s) => s === ".." || (s === "" && n !== ""))
  ) {
    throw new Error(`unsafe path in snapshot: ${JSON.stringify(name)}`);
  }
  return n === "." ? "" : n;
}

/**
 * Extracts a tar.zst into `dest`. Only regular files and directories are accepted (a symlink, hardlink or
 * device entry rejects the whole archive), as are paths that are absolute or contain `..`. Decompressed size
 * is capped, which also bounds a decompression bomb.
 */
export function extractTarZst(archive: Buffer, dest: string, limit = maxBytes()) {
  const tar = zstdDecompressSync(archive, { maxOutputLength: limit + 1024 * 1024 });
  const root = resolve(dest);
  mkdirSync(root, { recursive: true });
  let total = 0;
  let longName: string | undefined;
  let paxPath: string | undefined;
  for (let off = 0; off + 512 <= tar.length; ) {
    const h = tar.subarray(off, off + 512);
    if (h.every((x) => x === 0)) break;
    let size = octal(h.subarray(124, 136));
    const type = String.fromCharCode(h[156] || 48);
    const body = () => tar.subarray(off + 512, off + 512 + size);
    const next = off + 512 + Math.ceil(size / 512) * 512;
    if (next > tar.length) throw new Error("truncated tar");
    if (type === "L") {
      longName = cstr(body());
    } else if (type === "x") {
      for (const m of body()
        .toString("utf8")
        .matchAll(/\d+ (\w+)=([^\n]*)\n/g)) {
        if (m[1] === "path") paxPath = m[2];
        if (m[1] === "size") size = Number(m[2]);
      }
    } else if (type === "g") {
      // global pax header: nothing we need
    } else {
      const prefix = cstr(h.subarray(345, 500));
      const raw =
        paxPath ??
        longName ??
        (prefix && cstr(h.subarray(257, 262)) === "ustar" ? `${prefix}/` : "") + cstr(h.subarray(0, 100));
      paxPath = longName = undefined;
      if (type !== "0" && type !== "5")
        throw new Error(`snapshot entry ${JSON.stringify(raw)} has unsupported type ${JSON.stringify(type)}`);
      const rel = safeName(raw);
      if (!rel && type === "5") {
        off = next;
        continue;
      }
      const out = resolve(root, rel);
      if (!rel || !out.startsWith(root + sep))
        throw new Error(`unsafe path in snapshot: ${JSON.stringify(raw)}`);
      if (type === "5") mkdirSync(out, { recursive: true, mode: 0o755 });
      else {
        total += size;
        if (total > limit) throw new Error(`snapshot is larger than ${limit} bytes`);
        mkdirSync(dirname(out), { recursive: true, mode: 0o755 });
        writeFileSync(out, body(), { mode: 0o644 });
      }
    }
    off = next;
  }
}

const inflight = new Map<string, Promise<string>>();

/** The latest version's extracted snapshot dir (cached by version id), or null if the project has no versions. */
export async function latestSnapshot(db: Db, projectId: string) {
  const [v] = await db
    .select()
    .from(projectVersion)
    .where(eq(projectVersion.projectId, projectId))
    .orderBy(desc(projectVersion.number))
    .limit(1);
  if (!v) return null;
  const final = join(cacheDir(), v.id);
  let p = inflight.get(v.id);
  if (!p) {
    p = (async () => {
      try {
        readdirSync(final);
        return final;
      } catch {}
      const buf = await getBuffer(v.snapshotKey);
      if (v.snapshotSha256 && createHash("sha256").update(buf).digest("hex") !== v.snapshotSha256) {
        throw new Error("stored snapshot does not match its checksum");
      }
      // Temp dir then atomic rename: a concurrent request in another process sees all of it or none.
      const tmp = join(cacheDir(), `.tmp-${v.id}-${randomUUID()}`);
      try {
        extractTarZst(buf, tmp);
        try {
          renameSync(tmp, final);
        } catch (e) {
          const code = (e as { code?: string }).code;
          if (code !== "ENOTEMPTY" && code !== "EEXIST") throw e; // lost the race: the winner's copy is identical
        }
      } finally {
        rmSync(tmp, { recursive: true, force: true });
      }
      return final;
    })().finally(() => inflight.delete(v.id));
    inflight.set(v.id, p);
  }
  return {
    dir: await p,
    version: {
      id: v.id,
      number: v.number,
      createdAt: v.createdAt,
      message: v.message,
      porterVersion: v.porterVersion,
    },
  };
}
export type Snapshot = NonNullable<Awaited<ReturnType<typeof latestSnapshot>>>;

type Row = Record<string, string | number | null>;
/** Runs `fn` against project.db opened read-only; null when the table or column is missing (older snapshot). */
export function withDb<T>(
  dir: string,
  fn: (q: (sql: string, ...a: (string | number)[]) => Row[]) => T,
): T | null {
  let db: DatabaseSync | undefined;
  try {
    db = new DatabaseSync(join(dir, "project.db"), { readOnly: true });
    const d = db;
    return fn((sql, ...a) => d.prepare(sql).all(...a) as Row[]);
  } catch {
    return null;
  } finally {
    db?.close();
  }
}

export type Page<T> = { rows: T[]; total: number; page: number; pages: number };
const pageOf = <T>(rows: T[], total: number, page: number): Page<T> => ({
  rows,
  total,
  page,
  pages: Math.max(1, Math.ceil(total / PAGE_SIZE)),
});
/** Page number from a query string value: a positive integer, else 1. */
export const pageNum = (s: unknown) => {
  const n = Number(s);
  return Number.isInteger(n) && n >= 1 && n <= 1e6 ? n : 1;
};

export function overview(dir: string) {
  return {
    stages: withDb(dir, (q) => {
      const last = new Map<string, Row>();
      for (const r of q("SELECT * FROM runs WHERE deleted_at IS NULL ORDER BY id"))
        last.set(String(r.stage), r);
      return [...last.values()].map((r) => ({
        stage: String(r.stage),
        startedAt: String(r.started_at),
        endedAt: r.ended_at == null ? null : String(r.ended_at),
        dryRun: r.dry_run === 1,
      }));
    }),
    issueCounts: withDb(dir, (q) =>
      q(
        `SELECT kind, severity, count(*) AS n FROM issues
         WHERE deleted_at IS NULL AND resolution = 'open' GROUP BY kind, severity ORDER BY kind, severity`,
      ).map((r) => ({ kind: String(r.kind), severity: String(r.severity), n: Number(r.n) })),
    ),
    coverage: withDb(dir, (q) => {
      const [r] = q(
        `SELECT count(*) AS total, coalesce(sum(json_type(data, '$.target') = 'object'), 0) AS mapped
         FROM items WHERE kind = 'route' AND deleted_at IS NULL`,
      );
      return { routes: Number(r.total), mapped: Number(r.mapped) };
    }),
  };
}

/** Issues, open and closed. `full` adds stage, resolution and item id (staff); without it only kind, severity, message, item title. */
export function issuesPage(dir: string, page: number, full: boolean) {
  return withDb(dir, (q) => {
    const total = Number(q("SELECT count(*) AS n FROM issues WHERE deleted_at IS NULL")[0].n);
    const rows = q(
      `SELECT i.kind, i.severity, i.message, i.stage, i.resolution, i.item_id,
         (SELECT coalesce(json_extract(t.data, '$.title'), json_extract(t.data, '$.filename'))
            FROM items t WHERE t.id = i.item_id) AS item_title
       FROM issues i WHERE i.deleted_at IS NULL ORDER BY i.id LIMIT ${PAGE_SIZE} OFFSET ?`,
      (page - 1) * PAGE_SIZE,
    ).map((r) => ({
      kind: String(r.kind),
      severity: String(r.severity),
      message: String(r.message),
      itemTitle: r.item_title == null ? null : String(r.item_title),
      ...(full
        ? { stage: String(r.stage), resolution: String(r.resolution), itemId: String(r.item_id) }
        : {}),
    }));
    return pageOf(rows, total, page);
  });
}

export function urlsPage(dir: string, page: number) {
  return withDb(dir, (q) => {
    const total = Number(
      q("SELECT count(*) AS n FROM items WHERE kind = 'route' AND deleted_at IS NULL")[0].n,
    );
    const rows = q(
      `SELECT json_extract(data, '$.url') AS url, json_extract(data, '$.http_status') AS status,
         CASE json_type(data, '$.target') WHEN 'object' THEN (SELECT key FROM json_each(data, '$.target')) ELSE 'none' END AS target
       FROM items WHERE kind = 'route' AND deleted_at IS NULL
       ORDER BY lower(coalesce(json_extract(data, '$.url'), '')), id LIMIT ${PAGE_SIZE} OFFSET ?`,
      (page - 1) * PAGE_SIZE,
    ).map((r) => ({
      url: String(r.url ?? ""),
      status: r.status == null ? null : Number(r.status),
      target: String(r.target),
    }));
    return pageOf(rows, total, page);
  });
}

/** Names of the .html/.json files directly under reports/. This list is the only way a name reaches the filesystem. */
export function listReports(dir: string) {
  try {
    return readdirSync(join(dir, "reports"), { withFileTypes: true })
      .filter((e) => e.isFile() && /\.(html|json)$/.test(e.name))
      .map((e) => e.name)
      .sort();
  } catch {
    return [];
  }
}
