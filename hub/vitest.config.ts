import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  // apps/web's tsconfig keeps JSX for Next ("preserve"); component tests need it compiled.
  oxc: { jsx: { runtime: "automatic" } },
  // apps/web imports its own files as @/… (shadcn's alias, tsconfig paths).
  resolve: { alias: { "@/": fileURLToPath(new URL("./apps/web/", import.meta.url)) } },
  test: {
    include: ["packages/**/*.test.ts", "apps/**/*.test.ts"],
    testTimeout: 30000,
    // Hooks create and drop a database each; under the full suite, Postgres serialises those and 10 s is too tight.
    hookTimeout: 60000,
    // Dev MinIO from docker-compose.dev.yml; its own bucket so tests never touch dev data.
    env: {
      S3_ENDPOINT: "http://localhost:59100",
      S3_BUCKET: "porter-hub-test",
      S3_ACCESS_KEY: "porter-dev",
      S3_SECRET_KEY: "porter-dev-secret",
    },
  },
});
