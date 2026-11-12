import { getApi } from "../../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string; userId: string }> };
export const PATCH = async (req: Request, { params }: Ctx) => getApi().updateMember(req, await params);
export const DELETE = async (req: Request, { params }: Ctx) => getApi().removeMember(req, await params);
