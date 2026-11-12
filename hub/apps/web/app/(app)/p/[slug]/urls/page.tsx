import { getDb } from "@porter-hub/db";
import { PageTitle } from "../../../../../components/ui/frame";
import { requireUser } from "../../../../../lib/session";
import { pageNum } from "../../../../../lib/snapshot-reader";
import { NoSnapshot, orStatus, ProjectFrame, UrlsView } from "../../../../../lib/view-parts";
import { loadProject } from "../../../../../lib/views";

export const dynamic = "force-dynamic";

export default async function Urls({
  params,
  searchParams,
}: {
  params: Promise<{ slug: string }>;
  searchParams: Promise<{ page?: string }>;
}) {
  const { slug } = await params;
  const page = pageNum((await searchParams).page);
  const user = await requireUser();
  const { project, role, snap } = await orStatus(loadProject(getDb(), user, slug, "view_raw"));
  return (
    <ProjectFrame project={project} role={role} user={user} active="URLs">
      <PageTitle title="URLs" sub="Every old URL and what it maps to on the new site." />
      {snap ? <UrlsView snap={snap} page={page} base={`/p/${slug}/urls`} /> : <NoSnapshot />}
    </ProjectFrame>
  );
}
