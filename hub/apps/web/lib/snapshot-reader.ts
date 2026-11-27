import { createHash, randomUUID } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { rename, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { projectVersion } from "@porter-hub/db";
import { readSetup } from "@porter-hub/porter/config";
import { extractTarZst } from "@porter-hub/storage/tar";
import { desc, eq } from "drizzle-orm";
import type { Db } from "./permissions";
import { parseSummary, type RunRow, runState, stageViews, storedViews } from "./stage-state";
import { getStream } from "./storage";

// node:sqlite (built in, no native build) and zlib's zstd both need Node >= 22.15; the Dockerfile must use at least that.

/** Moved to @porter-hub/storage (shared with the worker); re-exported for existing callers. */
export { extractTarZst };
export const PAGE_SIZE = 100;
const cacheDir = () => process.env.HUB_CACHE_DIR || join(tmpdir(), "porter-hub-cache");
const maxBytes = () => Number(process.env.HUB_SNAPSHOT_MAX_BYTES) || 2 * 1024 ** 3;

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
  return {
    dir: await versionDir(v),
    version: {
      id: v.id,
      number: v.number,
      createdAt: v.createdAt,
      message: v.message,
      porterVersion: v.porterVersion,
      /** `porter --json status` as stored with the version; its `stage_views` are porter's own. */
      summary: v.summary as unknown,
    },
  };
}
export type Snapshot = NonNullable<Awaited<ReturnType<typeof latestSnapshot>>>;

/** One version's extracted snapshot dir, cached by version id. */
export async function versionDir(v: { id: string; snapshotKey: string; snapshotSha256: string | null }) {
  const final = join(cacheDir(), v.id);
  // Single-flight per version: concurrent requests share one extraction. A failure clears the entry so the next request retries.
  let p = inflight.get(v.id);
  if (!p) {
    p = (async () => {
      if (existsSync(join(final, "porter.toml"))) return final; // every snapshot has one
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
  return p;
}

type Row = Record<string, string | number | null>;

/**
 * Routes as the engine lists them: only the source adapter's own (normalised) rows. Each connector
 * (crawl, wp-rest, ...) also stores its raw copy of a URL, so counting every row shows each URL twice.
 */
const ROUTES = "kind = 'route' AND connector = ? AND deleted_at IS NULL";
const R_ROUTES = "r.kind = 'route' AND r.connector = ? AND r.deleted_at IS NULL";
const adapterOf = (dir: string) => {
  try {
    return readSetup(readFileSync(join(dir, "porter.toml"), "utf8"))?.adapter ?? "";
  } catch {
    return "";
  }
};
/** Runs `fn` against project.db opened read-only; null only for "no such table/column" (older snapshot). Other errors throw. */
export function withDb<T>(
  dir: string,
  fn: (q: (sql: string, ...a: (string | number)[]) => Row[]) => T,
): T | null {
  // A version set up on the hub has no project.db until its first run.
  if (!existsSync(join(dir, "project.db"))) return null;
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

const runRow = (r: Row): RunRow => ({
  id: String(r.id),
  stage: String(r.stage),
  startedAt: String(r.started_at),
  endedAt: r.ended_at == null ? null : String(r.ended_at),
  summary: parseSummary(r.summary),
  dryRun: "dry_run" in r ? r.dry_run === 1 : null,
  configFp: r.config_fp == null ? null : String(r.config_fp),
});

/** `summary`: the version's stored `porter --json status`, whose stage views win over the hub's own derivation. */
export function overview(dir: string, summary?: unknown) {
  const adapter = adapterOf(dir);
  const last = withDb(dir, (q) => {
    const m = new Map<string, Row>();
    for (const r of q("SELECT * FROM runs WHERE deleted_at IS NULL ORDER BY id")) m.set(String(r.stage), r);
    return [...m.values()];
  });
  const rows = last?.map(runRow) ?? null;
  return {
    stages:
      last?.map((r) => ({
        stage: String(r.stage),
        startedAt: String(r.started_at),
        endedAt: r.ended_at == null ? null : String(r.ended_at),
        dryRun: r.dry_run === 1,
      })) ?? null,
    /** Each pipeline stage's state: porter's own when stored, else derived like porter does (lib/stage-state.ts). */
    views: rows && (storedViews(summary, rows) ?? stageViews(rows)),
    /**
     * What has reached the site, whatever the last load was. `everWritten`: a load that was not a dry run finished
     * (the engine's `ever_written`). `lastReal`: the state of the latest load that was not a dry run, or null.
     * null for an older snapshot that doesn't record dry runs.
     */
    writes: withDb(dir, (q) => {
      const ever = q(
        `SELECT 1 FROM runs WHERE stage = 'load' AND dry_run = 0 AND summary IS NOT NULL AND deleted_at IS NULL LIMIT 1`,
      );
      const [real] = q(
        `SELECT * FROM runs WHERE stage = 'load' AND dry_run = 0 AND deleted_at IS NULL ORDER BY id DESC LIMIT 1`,
      );
      return { everWritten: ever.length > 0, lastReal: real ? runState(runRow(real)) : null };
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
         FROM items WHERE ${ROUTES}`,
        adapter,
      );
      return { routes: Number(r.total), mapped: Number(r.mapped) };
    }),
  };
}

export type IssueFilter = { kind?: string; severity?: string; status?: "open" | "all"; q?: string };
const like = (q: string) => `%${q.replace(/[\\%_]/g, (c) => `\\${c}`)}%`;

/**
 * Issues, filtered and paged, with per-kind and per-severity counts for the filter chips (counted under
 * the other filters, so each chip says what clicking it would show). `full` (view_raw members) adds
 * message, stage, resolution and item id, and allows searching messages. Share views get only kind,
 * severity and item title, open issues only: messages can echo field or submission values.
 */
export function issuesPage(dir: string, page: number, full: boolean, f: IssueFilter = {}) {
  return withDb(dir, (q) => {
    const where = (skip?: "kind" | "severity") => {
      const w = ["i.deleted_at IS NULL"];
      const a: string[] = [];
      if (!full || f.status !== "all") w.push("i.resolution = 'open'");
      const add = (sql: string, v: string) => {
        w.push(sql);
        a.push(v);
      };
      if (f.kind && skip !== "kind") add("i.kind = ?", f.kind);
      if (f.severity && skip !== "severity") add("i.severity = ?", f.severity);
      if (full && f.q) add("i.message LIKE ? ESCAPE '\\'", like(f.q));
      return { sql: w.join(" AND "), a };
    };
    const count = (by: "kind" | "severity") => {
      const { sql, a } = where(by);
      return q(
        `SELECT i.${by} AS k, count(*) AS n FROM issues i WHERE ${sql} GROUP BY i.${by} ORDER BY n DESC, k`,
        ...a,
      ).map((r) => ({ key: String(r.k), n: Number(r.n) }));
    };
    const { sql, a } = where();
    const total = Number(q(`SELECT count(*) AS n FROM issues i WHERE ${sql}`, ...a)[0].n);
    const rows = q(
      `SELECT i.kind, i.severity, i.message, i.stage, i.resolution, i.item_id,
         (SELECT coalesce(json_extract(t.data, '$.title'), json_extract(t.data, '$.filename'))
            FROM items t WHERE t.id = i.item_id) AS item_title
       FROM issues i WHERE ${sql}
       ORDER BY CASE i.severity WHEN 'error' THEN 0 WHEN 'warning' THEN 1 ELSE 2 END, i.id LIMIT ${PAGE_SIZE} OFFSET ?`,
      ...a,
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
    return { ...pageOf(rows, total, page), kinds: count("kind"), severities: count("severity") };
  });
}

type Sp = Record<string, string | string[] | undefined>;
const one = (v: string | string[] | undefined) =>
  typeof v === "string" && v.trim() ? v.trim().slice(0, 200) : undefined;
/** Filters from a page's search params; anything unexpected is ignored. */
export const issueFilter = (sp: Sp): IssueFilter => ({
  kind: one(sp.kind),
  severity: one(sp.severity),
  status: sp.status === "all" ? "all" : "open",
  q: one(sp.q),
});
export const urlFilter = (sp: Sp): UrlFilter => ({
  show: sp.show === "mapped" || sp.show === "unmapped" ? sp.show : undefined,
  q: one(sp.q),
});

export type UrlFilter = { show?: "unmapped" | "mapped"; q?: string };

/** The adapter's routes (see ROUTES), filtered and paged; `counts` are per `show` under the search. */
export function urlsPage(dir: string, page: number, f: UrlFilter = {}) {
  const adapter = adapterOf(dir);
  return withDb(dir, (q) => {
    const MAPPED = "json_type(r.data, '$.target') = 'object'";
    const search = f.q ? " AND json_extract(r.data, '$.url') LIKE ? ESCAPE '\\'" : "";
    const sa = f.q ? [like(f.q)] : [];
    const [c] = q(
      `SELECT count(*) AS total, coalesce(sum(${MAPPED}), 0) AS mapped FROM items r WHERE ${R_ROUTES}${search}`,
      adapter,
      ...sa,
    );
    const counts = {
      all: Number(c.total),
      mapped: Number(c.mapped),
      unmapped: Number(c.total) - Number(c.mapped),
    };
    const show = f.show === "mapped" ? ` AND ${MAPPED}` : f.show === "unmapped" ? ` AND NOT ${MAPPED}` : "";
    const rows = q(
      `SELECT json_extract(r.data, '$.url') AS url, json_extract(r.data, '$.http_status') AS status,
         CASE json_type(r.data, '$.target') WHEN 'object' THEN (SELECT key FROM json_each(r.data, '$.target')) ELSE 'none' END AS target,
         (SELECT coalesce(json_extract(t.data, '$.title'), json_extract(t.data, '$.filename'), json_extract(t.data, '$.url'))
            FROM items t WHERE t.id = (SELECT value FROM json_each(r.data, '$.target'))) AS target_title
       FROM items r WHERE ${R_ROUTES}${search}${show}
       ORDER BY lower(coalesce(json_extract(r.data, '$.url'), '')), r.id LIMIT ${PAGE_SIZE} OFFSET ?`,
      adapter,
      ...sa,
      (page - 1) * PAGE_SIZE,
    ).map((r) => ({
      url: String(r.url ?? ""),
      status: r.status == null ? null : Number(r.status),
      target: String(r.target),
      targetTitle: r.target_title == null ? null : String(r.target_title),
    }));
    const total = f.show ? counts[f.show] : counts.all;
    return { ...pageOf(rows, total, page), counts };
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
