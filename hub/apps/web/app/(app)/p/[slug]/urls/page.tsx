import { getDb } from "@porter-hub/db";
import { requireUser } from "../../../../../lib/session";
import { pageNum } from "../../../../../lib/snapshot-reader";
import { NoSnapshot, orStatus, Shell, UrlsView } from "../../../../../lib/view-parts";
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
  const { snap } = await orStatus(loadProject(getDb(), await requireUser(), slug, "view_raw"));
  return (
    <Shell title={`${slug}: URLs`}>
      {snap ? <UrlsView snap={snap} page={page} base={`/p/${slug}/urls`} /> : <NoSnapshot />}
    </Shell>
  );
}
