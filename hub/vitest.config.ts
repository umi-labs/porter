import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["packages/**/*.test.ts", "apps/**/*.test.ts"],
    testTimeout: 30000,
    // Dev MinIO from docker-compose.yml; its own bucket so tests never touch dev data.
    env: {
      S3_ENDPOINT: "http://localhost:59100",
      S3_BUCKET: "porter-hub-test",
      S3_ACCESS_KEY: "porter-dev",
      S3_SECRET_KEY: "porter-dev-secret",
    },
  },
});
