import { getDb } from "@porter-hub/db";
import { SHARE_HEADERS, serveReport } from "../../../../../lib/report";
import { loadShare } from "../../../../../lib/views";

export const dynamic = "force-dynamic";

export async function GET(_req: Request, { params }: { params: Promise<{ token: string; name: string }> }) {
  const { token, name } = await params;
  const v = await loadShare(getDb(), token, "report");
  // Same 404 for unknown, expired, revoked and a missing snapshot or report.
  if (!v?.snap) return new Response("Not found", { status: 404, headers: SHARE_HEADERS });
  return serveReport(v.snap.dir, name, SHARE_HEADERS);
}
