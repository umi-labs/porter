import { getApi } from "../../../../lib/api";

type Ctx = { params: Promise<{ slug: string }> };
export const GET = async (req: Request, { params }: Ctx) => getApi().getProject(req, await params);
