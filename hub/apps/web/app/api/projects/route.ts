import { getApi } from "../../../lib/api";

export const GET = (req: Request) => getApi().listProjects(req);
export const POST = (req: Request) => getApi().createProject(req);
