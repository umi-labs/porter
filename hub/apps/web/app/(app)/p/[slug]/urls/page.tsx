import { getDb } from "@porter-hub/db";
import { ProjectFrame } from "@/components/porter/app-frame";
import { PageTitle } from "@/components/porter/frame";
import { requireUser } from "@/lib/session";
import { pageNum, urlFilter } from "@/lib/snapshot-reader";
import { NoSnapshot, orStatus, UrlsView } from "@/lib/view-parts";
import { loadProject } from "@/lib/views";

export const dynamic = "force-dynamic";

export default async function Urls({
  params,
  searchParams,
}: {
  params: Promise<{ slug: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const { slug } = await params;
  const sp = await searchParams;
  const page = pageNum(sp.page);
  const user = await requireUser();
  const { project, role, snap } = await orStatus(loadProject(getDb(), user, slug, "view_raw"));
  return (
    <ProjectFrame project={project} role={role} user={user} active="URLs">
      <PageTitle title="URLs" sub="Every old URL and what it maps to on the new site." />
      {snap ? (
        <UrlsView snap={snap} page={page} filter={urlFilter(sp)} base={`/p/${slug}/urls`} />
      ) : (
        <NoSnapshot />
      )}
    </ProjectFrame>
  );
}
