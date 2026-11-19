import { getApi } from "../../../../../lib/api";

export const GET = (req: Request, { params }: { params: Promise<{ slug: string }> }) =>
  params.then((p) => getApi().listShares(req, p));
export const POST = (req: Request, { params }: { params: Promise<{ slug: string }> }) =>
  params.then((p) => getApi().createShare(req, p));
