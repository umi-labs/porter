import Link from "next/link";
import { Fragment, type ReactNode } from "react";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { cn } from "@/lib/utils";

export type Crumb = { label: string; href?: string };
export type Tab = { label: string; href: string; active?: boolean };

export function LogoMark() {
  return (
    <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-foreground">
      <svg
        viewBox="0 0 24 24"
        fill="none"
        className="size-3.5 stroke-background"
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d="M4 7h11l-3-3M20 17H9l3 3" />
      </svg>
    </span>
  );
}

function Logo({ link }: { link: boolean }) {
  const mark = (
    <>
      <LogoMark />
      Porter
    </>
  );
  const cls = "flex items-center gap-2 text-sm font-semibold text-foreground";
  return link ? (
    <Link href="/" className={cls}>
      {mark}
    </Link>
  ) : (
    <span className={cls}>{mark}</span>
  );
}

export const initials = (s: string) =>
  s
    .split(/[\s@.]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase())
    .join("");

export function Crumbs({ crumbs, lead = true }: { crumbs: Crumb[]; lead?: boolean }) {
  return (
    <Breadcrumb className="min-w-0">
      <BreadcrumbList className="flex-nowrap text-sm">
        {crumbs.map((c, i) => (
          <Fragment key={c.label}>
            {(lead || i > 0) && <BreadcrumbSeparator />}
            <BreadcrumbItem className="min-w-0">
              {c.href ? (
                <BreadcrumbLink render={<Link href={c.href} />} className="truncate">
                  {c.label}
                </BreadcrumbLink>
              ) : (
                <BreadcrumbPage className="truncate font-medium">{c.label}</BreadcrumbPage>
              )}
            </BreadcrumbItem>
          </Fragment>
        ))}
      </BreadcrumbList>
    </Breadcrumb>
  );
}

/** Page chrome without the sidebar (shared links, forbidden): header with logo and breadcrumbs, then the content column. */
export function Frame({
  crumbs = [],
  children,
  narrow,
  home = true,
}: {
  home?: boolean;
  crumbs?: Crumb[];
  children: ReactNode;
  narrow?: boolean;
}) {
  const width = narrow ? "max-w-[1000px]" : "max-w-[1200px]";
  return (
    <>
      <header className="border-b bg-background">
        <div className={cn("mx-auto flex min-h-14 flex-wrap items-center gap-3 px-4 sm:px-6", width)}>
          <Logo link={home} />
          {crumbs.length > 0 && <Crumbs crumbs={crumbs} />}
        </div>
      </header>
      <main className={cn("mx-auto flex flex-col gap-5 px-4 pt-7 pb-16 text-sm sm:px-6", width)}>
        {children}
      </main>
    </>
  );
}

export function PageTitle({
  title,
  sub,
  actions,
}: {
  title: ReactNode;
  sub?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-end gap-4">
      <div className="flex min-w-0 flex-[1_1_400px] flex-col gap-1">
        <h1 className="m-0 font-heading text-2xl font-semibold tracking-tight break-words">{title}</h1>
        {sub && <span className="text-sm text-muted-foreground">{sub}</span>}
      </div>
      {actions && <div className="flex flex-wrap gap-2">{actions}</div>}
    </div>
  );
}
