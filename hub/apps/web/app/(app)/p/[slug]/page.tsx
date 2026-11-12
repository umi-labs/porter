import { readFileSync } from "node:fs";
import { join } from "node:path";
import { getDb } from "@porter-hub/db";
import { destinationHost } from "@porter-hub/porter/config";
import Link from "next/link";
import { RunForm } from "../../../../components/run-form";
import { roleCan } from "../../../../lib/permissions";
import { listRuns } from "../../../../lib/runs";
import { requireUser } from "../../../../lib/session";
import { NoSnapshot, Overview, orStatus, Shell } from "../../../../lib/view-parts";
import { loadProject } from "../../../../lib/views";

export const dynamic = "force-dynamic";

export default async function ProjectPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const user = await requireUser();
  const { project, role, snap } = await orStatus(loadProject(getDb(), user, slug, "view"));
  const runs = roleCan(role, "view_raw") ? await listRuns(getDb(), user, slug, 20) : null;
  let host: string | null = null;
  try {
    if (snap) host = destinationHost(readFileSync(join(snap.dir, "porter.toml"), "utf8"));
  } catch {} // unreadable porter.toml: the API refuses apply anyway
  return (
    <Shell title={project.name}>
      <p>
        {roleCan(role, "view_raw") && (
          <>
            <Link href={`/p/${slug}/issues`}>Issues</Link> | <Link href={`/p/${slug}/urls`}>URLs</Link> |{" "}
          </>
        )}
        {roleCan(role, "upload") && (
          <>
            <Link href={`/p/${slug}/uploads`}>Uploads</Link> |{" "}
          </>
        )}
        {roleCan(role, "share") && <Link href={`/p/${slug}/settings`}>Settings</Link>}
      </p>
      {snap ? <Overview snap={snap} reportBase={`/p/${slug}/reports`} /> : <NoSnapshot />}
      {runs && (
        <>
          <h2>Remote runs</h2>
          {snap && roleCan(role, "run") && <RunForm slug={slug} host={host} />}
          {runs.length ? (
            <ul>
              {runs.map((r) => (
                <li key={r.id}>
                  <Link href={`/p/${slug}/runs/${r.id}`}>
                    {r.created_at.toISOString().slice(0, 16).replace("T", " ")}
                  </Link>{" "}
                  {r.stages === "all" ? "all" : r.stages.join(", ")}
                  {r.apply ? " (apply)" : ""} on v{r.base_version}: {r.state}
                  {r.exit_code != null ? ` (exit ${r.exit_code})` : ""}
                  {r.result_version != null ? ` → v${r.result_version}` : ""}
                </li>
              ))}
            </ul>
          ) : (
            <p>No runs yet.</p>
          )}
        </>
      )}
    </Shell>
  );
}
