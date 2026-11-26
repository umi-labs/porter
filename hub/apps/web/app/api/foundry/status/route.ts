import { getApi } from "@/lib/api";

/** Foundry hub: the migration status of the project linked to `?ref=<org>/<project>`. */
export const GET = (req: Request) => getApi().foundryStatus(req);
