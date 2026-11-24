import { readFileSync } from "node:fs";
import { join } from "node:path";
import { getDb } from "@porter-hub/db";
import { destinationHost } from "@porter-hub/porter/config";
import Link from "next/link";
import { Chip } from "@/components/porter/chip";
import { PageTitle } from "@/components/porter/frame";
import { RunForm } from "@/components/run-form";
import { TableCell, TableRow } from "@/components/ui/table";
import { roleCan } from "@/lib/permissions";
import { runLook } from "@/lib/run-look";
import { listRuns } from "@/lib/runs";
import { requireUser } from "@/lib/session";
import { DataTable, NoSnapshot, Overview, orStatus, ProjectFrame, Section } from "@/lib/view-parts";
import { loadProject } from "@/lib/views";

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
                <span className="font-mono text-xs">{host}</span>
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
        <Section
          title="Runs on the hub"
          aside="Each run uses the latest pushed version and pushes its result."
        >
          {snap && roleCan(role, "run") && <RunForm slug={slug} host={host} />}
          {runs.length ? (
            <DataTable head={["Started", "Stages", "Version", "State"]}>
              {runs.map((r) => {
                const look = runLook(r);
                return (
                  <TableRow key={r.id}>
                    <TableCell>
                      <Link
                        href={`/p/${slug}/runs/${r.id}`}
                        className="inline-flex min-h-11 items-center font-medium text-primary hover:underline"
                      >
                        {r.created_at.toISOString().slice(0, 16).replace("T", " ")}
                      </Link>
                    </TableCell>
                    <TableCell>
                      {r.stages === "all" ? "all" : r.stages.join(", ")}
                      {r.apply ? " (apply)" : ""}
                    </TableCell>
                    <TableCell>
                      v{r.base_version}
                      {r.result_version != null ? ` → v${r.result_version}` : ""}
                    </TableCell>
                    <TableCell>
                      {look.state ? <Chip state={look.state} label={look.label} /> : look.label}
                      {r.exit_code != null && (
                        <span className="ml-2 text-xs text-muted-foreground">exit {r.exit_code}</span>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </DataTable>
          ) : (
            <p className="m-0 text-muted-foreground">No runs yet.</p>
          )}
        </Section>
      )}
    </ProjectFrame>
  );
}
