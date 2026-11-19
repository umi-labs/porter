import { fileURLToPath } from "node:url";

// Next 15 needs the TypeScript 6 JS API (apps/web devDependency); `pnpm -C hub typecheck` uses tsc 7.
/** @type {import("next").NextConfig} */
export default {
  output: "standalone",
  // The pnpm workspace root, so the standalone build traces packages/* and lands at .next/standalone/apps/web/server.js.
  outputFileTracingRoot: fileURLToPath(new URL("../..", import.meta.url)),
  // A long-running `next dev` can use its own build dir so a `next build` in the same checkout doesn't clobber it.
  distDir: process.env.NEXT_DIST_DIR || ".next",
  // Linting is biome (`pnpm -C hub lint`).
  eslint: { ignoreDuringBuilds: true },
  // forbidden() for the 403 on pages a role may not see.
  experimental: { authInterrupts: true },
  async headers() {
    // Every page and response under /s/ (the report route also sets its own CSP).
    const share = [
      { key: "Referrer-Policy", value: "no-referrer" },
      { key: "X-Robots-Tag", value: "noindex" },
      { key: "Cache-Control", value: "private, no-store" },
    ];
    return [{ source: "/s/:path*", headers: share }];
  },
};
