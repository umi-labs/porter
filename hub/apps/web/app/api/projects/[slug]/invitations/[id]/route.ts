import { getApi } from "../../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string; id: string }> };
export const DELETE = async (req: Request, { params }: Ctx) => getApi().revokeInvitation(req, await params);
