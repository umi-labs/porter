import { getDb } from "@porter-hub/db";
import { RunLive } from "../../../../../../components/run-live";
import { roleCan } from "../../../../../../lib/permissions";
import { access } from "../../../../../../lib/projects";
import { getRun } from "../../../../../../lib/runs";
import { requireUser } from "../../../../../../lib/session";
import { orStatus, ProjectFrame } from "../../../../../../lib/view-parts";

export const dynamic = "force-dynamic";

/** Live log of one remote run (view_raw: clients never see run logs). */
export default async function RunPage({ params }: { params: Promise<{ slug: string; id: string }> }) {
  const { slug, id } = await params;
  const user = await requireUser();
  const db = getDb();
  const run = await orStatus(getRun(db, user, slug, id));
  const { project, role } = await access(db, user, slug, "view_raw");
  return (
    <ProjectFrame project={project} role={role} user={user} active="Overview">
      <RunLive slug={slug} initial={run} canCancel={roleCan(role, "run")} />
    </ProjectFrame>
  );
}
