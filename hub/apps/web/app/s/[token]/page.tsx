import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { Card } from "../../../components/ui/card";
import { PageTitle } from "../../../components/ui/frame";
import { shareAllows } from "../../../lib/shares";
import { listReports } from "../../../lib/snapshot-reader";
import { Empty, NoSnapshot, Overview } from "../../../lib/view-parts";
import { loadShare } from "../../../lib/views";

export const dynamic = "force-dynamic";

export default async function Shared({ params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  const v = await loadShare(getDb(), token, "index");
  if (!v) notFound();
  const base = `/s/${token}`;
  const full = shareAllows(v.scope, "overview");
  return (
    <>
      <PageTitle
        title={v.project.name}
        sub={
          full
            ? "Read-only project: overview, issues, URLs and reports."
            : "Reports only. No sign-in, no editing."
        }
      />
      {!v.snap ? (
        <NoSnapshot />
      ) : !full ? (
        <Card title={`Reports from v${v.snap.version.number}`}>
          {listReports(v.snap.dir).length === 0 ? (
            <Empty title="No reports in this version" />
          ) : (
            <ul className="m-0 flex list-none flex-col p-0">
              {listReports(v.snap.dir).map((r) => (
                <li key={r} className="border-t border-panel first:border-t-0">
                  <a href={`${base}/reports/${encodeURIComponent(r)}`} className="flex min-h-11 items-center">
                    {r}
                  </a>
                </li>
              ))}
            </ul>
          )}
        </Card>
      ) : (
        <>
          <nav aria-label="Shared project" className="flex gap-4 text-[14px]">
            <a href={`${base}/issues`} className="flex min-h-11 items-center">
              Issues
            </a>
            <a href={`${base}/urls`} className="flex min-h-11 items-center">
              URLs
            </a>
          </nav>
          <Overview
            snap={v.snap}
            reportBase={`${base}/reports`}
            audience="share"
            issuesHref={`${base}/issues`}
          />
        </>
      )}
    </>
  );
}
