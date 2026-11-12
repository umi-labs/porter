import { createHash, randomUUID } from "node:crypto";
import { createWriteStream, existsSync, readdirSync } from "node:fs";
import { mkdir, rename, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import { createZstdDecompress } from "node:zlib";
import { projectVersion } from "@porter-hub/db";
import { desc, eq } from "drizzle-orm";
import * as tar from "tar-stream";
import type { Db } from "./permissions";
import { getStream } from "./storage";

// node:sqlite (built in, no native build) and zlib's zstd both need Node >= 22.15; the Dockerfile must use at least that.

export const PAGE_SIZE = 100;
const MAX_ENTRIES = 200_000;
const cacheDir = () => process.env.HUB_CACHE_DIR || join(tmpdir(), "porter-hub-cache");
const maxBytes = () => Number(process.env.HUB_SNAPSHOT_MAX_BYTES) || 2 * 1024 ** 3;

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

/** Passes bytes through, failing once more than `limit` have gone by. */
const capBytes = (limit: number, what: string, seen: { n: number }) =>
  new Transform({
    transform(chunk, _enc, cb) {
      seen.n += chunk.length;
      cb(seen.n > limit ? new Error(`${what} is larger than ${limit} bytes`) : null, chunk);
    },
  });

/**
 * Streams a tar.zst (zstd -> tar parser -> files) into `dest`; memory stays O(chunk), the event loop is never blocked.
 * Only regular files and directories are accepted: a symlink, hardlink or device entry, an absolute path or a
 * `..` rejects the whole archive. The cap counts bytes actually decompressed and written, not sizes the archive declares.
 * If `hash` is given it is fed the compressed bytes.
 */
export async function extractTarZst(
  source: Buffer | Readable,
  dest: string,
  limit = maxBytes(),
  hash?: ReturnType<typeof createHash>,
) {
  const root = resolve(dest);
  await mkdir(root, { recursive: true });
  const input = Buffer.isBuffer(source) ? Readable.from([source]) : source;
  if (hash) input.on("data", (c) => hash.update(c));
  const extract = tar.extract();
  const written = { n: 0 };
  const run = pipeline(
    input,
    createZstdDecompress(),
    capBytes(limit + 1024 * 1024, "snapshot", { n: 0 }),
    extract,
  );
  const walk = (async () => {
    let entries = 0;
    for await (const entry of extract) {
      const { name, type } = entry.header;
      if (++entries > MAX_ENTRIES) throw new Error("snapshot has too many entries");
      if (type !== "file" && type !== "directory") {
        throw new Error(
          `snapshot entry ${JSON.stringify(name)} has unsupported type ${JSON.stringify(type)}`,
        );
      }
      const rel = safeName(name);
      const out = resolve(root, rel);
      if (!rel && type === "directory") {
        entry.resume();
        continue;
      }
      if (!rel || !out.startsWith(root + sep))
        throw new Error(`unsafe path in snapshot: ${JSON.stringify(name)}`);
      if (type === "directory") {
        await mkdir(out, { recursive: true, mode: 0o755 });
        entry.resume();
      } else {
        await mkdir(dirname(out), { recursive: true, mode: 0o755 });
        await pipeline(entry, capBytes(limit, "snapshot", written), createWriteStream(out, { mode: 0o644 }));
      }
    }
  })();
  // Whichever side fails first rejects; tear the source down so S3 stops sending.
  try {
    await Promise.all([run, walk]);
  } catch (e) {
    input.destroy();
    extract.destroy();
    throw e;
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
  // Single-flight per version: concurrent requests share one extraction. A failure clears the entry so the next request retries.
  let p = inflight.get(v.id);
  if (!p) {
    p = (async () => {
      if (existsSync(join(final, "project.db"))) return final;
      // Temp dir then atomic rename: a concurrent request in another process sees all of it or none.
      const tmp = join(cacheDir(), `.tmp-${v.id}-${randomUUID()}`);
      try {
        const hash = createHash("sha256");
        await extractTarZst(await getStream(v.snapshotKey), tmp, maxBytes(), hash);
        if (v.snapshotSha256 && hash.digest("hex") !== v.snapshotSha256) {
          throw new Error("stored snapshot does not match its checksum");
        }
        try {
          await rename(tmp, final);
        } catch (e) {
          const code = (e as { code?: string }).code;
          if (code !== "ENOTEMPTY" && code !== "EEXIST") throw e; // lost the race: the winner's copy is identical
        }
      } finally {
        await rm(tmp, { recursive: true, force: true });
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
/** Runs `fn` against project.db opened read-only; null only for "no such table/column" (older snapshot). Other errors throw. */
export function withDb<T>(
  dir: string,
  fn: (q: (sql: string, ...a: (string | number)[]) => Row[]) => T,
): T | null {
  let db: DatabaseSync | undefined;
  try {
    db = new DatabaseSync(join(dir, "project.db"), { readOnly: true });
    const d = db;
    return fn((sql, ...a) => d.prepare(sql).all(...a) as Row[]);
  } catch (e) {
    // Only an older snapshot lacking a table or column is "not available"; corrupt or missing databases and bugs propagate.
    if (/no such (table|column)/i.test((e as Error).message)) return null;
    throw e;
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

/** Issues, open and closed. `full` (view_raw members) adds message, stage, resolution and item id. Share views get only kind, severity and item title: messages can echo field or submission values. */
export function issuesPage(dir: string, page: number, full: boolean) {
  return withDb(dir, (q) => {
    const only = full ? "" : "AND resolution = 'open'"; // shares show open issues only; the resolution tag stays hidden
    const total = Number(q(`SELECT count(*) AS n FROM issues WHERE deleted_at IS NULL ${only}`)[0].n);
    const rows = q(
      `SELECT i.kind, i.severity, i.message, i.stage, i.resolution, i.item_id,
         (SELECT coalesce(json_extract(t.data, '$.title'), json_extract(t.data, '$.filename'))
            FROM items t WHERE t.id = i.item_id) AS item_title
       FROM issues i WHERE i.deleted_at IS NULL ${only.replace("resolution", "i.resolution")} ORDER BY i.id LIMIT ${PAGE_SIZE} OFFSET ?`,
      (page - 1) * PAGE_SIZE,
    ).map((r) => ({
      kind: String(r.kind),
      severity: String(r.severity),
      itemTitle: r.item_title == null ? null : String(r.item_title),
      ...(full
        ? {
            message: String(r.message),
            stage: String(r.stage),
            resolution: String(r.resolution),
            itemId: String(r.item_id),
          }
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
