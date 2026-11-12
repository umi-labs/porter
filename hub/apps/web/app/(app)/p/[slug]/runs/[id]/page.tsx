import { getDb } from "@porter-hub/db";
import Link from "next/link";
import { RunLive } from "../../../../../../components/run-live";
import { roleCan } from "../../../../../../lib/permissions";
import { access } from "../../../../../../lib/projects";
import { getRun } from "../../../../../../lib/runs";
import { requireUser } from "../../../../../../lib/session";
import { orStatus, Shell } from "../../../../../../lib/view-parts";

export const dynamic = "force-dynamic";

/** Live log of one remote run (view_raw: clients never see run logs). */
export default async function RunPage({ params }: { params: Promise<{ slug: string; id: string }> }) {
  const { slug, id } = await params;
  const user = await requireUser();
  const db = getDb();
  const run = await orStatus(getRun(db, user, slug, id));
  const { role } = await access(db, user, slug, "view_raw");
  return (
    <Shell title={`${slug}: run ${run.id.slice(0, 8)}`}>
      <p>
        <Link href={`/p/${slug}`}>Back to the project</Link>
      </p>
      <RunLive slug={slug} initial={run} canCancel={roleCan(role, "run")} />
    </Shell>
  );
}
