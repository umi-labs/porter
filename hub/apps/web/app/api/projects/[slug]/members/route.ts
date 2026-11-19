import { getApi } from "../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string }> };
export const GET = async (req: Request, { params }: Ctx) => getApi().listMembers(req, await params);
export const POST = async (req: Request, { params }: Ctx) => getApi().addMember(req, await params);
