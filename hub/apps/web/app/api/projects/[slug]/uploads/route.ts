import { getApi } from "../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string }> };
export const GET = async (req: Request, { params }: Ctx) => getApi().listUploads(req, await params);
export const POST = async (req: Request, { params }: Ctx) => getApi().createUpload(req, await params);
