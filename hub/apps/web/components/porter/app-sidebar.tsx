"use client";
import {
  ChevronsUpDownIcon,
  FolderIcon,
  GlobeIcon,
  LayersIcon,
  LayoutDashboardIcon,
  LogOutIcon,
  MonitorIcon,
  MoonIcon,
  SettingsIcon,
  SunIcon,
  TerminalIcon,
  TriangleAlertIcon,
  UploadIcon,
} from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useTheme } from "next-themes";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from "@/components/ui/sidebar";
import { authClient } from "@/lib/auth-client";
import { initials, LogoMark, type Tab } from "./frame";

const ICONS: Record<string, typeof FolderIcon> = {
  Overview: LayoutDashboardIcon,
  Issues: TriangleAlertIcon,
  URLs: GlobeIcon,
  Uploads: UploadIcon,
  Setup: TerminalIcon,
  Settings: SettingsIcon,
};

// 44px rows for touch; the desktop pointer gets the denser 36px.
const row = "h-11 pointer-fine:h-9 text-sm press";

export function AppSidebar({
  projects,
  staff,
  current,
  tabs = [],
  who,
  role,
}: {
  projects: { name: string; slug: string }[];
  staff: boolean;
  current?: { name: string; slug: string };
  tabs?: Tab[];
  who: string;
  role?: string;
}) {
  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              size="lg"
              className="text-sm font-semibold group-data-[collapsible=icon]:p-1!"
              tooltip="Porter"
              render={<Link href="/" />}
            >
              <LogoMark />
              <span>Porter</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        {current && tabs.length > 0 && (
          <SidebarGroup>
            <SidebarGroupLabel className="truncate">{current.name}</SidebarGroupLabel>
            <SidebarMenu>
              {tabs.map((t) => {
                const Icon = ICONS[t.label] ?? FolderIcon;
                return (
                  <SidebarMenuItem key={t.href}>
                    <SidebarMenuButton
                      className={row}
                      isActive={t.active}
                      tooltip={t.label}
                      render={<Link href={t.href} aria-current={t.active ? "page" : undefined} />}
                    >
                      <Icon />
                      <span>{t.label}</span>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                );
              })}
            </SidebarMenu>
          </SidebarGroup>
        )}
        <SidebarGroup>
          <SidebarGroupLabel>Projects</SidebarGroupLabel>
          <SidebarMenu>
            {staff && (
              <SidebarMenuItem>
                <SidebarMenuButton
                  className={row}
                  isActive={!current}
                  tooltip="All projects"
                  render={<Link href="/" aria-current={current ? undefined : "page"} />}
                >
                  <LayersIcon />
                  <span>All projects</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
            )}
            {projects.map((p) => {
              const active = p.slug === current?.slug;
              return (
                <SidebarMenuItem key={p.slug}>
                  <SidebarMenuButton
                    className={row}
                    isActive={active}
                    tooltip={p.name}
                    render={<Link href={`/p/${p.slug}`} aria-current={active ? "page" : undefined} />}
                  >
                    <FolderIcon />
                    <span>{p.name}</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              );
            })}
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter>
        <UserMenu who={who} role={role} />
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}

function UserMenu({ who, role }: { who: string; role?: string }) {
  const { theme, setTheme } = useTheme();
  const router = useRouter();
  const signOut = async () => {
    await authClient.signOut();
    router.push("/sign-in");
    router.refresh();
  };
  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={<SidebarMenuButton size="lg" className="press data-popup-open:bg-sidebar-accent" />}
          >
            <Avatar className="size-8">
              <AvatarFallback className="text-[10px]">{initials(who)}</AvatarFallback>
            </Avatar>
            <span className="grid min-w-0 flex-1 text-left leading-tight">
              <span className="truncate text-sm font-medium">{who}</span>
              {role && <span className="truncate text-xs text-muted-foreground">{role}</span>}
            </span>
            <ChevronsUpDownIcon className="ml-auto" />
          </DropdownMenuTrigger>
          <DropdownMenuContent side="top" align="start" className="min-w-56">
            <DropdownMenuGroup>
              <DropdownMenuLabel>Theme</DropdownMenuLabel>
              <DropdownMenuRadioGroup value={theme ?? "system"} onValueChange={(v) => setTheme(String(v))}>
                <DropdownMenuRadioItem value="light" className="min-h-11 pointer-fine:min-h-8">
                  <SunIcon />
                  Light
                </DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="dark" className="min-h-11 pointer-fine:min-h-8">
                  <MoonIcon />
                  Dark
                </DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="system" className="min-h-11 pointer-fine:min-h-8">
                  <MonitorIcon />
                  System
                </DropdownMenuRadioItem>
              </DropdownMenuRadioGroup>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuItem className="min-h-11 pointer-fine:min-h-8" onClick={signOut}>
              <LogOutIcon />
              Sign out
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}
