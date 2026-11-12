import { defineConfig } from "drizzle-kit";

export default defineConfig({
  dialect: "postgresql",
  schema: "./schema.ts",
  out: "./migrations",
  dbCredentials: { url: process.env.DATABASE_URL ?? "postgres://porter:porter@localhost:55439/porter" },
});
