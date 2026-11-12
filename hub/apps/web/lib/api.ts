import { getDb } from "@porter-hub/db";
import { getAuth } from "./auth";
import { getListener, type Listener } from "./listen";
import type { Db } from "./permissions";
import * as svc from "./projects";
import { type Actor, HttpError, parse, schemas } from "./projects";
import * as runs from "./runs";
import * as sec from "./secrets";
import * as sh from "./shares";
import * as up from "./uploads";
import * as ver from "./versions";

type GetActor = (req: Request) => Promise<Actor | null>;
type P = Record<string, string>;

/** Route handlers, with the database and the session lookup injected so tests can drive them. */
export function makeApi(
  db: Db,
  getActor: GetActor,
  opts: { listener?: () => Listener; readToml?: runs.ReadToml; openErrors?: runs.OpenErrors } = {},
) {
  const fail = (e: unknown) => {
    if (e instanceof HttpError) return Response.json({ ...e.extra, error: e.message }, { status: e.status });
    console.error(e);
    return Response.json({ error: "Internal error" }, { status: 500 });
  };
  /** For handlers that build their own Response (the SSE stream). */
  const raw =
    (fn: (actor: Actor, req: Request, params: P) => Promise<Response>) => async (req: Request, params: P) => {
      try {
        const actor = await getActor(req);
        if (!actor) throw new HttpError(401, "Sign in required");
        return await fn(actor, req, params);
      } catch (e) {
        return fail(e);
      }
    };
  const route =
    <Ctx extends P | undefined = undefined>(
      fn: (actor: Actor, req: Request, params: Ctx) => Promise<unknown>,
      status = 200,
    ) =>
    async (req: Request, params: Ctx = undefined as Ctx) => {
      try {
        const actor = await getActor(req);
        if (!actor) throw new HttpError(401, "Sign in required");
        return Response.json(await fn(actor, req, params), { status });
      } catch (e) {
        return fail(e);
      }
    };
  const body = async (req: Request) => req.json().catch(() => undefined);
  const slugOf = (p: P) => p.slug;

  return {
    listProjects: route((a) => svc.listProjects(db, a)),
    createProject: route(
      async (a, r) => svc.createProject(db, a, parse(schemas.createProject, await body(r))),
      201,
    ),
    getProject: route<P>((a, _r, p) => svc.getProject(db, a, slugOf(p))),
    listMembers: route<P>((a, _r, p) => svc.listMembers(db, a, slugOf(p))),
    addMember: route<P>(async (a, r, p) => {
      await svc.access(db, a, slugOf(p), "manage_members");
      return svc.addMember(db, a, slugOf(p), parse(schemas.addMember, await body(r)));
    }, 201),
    updateMember: route<P>(async (a, r, p) => {
      await svc.access(db, a, slugOf(p), "manage_members");
      return svc.setRole(db, a, slugOf(p), p.userId, parse(schemas.setRole, await body(r)).role);
    }),
    removeMember: route<P>((a, _r, p) => svc.removeMember(db, a, slugOf(p), p.userId)),
    listInvitations: route<P>((a, _r, p) => svc.listInvitations(db, a, slugOf(p))),
    createInvitation: route<P>(async (a, r, p) => {
      await svc.access(db, a, slugOf(p), "manage_members");
      return svc.createInvitation(db, a, slugOf(p), parse(schemas.invite, await body(r)));
    }, 201),
    revokeInvitation: route<P>(async (a, _r, p) => {
      // Permission first (403/404), then the id shape, so a non-member learns nothing.
      await svc.access(db, a, slugOf(p), "manage_members");
      return svc.revokeInvitation(db, a, slugOf(p), parse(schemas.id, p.id));
    }),
    listShares: route<P>((a, _r, p) => sh.listShares(db, a, slugOf(p))),
    createShare: route<P>(async (a, r, p) => sh.createShare(db, a, slugOf(p), await body(r)), 201),
    revokeShare: route<P>((a, _r, p) => sh.revokeShare(db, a, slugOf(p), p.id)),
    listVersions: route<P>((a, _r, p) => ver.listVersions(db, a, slugOf(p))),
    getVersion: route<P>((a, _r, p) => ver.getVersion(db, a, slugOf(p), p.n)),
    initVersion: route<P>(async (a, r, p) => ver.initVersion(db, a, slugOf(p), await body(r))),
    commitVersion: route<P>(async (a, r, p) => ver.commitVersion(db, a, slugOf(p), await body(r)), 201),
    uploadMediaUrls: route<P>(async (a, r, p) =>
      ver.uploadMediaUrls(db, a, slugOf(p), p.upload_id, await body(r)),
    ),
    createUpload: route<P>(async (a, r, p) => up.createUpload(db, a, slugOf(p), await body(r)), 201),
    listUploads: route<P>((a, _r, p) => up.listUploads(db, a, slugOf(p))),
    getUpload: route<P>((a, _r, p) => up.getUpload(db, a, slugOf(p), p.id)),
    uploadPartUrls: route<P>(async (a, r, p) => up.partUrls(db, a, slugOf(p), p.id, await body(r))),
    completeUpload: route<P>(
      async (a, r, p) => up.completeUpload(db, a, slugOf(p), p.id, await body(r)),
      202,
    ),
    abortUpload: route<P>((a, _r, p) => up.abortUpload(db, a, slugOf(p), p.id)),
    downloadUpload: route<P>((a, _r, p) => up.downloadUpload(db, a, slugOf(p), p.id)),
    mediaUrls: route<P>(async (a, r, p) => ver.mediaUrls(db, a, slugOf(p), await body(r))),
    createRun: route<P>(
      async (a, r, p) => runs.createRun(db, a, slugOf(p), await body(r), opts.readToml, opts.openErrors),
      201,
    ),
    listRuns: route<P>((a, _r, p) => runs.listRuns(db, a, slugOf(p))),
    getRun: route<P>((a, _r, p) => runs.getRun(db, a, slugOf(p), p.id)),
    cancelRun: route<P>((a, _r, p) => runs.cancelRun(db, a, slugOf(p), p.id)),
    runEvents: raw((a, r, p) => runs.runEvents(db, (opts.listener ?? getListener)(), a, slugOf(p), p.id, r)),
    listSecrets: route<P>((a, _r, p) => sec.listSecrets(db, a, slugOf(p))),
    putSecret: route<P>(async (a, r, p) => sec.putSecret(db, a, slugOf(p), p.name, await body(r))),
    deleteSecret: route<P>((a, _r, p) => sec.deleteSecret(db, a, slugOf(p), p.name)),
  };
}

let instance: ReturnType<typeof makeApi> | undefined;
/** The app's API, bound to the real database and Better Auth session. */
export const getApi = () => {
  instance ??= makeApi(getDb(), async (req) => {
    const s = await getAuth().api.getSession({ headers: req.headers });
    return s ? { id: s.user.id } : null;
  });
  return instance;
};
