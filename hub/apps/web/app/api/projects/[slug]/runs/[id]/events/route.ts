import { getApi } from "../../../../../../../lib/api";

export const dynamic = "force-dynamic";
type Ctx = { params: Promise<{ slug: string; id: string }> };
/** Server-sent events (text/event-stream); see lib/runs.ts runEvents. */
export const GET = async (req: Request, { params }: Ctx) => getApi().runEvents(req, await params);
