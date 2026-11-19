import { getApi } from "../../../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string; id: string }> };
export const POST = async (req: Request, { params }: Ctx) => getApi().cancelRun(req, await params);
