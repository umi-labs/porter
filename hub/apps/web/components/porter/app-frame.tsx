import { getDb } from "@porter-hub/db";
import { cookies } from "next/headers";
import type { ReactNode } from "react";
import { Separator } from "@/components/ui/separator";
import { SidebarInset, SidebarProvider, SidebarTrigger } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { type Role, roleCan } from "@/lib/permissions";
import { isStaff, listProjects } from "@/lib/projects";
import { cn } from "@/lib/utils";
import { AppSidebar } from "./app-sidebar";
import { type Crumb, Crumbs, type Tab } from "./frame";

/** Signed-in page chrome: the sidebar (projects, the current project's pages, the user menu), a slim bar with breadcrumbs, then the content column. */
export async function AppFrame({
  user,
  crumbs = [],
  project,
  role,
  tabs,
  narrow,
  children,
}: {
  user: { id: string; name: string; email: string };
  crumbs?: Crumb[];
  project?: { name: string; slug: string };
  role?: string;
  tabs?: Tab[];
  narrow?: boolean;
  children: ReactNode;
}) {
  const [projects, staff, jar] = await Promise.all([
    listProjects(getDb(), user),
    isStaff(getDb(), user),
    cookies(),
  ]);
  return (
    <TooltipProvider delay={500}>
      <SidebarProvider defaultOpen={jar.get("sidebar_state")?.value !== "false"}>
        <AppSidebar
          projects={projects.map((p) => ({ name: p.name, slug: p.slug }))}
          staff={staff}
          current={project}
          tabs={tabs}
          who={user.name || user.email}
          role={role}
        />
        <SidebarInset className="bg-transparent">
          <header className="sticky top-0 z-10 flex min-h-14 items-center gap-2 border-b bg-background px-4 sm:px-6">
            <SidebarTrigger className="press -ml-2 size-11 pointer-fine:size-8" />
            {crumbs.length > 0 && (
              <>
                <Separator
                  orientation="vertical"
                  className="mr-1 data-vertical:h-4 data-vertical:self-center"
                />
                <Crumbs crumbs={crumbs} lead={false} />
              </>
            )}
          </header>
          <div
            className={cn(
              "mx-auto flex w-full flex-col gap-5 px-4 pt-7 pb-16 text-sm sm:px-6",
              narrow ? "max-w-[1000px]" : "max-w-[1200px]",
            )}
          >
            {children}
          </div>
        </SidebarInset>
      </SidebarProvider>
    </TooltipProvider>
  );
}

/** The project's tabs for this role. Clients get Overview and Uploads only. */
export function projectTabs(slug: string, role: Role, active: string): Tab[] {
  const tabs: [string, string, boolean][] = [
    ["Overview", "", true],
    ["Issues", "/issues", roleCan(role, "view_raw")],
    ["URLs", "/urls", roleCan(role, "view_raw")],
    ["Uploads", "/uploads", roleCan(role, "view")],
    ["Setup", "/setup", roleCan(role, "push")],
    [
      "Settings",
      "/settings",
      roleCan(role, "share") || roleCan(role, "manage_members") || roleCan(role, "manage_secrets"),
    ],
  ];
  return tabs
    .filter(([, , ok]) => ok)
    .map(([label, path]) => ({ label, href: `/p/${slug}${path}`, active: label === active }));
}

/** A signed-in project page: the sidebar with the role's pages, breadcrumbs, then the content. */
export function ProjectFrame({
  project,
  role,
  user,
  active,
  children,
}: {
  project: { name: string; slug: string };
  role: Role;
  user: { id: string; name: string; email: string };
  active: string;
  children: ReactNode;
}) {
  return (
    <AppFrame
      user={user}
      crumbs={[{ label: "Projects", href: "/" }, { label: project.name }]}
      project={project}
      role={role}
      tabs={projectTabs(project.slug, role, active)}
      narrow={role === "client"}
    >
      {children}
    </AppFrame>
  );
}
