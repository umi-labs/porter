import { getApi } from "../../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string }> };
export const POST = async (req: Request, { params }: Ctx) => getApi().initVersion(req, await params);
