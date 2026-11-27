import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { PageTitle } from "@/components/porter/frame";
import { issueFilter, pageNum } from "@/lib/snapshot-reader";
import { IssuesView, NoSnapshot } from "@/lib/view-parts";
import { loadShare } from "@/lib/views";

export const dynamic = "force-dynamic";

export default async function SharedIssues({
  params,
  searchParams,
}: {
  params: Promise<{ token: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const { token } = await params;
  const sp = await searchParams;
  const page = pageNum(sp.page);
  const v = await loadShare(getDb(), token, "issues");
  if (!v) notFound();
  return (
    <>
      <PageTitle
        title="Issues"
        sub={
          <a href={`/s/${token}`} className="text-primary hover:underline">
            Back to the overview
          </a>
        }
      />
      {v.snap ? (
        <IssuesView
          snap={v.snap}
          page={page}
          filter={issueFilter(sp)}
          base={`/s/${token}/issues`}
          full={false}
        />
      ) : (
        <NoSnapshot />
      )}
    </>
  );
}
