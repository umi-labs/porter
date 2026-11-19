import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { PageTitle } from "../../../../components/ui/frame";
import { pageNum } from "../../../../lib/snapshot-reader";
import { IssuesView, NoSnapshot } from "../../../../lib/view-parts";
import { loadShare } from "../../../../lib/views";

export const dynamic = "force-dynamic";

export default async function SharedIssues({
  params,
  searchParams,
}: {
  params: Promise<{ token: string }>;
  searchParams: Promise<{ page?: string }>;
}) {
  const { token } = await params;
  const page = pageNum((await searchParams).page);
  const v = await loadShare(getDb(), token, "issues");
  if (!v) notFound();
  return (
    <>
      <PageTitle title="Issues" sub={<a href={`/s/${token}`}>Back to the overview</a>} />
      {v.snap ? (
        <IssuesView snap={v.snap} page={page} base={`/s/${token}/issues`} full={false} />
      ) : (
        <NoSnapshot />
      )}
    </>
  );
}
