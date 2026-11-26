import { createHash, randomUUID } from "node:crypto";
import { createReadStream } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { project, projectVersion, runSecret, upload, versionMedia } from "@porter-hub/db";
import {
  applySetup,
  PAYLOAD_KEY_ENV,
  readSetup,
  SANITY_TOKEN_ENV,
  WP_PASSWORD_ENV,
  WP_USER_ENV,
  wayOf,
} from "@porter-hub/porter/config";
import { encryptSecret } from "@porter-hub/porter/secrets";
import { packTarZst } from "@porter-hub/storage/tar";
import { and, desc, eq } from "drizzle-orm";
import { z } from "zod";
import type { Db } from "./permissions";
import { type Actor, access, audit, HttpError, parse, type Tx } from "./projects";
import { versionDir } from "./snapshot-reader";
import { deleteObject, keys, putFile } from "./storage";
import { latestOf, refuseWhileRunning } from "./versions";

/** An http(s) URL without credentials (porter refuses `user:pass@` too). */
const url = z
  .string()
  .trim()
  .max(2000)
  .refine((v) => {
    try {
      const u = new URL(v);
      return /^https?:$/.test(u.protocol) && !u.username && !u.password;
    } catch {
      return false;
    }
  }, "must be an http(s) URL without user:pass@");
const opt = <T extends z.ZodType>(t: T) =>
  z.preprocess((v) => (v === "" || v == null ? undefined : v), t.optional());
const secret = opt(
  z
    .string()
    .max(8192)
    .refine((v) => !v.includes("\0"), "must not contain NUL"),
);

const schema = z.object({
  adapter: z.string(),
  kinds: z.preprocess(
    (v) => (typeof v === "string" ? v.split(",").filter(Boolean) : v),
    z.array(z.string()).min(1, "choose at least one way in").max(8),
  ),
  upload: opt(z.uuid()),
  url: opt(url),
  projectId: opt(
    z
      .string()
      .trim()
      .regex(/^[a-z0-9-]{1,64}$/i, "letters, digits and hyphens"),
  ),
  dataset: opt(
    z
      .string()
      .trim()
      .regex(/^[a-z0-9_-]{1,64}$/i, "letters, digits, _ and -"),
  ),
  siteUrl: opt(url),
  destUrl: opt(url),
  /** Write-only: stored as run secrets, never in porter.toml. */
  sanityToken: secret,
  payloadKey: secret,
  wpUser: secret,
  wpPassword: secret,
});

const latestVersion = async (db: Db | Tx, projectId: string) =>
  (
    await db
      .select()
      .from(projectVersion)
      .where(eq(projectVersion.projectId, projectId))
      .orderBy(desc(projectVersion.number))
      .limit(1)
  )[0];

/** The setup form's current values (from the latest version's porter.toml), and the uploads it can pick from. */
export async function getSetup(db: Db, actor: Actor, slug: string) {
  const { project: p, role } = await access(db, actor, slug, "push");
  const v = await latestVersion(db, p.id);
  const toml = v ? await readFile(join(await versionDir(v), "porter.toml"), "utf8").catch(() => null) : null;
  const uploads = await db
    .select({
      id: upload.id,
      filename: upload.filename,
      purpose: upload.purpose,
      createdAt: upload.createdAt,
    })
    .from(upload)
    .where(and(eq(upload.projectId, p.id), eq(upload.state, "ready")))
    .orderBy(desc(upload.createdAt));
  const secrets = await db
    .select({ name: runSecret.name })
    .from(runSecret)
    .where(eq(runSecret.projectId, p.id));
  return {
    project: p,
    role,
    current: toml ? readSetup(toml) : null,
    version: v?.number ?? null,
    uploads,
    hasSanityToken: secrets.some((s) => s.name === SANITY_TOKEN_ENV),
    hasPayloadKey: secrets.some((s) => s.name === PAYLOAD_KEY_ENV),
    hasWpLogin: secrets.some((s) => s.name === WP_PASSWORD_ENV),
  };
}

const sha256File = async (path: string) => {
  const h = createHash("sha256");
  for await (const c of createReadStream(path)) h.update(c);
  return h.digest("hex");
};

/**
 * Applies the setup form to the latest porter.toml (or a fresh one) and commits it as a new version, so a
 * project set up on the hub can run without a push. The rest of the latest snapshot (project.db, raw/,
 * reports/, its media) carries over unchanged. Secrets go to run secrets.
 */
export async function saveSetup(db: Db, actor: Actor, slug: string, input: unknown) {
  // push is owners and developers, who may also manage secrets.
  const { project: p } = await access(db, actor, slug, "push");
  const s = parse(schema, input);
  const ways = s.kinds.map((k) => {
    const w = wayOf(s.adapter, k);
    if (!w) throw new HttpError(400, `kinds: ${k} is not a way into this source`);
    if (w.input === "local-only")
      throw new HttpError(400, `${w.label} runs locally only; run porter on your machine and push`);
    return w;
  });
  const uses = (input: string) => ways.some((w) => w.input === input);
  if (ways.filter((w) => w.input === "upload").length > 1)
    throw new HttpError(400, "kinds: choose one uploaded file at a time");
  if (uses("upload") && !s.upload) throw new HttpError(400, "upload: choose an uploaded file");
  if (uses("url") && !s.url) throw new HttpError(400, "url: the site URL is required");
  if (uses("sanity-api") && !(s.projectId && s.dataset))
    throw new HttpError(400, "projectId and dataset are required");
  if (uses("site") && !s.siteUrl) throw new HttpError(400, "siteUrl: the live site URL is required to crawl");

  let uploadName: string | undefined;
  if (uses("upload")) {
    const [u] = await db
      .select({ filename: upload.filename, state: upload.state })
      .from(upload)
      .where(and(eq(upload.id, s.upload as string), eq(upload.projectId, p.id)));
    if (u?.state !== "ready") throw new HttpError(400, "upload: not a finished upload on this project");
    uploadName = u.filename;
  }

  return commitToml(db, actor, p, (prev) => applySetup(prev, p.name, s, uploadName), {
    message: `source set up on the hub (${ways.map((w) => w.label).join(", ")})`,
    secrets: {
      [SANITY_TOKEN_ENV]: s.sanityToken,
      [PAYLOAD_KEY_ENV]: s.payloadKey,
      [WP_USER_ENV]: s.wpUser,
      [WP_PASSWORD_ENV]: s.wpPassword,
    },
    detail: { adapter: s.adapter, kinds: s.kinds },
  });
}

/**
 * Rewrites the latest porter.toml with `edit` (given null when there is no version yet) and commits it as a new
 * version, so a project can be set up and run from the hub without a push. The rest of the latest snapshot
 * (project.db, raw/, reports/, its media) carries over unchanged. `secrets` (empty values skipped) go to run
 * secrets in the same transaction. Refused while a run is queued or running.
 */
export async function commitToml(
  db: Db,
  actor: Actor,
  p: { id: string },
  edit: (prev: string | null) => string,
  opts: { message: string; secrets?: Record<string, string | undefined>; detail?: object },
) {
  await refuseWhileRunning(db, p.id);
  const base = await latestVersion(db, p.id);
  const baseDir = base ? await versionDir(base) : null;
  const prev = baseDir ? await readFile(join(baseDir, "porter.toml"), "utf8").catch(() => null) : null;
  let toml: string;
  try {
    toml = edit(prev);
  } catch {
    throw new HttpError(409, "The latest porter.toml can't be read; fix it locally and push");
  }

  const tmp = await mkdtemp(join(tmpdir(), "porter-setup-"));
  const snapshotKey = keys.snapshot(p.id, randomUUID());
  let committed = false;
  try {
    const out = join(tmp, "snapshot.tar.zst");
    if (baseDir) await packTarZst(baseDir, out, () => false, { "porter.toml": Buffer.from(toml) });
    else {
      const dir = join(tmp, "project");
      await mkdir(dir);
      await writeFile(join(dir, "porter.toml"), toml);
      await packTarZst(dir, out);
    }
    const sha = await sha256File(out);
    const { size } = await stat(out);
    await putFile(snapshotKey, out, sha, size);

    const n = await db.transaction(async (tx) => {
      await tx.select({ id: project.id }).from(project).where(eq(project.id, p.id)).for("update");
      await refuseWhileRunning(tx, p.id);
      const latest = await latestOf(tx, p.id);
      if (latest !== (base?.number ?? null))
        throw new HttpError(409, "A new version arrived while saving; reload and try again");
      const number = (latest ?? 0) + 1;
      const [v] = await tx
        .insert(projectVersion)
        .values({
          projectId: p.id,
          number,
          snapshotKey,
          snapshotSha256: sha,
          porterVersion: base?.porterVersion ?? null,
          createdBy: actor.id,
          size,
          // The old stage views were judged against the old porter.toml; the hub re-derives them from project.db.
          summary: null,
          message: opts.message,
        })
        .returning({ id: projectVersion.id });
      if (base) {
        const media = await tx
          .select({ sha256: versionMedia.sha256 })
          .from(versionMedia)
          .where(eq(versionMedia.versionId, base.id));
        for (let i = 0; i < media.length; i += 5000)
          await tx
            .insert(versionMedia)
            .values(
              media.slice(i, i + 5000).map((m) => ({ versionId: v.id, projectId: p.id, sha256: m.sha256 })),
            );
      }
      for (const [name, value] of Object.entries(opts.secrets ?? {})) {
        if (!value) continue;
        const enc = encryptSecret(p.id, name, value);
        await tx
          .insert(runSecret)
          .values({ projectId: p.id, name, ...enc, createdBy: actor.id })
          .onConflictDoUpdate({
            target: [runSecret.projectId, runSecret.name],
            set: { ...enc, createdBy: actor.id, updatedAt: new Date() },
          });
        await audit(tx, actor, p.id, "secret.put", name);
      }
      await audit(tx, actor, p.id, "version.setup", v.id, { number, ...opts.detail });
      return number;
    });
    committed = true;
    return { number: n };
  } finally {
    if (!committed) await deleteObject(snapshotKey).catch(() => {});
    await rm(tmp, { recursive: true, force: true });
  }
}
