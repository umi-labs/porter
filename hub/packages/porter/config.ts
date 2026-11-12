import { parse, stringify } from "smol-toml";

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
