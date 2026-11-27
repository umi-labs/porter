import { getDb } from "@porter-hub/db";
import { ProjectFrame } from "@/components/porter/app-frame";
import { PageTitle } from "@/components/porter/frame";
import { requireUser } from "@/lib/session";
import { issueFilter, pageNum } from "@/lib/snapshot-reader";
import { IssuesView, NoSnapshot, orStatus } from "@/lib/view-parts";
import { loadProject } from "@/lib/views";

export const dynamic = "force-dynamic";

export default async function Issues({
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
    <ProjectFrame project={project} role={role} user={user} active="Issues">
      <PageTitle
        title="Issues"
        sub="Filter by kind, severity or message. 100 per page; nothing is hidden, only paged."
      />
      {snap ? (
        <IssuesView snap={snap} page={page} filter={issueFilter(sp)} base={`/p/${slug}/issues`} full />
      ) : (
        <NoSnapshot />
      )}
    </ProjectFrame>
  );
}
