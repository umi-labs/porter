import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { PageTitle } from "@/components/porter/frame";
import { pageNum, urlFilter } from "@/lib/snapshot-reader";
import { NoSnapshot, UrlsView } from "@/lib/view-parts";
import { loadShare } from "@/lib/views";

export const dynamic = "force-dynamic";

export default async function SharedUrls({
  params,
  searchParams,
}: {
  params: Promise<{ token: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const { token } = await params;
  const sp = await searchParams;
  const page = pageNum(sp.page);
  const v = await loadShare(getDb(), token, "urls");
  if (!v) notFound();
  return (
    <>
      <PageTitle
        title="URLs"
        sub={
          <a href={`/s/${token}`} className="text-primary hover:underline">
            Back to the overview
          </a>
        }
      />
      {v.snap ? (
        <UrlsView snap={v.snap} page={page} filter={urlFilter(sp)} base={`/s/${token}/urls`} />
      ) : (
        <NoSnapshot />
      )}
    </>
  );
}
