import { getDb } from "@porter-hub/db";
import { sql } from "drizzle-orm";

export const dynamic = "force-dynamic";

/** Liveness for the container health check: no auth, says only whether the database answers. */
export async function GET() {
  try {
    await getDb().execute(sql`select 1`);
    return Response.json({ ok: true }, { headers: { "Cache-Control": "no-store" } });
  } catch {
    return Response.json({ ok: false }, { status: 503, headers: { "Cache-Control": "no-store" } });
  }
}
