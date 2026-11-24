import Link from "next/link";
import { Fragment, type ReactNode } from "react";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
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

function Logo({ link }: { link: boolean }) {
  const mark = (
    <>
      <span className="flex size-6 items-center justify-center rounded-md bg-foreground">
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

const initials = (s: string) =>
  s
    .split(/[\s@.]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase())
    .join("");

/** Page chrome: header with breadcrumbs, the signed-in user, optional tabs, and the content column. */
export function Frame({
  crumbs = [],
  who,
  role,
  tabs,
  children,
  narrow,
  home = true,
}: {
  home?: boolean;
  crumbs?: Crumb[];
  who?: string;
  role?: string;
  tabs?: Tab[];
  children: ReactNode;
  narrow?: boolean;
}) {
  const width = narrow ? "max-w-[1000px]" : "max-w-[1200px]";
  return (
    <>
      <header className="border-b bg-background">
        <div className={cn("mx-auto flex min-h-14 flex-wrap items-center gap-3 px-4 sm:px-6", width)}>
          <Logo link={home} />
          {crumbs.length > 0 && (
            <Breadcrumb className="min-w-0">
              <BreadcrumbList className="flex-nowrap text-sm">
                {crumbs.map((c) => (
                  <Fragment key={c.label}>
                    <BreadcrumbSeparator />
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
          )}
          <span className="flex-1" />
          {who && (
            <span className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
              <span className="truncate">
                {who}
                {role && <span className="text-muted-foreground/70"> · {role}</span>}
              </span>
              <Avatar className="size-7">
                <AvatarFallback className="text-[10px]">{initials(who)}</AvatarFallback>
              </Avatar>
            </span>
          )}
        </div>
        {tabs && tabs.length > 1 && (
          <nav aria-label="Project" className={cn("mx-auto flex gap-1 overflow-x-auto px-4 sm:px-6", width)}>
            {tabs.map((t) => (
              <Link
                key={t.href}
                href={t.href}
                aria-current={t.active ? "page" : undefined}
                className={cn(
                  "-mb-px flex min-h-11 items-center border-b-2 px-3 text-sm whitespace-nowrap transition-colors",
                  t.active
                    ? "border-primary font-medium text-foreground"
                    : "border-transparent text-muted-foreground hover:text-foreground",
                )}
              >
                {t.label}
              </Link>
            ))}
          </nav>
        )}
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
