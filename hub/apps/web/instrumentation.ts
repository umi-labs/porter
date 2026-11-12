/** Builds auth at server start so a production server without BETTER_AUTH_SECRET/URL refuses to boot. */
export async function register() {
  // The import must sit inside this check (not after an early return) so the edge bundle drops it.
  if (process.env.NEXT_RUNTIME === "nodejs") {
    try {
      (await import("./lib/auth")).getAuth();
    } catch (e) {
      console.error(e instanceof Error ? e.message : e);
      process.exit(1);
    }
  }
}
