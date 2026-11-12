/**
 * Server start, before the first request. Production: checks HUB_SECRET_KEY, applies pending migrations
 * (advisory-locked, so replicas take turns) and builds auth, so a server missing BETTER_AUTH_SECRET/URL refuses to boot.
 */
export async function register() {
  // The imports must sit inside this check (not after an early return) so the edge bundle drops them.
  if (process.env.NEXT_RUNTIME === "nodejs") {
    try {
      if (process.env.NODE_ENV === "production") {
        (await import("@porter-hub/porter/secrets")).secretKey();
        await (await import("@porter-hub/db/migrate")).runMigrations();
      }
      (await import("./lib/auth")).getAuth();
    } catch (e) {
      console.error(e instanceof Error ? e.message : e);
      process.exit(1);
    }
  }
}
