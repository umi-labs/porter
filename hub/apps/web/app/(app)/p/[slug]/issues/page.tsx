import { getDb } from "@porter-hub/db";
import { requireUser } from "../../../../../lib/session";
import { pageNum } from "../../../../../lib/snapshot-reader";
import { IssuesView, NoSnapshot, orStatus, Shell } from "../../../../../lib/view-parts";
import { loadProject } from "../../../../../lib/views";

export const dynamic = "force-dynamic";

export default async function Issues({
  params,
  searchParams,
}: {
  params: Promise<{ slug: string }>;
  searchParams: Promise<{ page?: string }>;
}) {
  const { slug } = await params;
  const page = pageNum((await searchParams).page);
  const { snap } = await orStatus(loadProject(getDb(), await requireUser(), slug, "view_raw"));
  return (
    <Shell title={`${slug}: issues`}>
      {snap ? <IssuesView snap={snap} page={page} base={`/p/${slug}/issues`} full /> : <NoSnapshot />}
    </Shell>
  );
}
