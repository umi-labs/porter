import { readFileSync } from "node:fs";
import { join } from "node:path";
import { listReports } from "./snapshot-reader";

export const REPORT_HEADERS = {
  "Content-Security-Policy": "sandbox",
  "X-Content-Type-Options": "nosniff",
  "Cache-Control": "private, no-store",
};
/** Extra headers on every response under /s/. */
export const SHARE_HEADERS = {
  "Referrer-Policy": "no-referrer",
  "X-Robots-Tag": "noindex",
  "Cache-Control": "private, no-store",
};

/** Serves one report. `name` must equal a file listed in reports/; it is never joined to a path unchecked. */
export function serveReport(dir: string, name: string, extra: Record<string, string> = {}) {
  const hit = listReports(dir).find((n) => n === name);
  if (!hit) return new Response("Not found", { status: 404, headers: extra });
  const type = hit.endsWith(".html") ? "text/html; charset=utf-8" : "application/json";
  return new Response(readFileSync(join(dir, "reports", hit)), {
    headers: { ...extra, ...REPORT_HEADERS, "Content-Type": type },
  });
}
