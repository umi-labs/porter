import { readFileSync } from "node:fs";
import { join } from "node:path";
import { getDb } from "@porter-hub/db";
import { destinationHost } from "@porter-hub/porter/config";
import Link from "next/link";
import { RunForm } from "../../../../components/run-form";
import { Card } from "../../../../components/ui/card";
import { Chip } from "../../../../components/ui/chip";
import { PageTitle } from "../../../../components/ui/frame";
import { Table } from "../../../../components/ui/table";
import { roleCan } from "../../../../lib/permissions";
import { runLook } from "../../../../lib/run-look";
import { listRuns } from "../../../../lib/runs";
import { requireUser } from "../../../../lib/session";
import { NoSnapshot, Overview, orStatus, ProjectFrame } from "../../../../lib/view-parts";
import { loadProject } from "../../../../lib/views";

export const dynamic = "force-dynamic";

export default async function ProjectPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const user = await requireUser();
  const { project, role, snap } = await orStatus(loadProject(getDb(), user, slug, "view"));
  const runs = roleCan(role, "view_raw") ? await listRuns(getDb(), user, slug, 20) : null;
  const client = !roleCan(role, "view_raw");
  let host: string | null = null;
  try {
    if (snap) host = destinationHost(readFileSync(join(snap.dir, "porter.toml"), "utf8"));
  } catch {} // unreadable porter.toml: the API refuses apply anyway
  return (
    <ProjectFrame project={project} role={role} user={user} active="Overview">
      <PageTitle
        title={project.name}
        sub={
          <>
            {project.summary}
            {!client && host && (
              <>
                {project.summary ? " · " : ""}
                <span className="font-mono text-small">{host}</span>
              </>
            )}
          </>
        }
      />
      {snap ? (
        <Overview
          snap={snap}
          reportBase={`/p/${slug}/reports`}
          audience={client ? "client" : "staff"}
          issuesHref={`/p/${slug}/issues`}
        />
      ) : (
        <NoSnapshot plain={client} />
      )}
      {runs && (
        <Card title="Runs on the hub" aside="Each run uses the latest pushed version and pushes its result.">
          {snap && roleCan(role, "run") && <RunForm slug={slug} host={host} />}
          {runs.length ? (
            <Table head={["Started", "Stages", "Version", "State"]}>
              {runs.map((r) => {
                const look = runLook(r);
                return (
                  <tr key={r.id}>
                    <td className="whitespace-nowrap">
                      <Link href={`/p/${slug}/runs/${r.id}`} className="inline-flex min-h-11 items-center">
                        {r.created_at.toISOString().slice(0, 16).replace("T", " ")}
                      </Link>
                    </td>
                    <td>
                      {r.stages === "all" ? "all" : r.stages.join(", ")}
                      {r.apply ? " (apply)" : ""}
                    </td>
                    <td className="whitespace-nowrap">
                      v{r.base_version}
                      {r.result_version != null ? ` → v${r.result_version}` : ""}
                    </td>
                    <td>
                      {look.state ? <Chip state={look.state} label={look.label} /> : look.label}
                      {r.exit_code != null && (
                        <span className="ml-2 text-small text-muted">exit {r.exit_code}</span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </Table>
          ) : (
            <p className="m-0 text-muted">No runs yet.</p>
          )}
        </Card>
      )}
    </ProjectFrame>
  );
}
