import { getDb } from "@porter-hub/db";
import Link from "next/link";
import { roleCan } from "../../../../lib/permissions";
import { requireUser } from "../../../../lib/session";
import { NoSnapshot, Overview, orStatus, Shell } from "../../../../lib/view-parts";
import { loadProject } from "../../../../lib/views";

export const dynamic = "force-dynamic";

export default async function ProjectPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const { project, role, snap } = await orStatus(loadProject(getDb(), await requireUser(), slug, "view"));
  return (
    <Shell title={project.name}>
      <p>
        {roleCan(role, "view_raw") && (
          <>
            <Link href={`/p/${slug}/issues`}>Issues</Link> | <Link href={`/p/${slug}/urls`}>URLs</Link> |{" "}
          </>
        )}
        {roleCan(role, "upload") && (
          <>
            <Link href={`/p/${slug}/uploads`}>Uploads</Link> |{" "}
          </>
        )}
        {roleCan(role, "share") && <Link href={`/p/${slug}/settings`}>Settings</Link>}
      </p>
      {snap ? <Overview snap={snap} reportBase={`/p/${slug}/reports`} /> : <NoSnapshot />}
    </Shell>
  );
}
