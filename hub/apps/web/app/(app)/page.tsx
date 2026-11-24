import { getDb } from "@porter-hub/db";
import Link from "next/link";
import { Chip } from "../../components/ui/chip";
import { Frame, PageTitle } from "../../components/ui/frame";
import { fmtAgo } from "../../lib/format";
import { projectHeadline } from "../../lib/headline";
import { isStaff, listProjects } from "../../lib/projects";
import { requireUser } from "../../lib/session";
import { NewProjectForm } from "./new-project-form";

export const dynamic = "force-dynamic";

// Layout and copy: docs/design/canvas/Hub-States.dc.html, "PROJECTS · EMPTY" and "PROJECTS · LIST WITH LONG NAMES".
export default async function Home() {
  const user = await requireUser();
  const [projects, staff] = await Promise.all([listProjects(getDb(), user), isStaff(getDb(), user)]);
  return (
    <Frame who={user.name || user.email} narrow>
      <PageTitle title="Projects" />
      {projects.length === 0 ? (
        <div className="flex min-h-[260px] flex-col items-center justify-center gap-2.5 rounded-lg border border-border bg-bg p-5 text-center">
          <span className="text-[17px] font-semibold">
            {staff ? "No projects yet" : "Nothing has been shared with you yet."}
          </span>
          {staff ? (
            <>
              <span className="max-w-[300px] text-ink">
                Projects appear here when someone pushes one from porter on their computer.
              </span>
              <code className="rounded-sm bg-surface px-2.5 py-1.5 font-mono text-[13px]">
                porter hub login, then porter hub push from the project
              </code>
            </>
          ) : (
            <span className="max-w-[300px] text-ink">
              Projects appear here when your Umi team invites you to one.
            </span>
          )}
        </div>
      ) : (
        <ul className="m-0 flex list-none flex-col overflow-hidden rounded-lg border border-border bg-bg p-0">
          {projects.map((p) => {
            const pill = projectHeadline(p.latest?.status ?? null);
            return (
              <li key={p.id} className="border-t border-panel first:border-t-0">
                <Link
                  href={`/p/${p.slug}`}
                  className="flex min-h-11 items-center gap-2.5 px-4 py-3 text-ink no-underline hover:bg-hover hover:no-underline"
                >
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="truncate font-semibold" title={p.name}>
                      {p.name}
                    </span>
                    <span className="text-small text-muted">
                      {p.latest
                        ? `v${p.latest.number} · ${fmtAgo(p.latest.createdAt)}`
                        : "Nothing pushed yet"}
                    </span>
                  </span>
                  {pill && <Chip state={pill.state} label={pill.label} />}
                </Link>
              </li>
            );
          })}
        </ul>
      )}
      {staff && <NewProjectForm />}
    </Frame>
  );
}
