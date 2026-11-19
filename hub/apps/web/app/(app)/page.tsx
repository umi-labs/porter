import { getDb } from "@porter-hub/db";
import Link from "next/link";
import { Frame, PageTitle } from "../../components/ui/frame";
import { isStaff, listProjects } from "../../lib/projects";
import { requireUser } from "../../lib/session";
import { Empty } from "../../lib/view-parts";
import { NewProjectForm } from "./new-project-form";

export const dynamic = "force-dynamic";

export default async function Home() {
  const user = await requireUser();
  const [projects, staff] = await Promise.all([listProjects(getDb(), user), isStaff(getDb(), user)]);
  return (
    <Frame who={user.name || user.email} narrow>
      <PageTitle title="Projects" />
      {projects.length === 0 ? (
        <Empty title="No projects yet">
          {staff
            ? "Create one below, then link a porter project to it with porter hub link <slug>."
            : "Projects appear here when you’re invited to one."}
        </Empty>
      ) : (
        <ul className="m-0 flex list-none flex-col overflow-hidden rounded-lg border border-border bg-bg p-0">
          {projects.map((p) => (
            <li
              key={p.id}
              className="flex flex-wrap items-center gap-3 border-t border-panel first:border-t-0"
            >
              <Link
                href={`/p/${p.slug}`}
                className="flex min-h-14 min-w-0 flex-1 flex-col justify-center px-5 py-2.5 text-ink no-underline hover:bg-hover hover:no-underline"
              >
                <span className="font-semibold break-words">{p.name}</span>
                <span className="text-small text-muted">
                  {p.role}
                  {p.summary ? ` · ${p.summary}` : ""}
                </span>
              </Link>
              {p.role === "owner" && (
                <Link href={`/p/${p.slug}/settings`} className="flex min-h-11 items-center px-5 text-[14px]">
                  Settings
                </Link>
              )}
            </li>
          ))}
        </ul>
      )}
      {staff && <NewProjectForm />}
    </Frame>
  );
}
