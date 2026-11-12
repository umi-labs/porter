import { getApi } from "../../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string; n: string }> };
export const GET = async (req: Request, { params }: Ctx) => getApi().getVersion(req, await params);
