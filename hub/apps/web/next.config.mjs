// Next 15 needs the TypeScript 6 JS API (apps/web devDependency); `pnpm -C hub typecheck` uses tsc 7.
/** @type {import("next").NextConfig} */
export default {
  output: "standalone",
  // Linting is biome (`pnpm -C hub lint`).
  eslint: { ignoreDuringBuilds: true },
};
