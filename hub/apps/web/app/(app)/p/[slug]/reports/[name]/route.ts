import { getDb } from "@porter-hub/db";
import { getAuth } from "../../../../../../lib/auth";
import { HttpError } from "../../../../../../lib/projects";
import { serveReport } from "../../../../../../lib/report";
import { loadProject } from "../../../../../../lib/views";

export const dynamic = "force-dynamic";

export async function GET(req: Request, { params }: { params: Promise<{ slug: string; name: string }> }) {
  const { slug, name } = await params;
  const s = await getAuth().api.getSession({ headers: req.headers });
  if (!s) return new Response("Sign in required", { status: 401 });
  try {
    const { snap } = await loadProject(getDb(), { id: s.user.id }, slug, "view");
    return snap ? serveReport(snap.dir, name) : new Response("Not found", { status: 404 });
  } catch (e) {
    if (e instanceof HttpError) return new Response(e.message, { status: e.status });
    throw e;
  }
}
