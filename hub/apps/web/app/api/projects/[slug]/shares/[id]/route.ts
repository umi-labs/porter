import { getApi } from "../../../../../../lib/api";

export const DELETE = (req: Request, { params }: { params: Promise<{ slug: string; id: string }> }) =>
  params.then((p) => getApi().revokeShare(req, p));
