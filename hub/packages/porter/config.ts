import { parse, stringify } from "smol-toml";
import { PAYLOAD_KEY_ENV, SANITY_TOKEN_ENV, type Setup, siteRoot } from "./sources";

export * from "./sources";
export * from "./stages";

type Toml = Record<string, unknown>;

/** The host of `[destination] url` in a porter.toml, or null if it has none (or it isn't a URL). */
export function destinationHost(toml: string) {
  const url = ((parse(toml) as Toml).destination as Toml | undefined)?.url;
  if (typeof url !== "string") return null;
  try {
    return new URL(url).host || null;
  } catch {
    return null;
  }
}

/**
 * Drops every `[[connectors]]` block with `kind = "wp-db"` (the hub never runs it). Returns the rewritten
 * TOML (null when nothing was removed) and how many connectors are left.
 */
export function stripWpDb(toml: string) {
  const doc = parse(toml) as Toml;
  const all = Array.isArray(doc.connectors) ? (doc.connectors as Toml[]) : [];
  const kept = all.filter((c) => c?.kind !== "wp-db");
  if (kept.length === all.length) return { toml: null, removed: 0, left: kept.length };
  doc.connectors = kept;
  if (!kept.length) delete doc.connectors;
  return { toml: stringify(doc), removed: all.length - kept.length, left: kept.length };
}

/** Where the worker puts an upload a connector names: `uploads/<upload id>/<filename>`, relative to the project. */
export const uploadPath = (id: string, filename: string) => `uploads/${id}/${filename}`;
const UPLOAD_PATH = /^uploads\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\/([^/\\]+)$/;

/** Every hub upload a porter.toml's connectors read, as `{ id, path }`. Other paths are left to porter. */
export function uploadRefs(toml: string) {
  const doc = parse(toml) as Toml;
  const all = Array.isArray(doc.connectors) ? (doc.connectors as Toml[]) : [];
  const paths = all.flatMap((c) => [c?.path, c?.dump, ...(Array.isArray(c?.paths) ? c.paths : [])]);
  return paths.flatMap((p) => {
    const m = typeof p === "string" ? UPLOAD_PATH.exec(p) : null;
    return m && m[2] !== "." && m[2] !== ".." ? [{ id: m[1], path: p as string }] : [];
  });
}

/**
 * The hub setup form applied to a porter.toml (`prev`, or a fresh one): sets the project name, source,
 * connectors, destination and verify site. Everything else (mapping, tidy rules, redirects) is kept.
 * `uploadName` is the chosen upload's filename.
 */
export function applySetup(prev: string | null, name: string, s: Setup, uploadName?: string) {
  const doc = (prev ? parse(prev) : {}) as Toml;
  const project = (doc.project ?? {}) as Toml;
  doc.project = { ...project, name: (project.name as string) || name };
  doc.source = { ...((doc.source ?? {}) as Toml), adapter: s.adapter };
  const connectors: Toml[] = [];
  const file = s.upload && uploadName ? uploadPath(s.upload, uploadName) : undefined;
  const site = s.siteUrl ? siteRoot(s.siteUrl) : undefined;
  for (const kind of s.kinds) {
    if (kind === "wxr" || kind === "sanity-export") connectors.push({ kind, path: file });
    if (kind === "umbraco-json") connectors.push({ kind, paths: [file] });
    if (kind === "wp-rest") connectors.push({ kind, url: s.url });
    if (kind === "sanity-api")
      connectors.push({ kind, project_id: s.projectId, dataset: s.dataset, token_env: SANITY_TOKEN_ENV });
    if (kind === "crawl" && site) connectors.push({ kind, url: site });
  }
  doc.connectors = connectors;
  // The form shows verify.site_url as the live site, so clearing it there clears it here too.
  const verify = { ...((doc.verify ?? {}) as Toml), site_url: site };
  if (!site) delete verify.site_url;
  if (Object.keys(verify).length) doc.verify = verify;
  else delete doc.verify;
  if (s.destUrl)
    doc.destination = {
      ...((doc.destination ?? {}) as Toml),
      adapter: "payload",
      url: s.destUrl,
      api_key_env: PAYLOAD_KEY_ENV,
    };
  return stringify(doc);
}

/** The setup form's values read back from a porter.toml (null if it can't be parsed). */
export function readSetup(toml: string): (Setup & { uploadName?: string }) | null {
  let doc: Toml;
  try {
    doc = parse(toml) as Toml;
  } catch {
    return null;
  }
  const all = Array.isArray(doc.connectors) ? (doc.connectors as Toml[]) : [];
  const of = (kind: string) => all.find((c) => c?.kind === kind);
  const str = (v: unknown) => (typeof v === "string" ? v : undefined);
  const file = all
    .map((c) => str(c?.path) ?? str(c?.dump) ?? (Array.isArray(c?.paths) ? str(c.paths[0]) : undefined))
    .find(Boolean);
  const m = file ? UPLOAD_PATH.exec(file) : null;
  return {
    adapter: str((doc.source as Toml | undefined)?.adapter) ?? "",
    kinds: [...new Set(all.map((c) => str(c?.kind)).filter((k): k is string => !!k))],
    upload: m?.[1],
    uploadName: m?.[2],
    url: str(of("wp-rest")?.url),
    projectId: str(of("sanity-api")?.project_id),
    dataset: str(of("sanity-api")?.dataset),
    siteUrl: str(of("crawl")?.url) ?? str((doc.verify as Toml | undefined)?.site_url),
    destUrl: str((doc.destination as Toml | undefined)?.url),
  };
}

/** Sets `[destination]` to a Payload site (key from the PAYLOAD_API_KEY run secret); nothing else changes. */
export function applyDestination(prev: string | null, name: string, url: string) {
  const doc = (prev ? parse(prev) : {}) as Toml;
  const project = (doc.project ?? {}) as Toml;
  doc.project = { ...project, name: (project.name as string) || name };
  doc.destination = {
    ...((doc.destination ?? {}) as Toml),
    adapter: "payload",
    url,
    api_key_env: PAYLOAD_KEY_ENV,
  };
  return stringify(doc);
}
