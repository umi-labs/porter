/**
 * Server start, before the first request. Production: checks HUB_SECRET_KEY and the S3 settings, applies pending migrations
 * (advisory-locked, so replicas take turns) and builds auth, so a server missing BETTER_AUTH_SECRET/URL refuses to boot.
 */
export async function register() {
  // The imports must sit inside this check (not after an early return) so the edge bundle drops them.
  if (process.env.NEXT_RUNTIME === "nodejs") {
    try {
      if (process.env.NODE_ENV === "production") {
        (await import("@porter-hub/porter/secrets")).secretKey();
        // Conditionally required in compose (MinIO or R2/S3), so checked here rather than left to fail on first push.
        for (const k of ["S3_ENDPOINT", "S3_ACCESS_KEY", "S3_SECRET_KEY", "S3_BUCKET"])
          if (!process.env[k]) throw new Error(`${k} must be set in production`);
        await (await import("@porter-hub/db/migrate")).runMigrations();
      }
      (await import("./lib/auth")).getAuth();
    } catch (e) {
      console.error(e instanceof Error ? e.message : e);
      process.exit(1);
    }
  }
}
