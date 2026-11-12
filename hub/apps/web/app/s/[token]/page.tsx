import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { shareAllows } from "../../../lib/shares";
import { listReports } from "../../../lib/snapshot-reader";
import { NoSnapshot, Overview, Shell } from "../../../lib/view-parts";
import { loadShare } from "../../../lib/views";

export const dynamic = "force-dynamic";

export default async function Shared({ params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  const v = await loadShare(getDb(), token, "index");
  if (!v) notFound();
  const base = `/s/${token}`;
  return (
    <Shell title={v.project.name}>
      {!v.snap ? (
        <NoSnapshot />
      ) : !shareAllows(v.scope, "overview") ? (
        <>
          <h2>Reports</h2>
          <ul>
            {listReports(v.snap.dir).map((r) => (
              <li key={r}>
                <a href={`${base}/reports/${encodeURIComponent(r)}`}>{r}</a>
              </li>
            ))}
          </ul>
        </>
      ) : (
        <>
          <p>
            <a href={`${base}/issues`}>Issues</a> | <a href={`${base}/urls`}>URLs</a>
          </p>
          <Overview snap={v.snap} reportBase={`${base}/reports`} />
        </>
      )}
    </Shell>
  );
}
