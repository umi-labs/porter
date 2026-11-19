import { getApi } from "../../../../../../lib/api";

type Ctx = { params: Promise<{ slug: string; name: string }> };
export const PUT = async (req: Request, { params }: Ctx) => getApi().putSecret(req, await params);
export const DELETE = async (req: Request, { params }: Ctx) => getApi().deleteSecret(req, await params);
