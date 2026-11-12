import { getApi } from "../../../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string; id: string }> };
export const GET = async (req: Request, { params }: Ctx) => getApi().downloadUpload(req, await params);
