import { getApi } from "../../../../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string; upload_id: string }> };
export const POST = async (req: Request, { params }: Ctx) => getApi().uploadMediaUrls(req, await params);
