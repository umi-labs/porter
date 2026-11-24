import { getDb } from "@porter-hub/db";
import { ChevronRightIcon, FolderIcon } from "lucide-react";
import Link from "next/link";
import { Chip } from "@/components/porter/chip";
import { Frame, PageTitle } from "@/components/porter/frame";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Kbd } from "@/components/ui/kbd";
import { fmtAgo } from "@/lib/format";
import { projectHeadline } from "@/lib/headline";
import { isStaff, listProjects } from "@/lib/projects";
import { requireUser } from "@/lib/session";
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
        <Empty className="animate-enter min-h-[260px] border bg-card">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <FolderIcon />
            </EmptyMedia>
            <EmptyTitle className="text-base">
              {staff ? "No projects yet" : "Nothing has been shared with you yet."}
            </EmptyTitle>
            <EmptyDescription className="text-sm">
              {staff
                ? "Projects appear here when someone pushes one from porter on their computer."
                : "Projects appear here when your Umi team invites you to one."}
            </EmptyDescription>
          </EmptyHeader>
          {staff && (
            <EmptyContent>
              <span className="flex flex-wrap items-center justify-center gap-1.5 text-xs text-muted-foreground">
                <Kbd>porter hub login</Kbd>, then <Kbd>porter hub push</Kbd> from the project
              </span>
            </EmptyContent>
          )}
        </Empty>
      ) : (
        <ItemGroup className="animate-enter overflow-hidden rounded-lg bg-card ring-1 ring-foreground/10">
          {projects.map((p) => {
            const pill = projectHeadline(p.latest?.status ?? null);
            return (
              <Item
                key={p.id}
                size="sm"
                className="rounded-none border-0 border-t px-4 py-3 first:border-t-0 hover:bg-muted/60"
                render={<Link href={`/p/${p.slug}`} />}
              >
                <ItemContent className="min-w-0">
                  <ItemTitle className="w-full truncate text-sm font-semibold" title={p.name}>
                    {p.name}
                  </ItemTitle>
                  <ItemDescription>
                    {p.latest ? `v${p.latest.number} · ${fmtAgo(p.latest.createdAt)}` : "Nothing pushed yet"}
                  </ItemDescription>
                </ItemContent>
                <ItemActions>
                  {pill && <Chip state={pill.state} label={pill.label} />}
                  <ChevronRightIcon className="size-4 text-muted-foreground" aria-hidden="true" />
                </ItemActions>
              </Item>
            );
          })}
        </ItemGroup>
      )}
      {staff && <NewProjectForm />}
    </Frame>
  );
}
