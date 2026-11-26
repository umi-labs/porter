/**
 * `pnpm -C hub admin:service-key "<name>"`: mints an API key for another service (e.g. "Foundry hub") and prints it
 * once. The key acts as its own staff bot user. In the worker container:
 * `docker compose exec worker node_modules/.bin/tsx apps/worker/src/service-key.ts "Foundry hub"`.
 */
import { connect } from "@porter-hub/db";
import { mintServiceKey } from "@porter-hub/db/service-keys";

if (process.argv[1]?.endsWith("service-key.ts")) {
  const name = (process.argv[2] ?? "").trim();
  if (!name) {
    console.error('usage: service-key "<name>"');
    process.exit(2);
  }
  const { db, client } = connect();
  mintServiceKey(db, name)
    .then(({ key }) => {
      console.log(`service key for ${name} (shown once; Porter keeps only its hash):`);
      console.log(key);
      console.log("Use it as: Authorization: Bearer <key>");
    })
    .catch((e) => {
      console.error(e instanceof Error ? e.message : e);
      process.exitCode = 1;
    })
    .finally(() => client.end());
}
