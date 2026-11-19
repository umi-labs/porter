import Link from "next/link";
import type { ReactNode } from "react";

export type Crumb = { label: string; href?: string };
export type Tab = { label: string; href: string; active?: boolean };

function Logo({ link }: { link: boolean }) {
  const cls = "flex items-center gap-2 font-semibold text-ink no-underline hover:no-underline";
  const mark = (
    <>
      <span className="flex size-[26px] items-center justify-center rounded-[7px] bg-ink">
        <svg
          width="15"
          height="15"
          viewBox="0 0 24 24"
          fill="none"
          className="stroke-bg"
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
  return link ? (
    <Link href="/" className={cls}>
      {mark}
    </Link>
  ) : (
    <span className={cls}>{mark}</span>
  );
}

/** Page chrome: header with breadcrumbs, the signed-in user, optional tabs, and the content column. */
export function Frame({
  crumbs = [],
  who,
  tabs,
  children,
  narrow,
  home = true,
}: {
  home?: boolean;
  crumbs?: Crumb[];
  who?: string;
  tabs?: Tab[];
  children: ReactNode;
  narrow?: boolean;
}) {
  const width = narrow ? "max-w-[1000px]" : "max-w-[1200px]";
  return (
    <>
      <header className="border-b border-border bg-bg">
        <div className={`mx-auto flex min-h-14 flex-wrap items-center gap-4 px-6 ${width}`}>
          <Logo link={home} />
          {crumbs.map((c) => (
            <span key={c.label} className="flex min-w-0 items-center gap-4 text-[14px]">
              <span aria-hidden="true" className="text-border-strong">
                /
              </span>
              {c.href ? (
                <Link href={c.href} className="truncate text-muted">
                  {c.label}
                </Link>
              ) : (
                <span className="truncate font-semibold">{c.label}</span>
              )}
            </span>
          ))}
          <span className="flex-1" />
          {who && <span className="truncate text-small text-muted">{who}</span>}
        </div>
        {tabs && tabs.length > 1 && (
          <nav aria-label="Project" className={`mx-auto flex gap-1 overflow-x-auto px-6 ${width}`}>
            {tabs.map((t) => (
              <Link
                key={t.href}
                href={t.href}
                aria-current={t.active ? "page" : undefined}
                className={`flex min-h-11 items-center whitespace-nowrap border-b-2 px-3 text-[14px] no-underline hover:no-underline ${
                  t.active ? "border-ink font-semibold text-ink" : "border-transparent font-medium text-muted"
                }`}
              >
                {t.label}
              </Link>
            ))}
          </nav>
        )}
      </header>
      <main className={`mx-auto flex flex-col gap-5 px-6 pt-7 pb-16 ${width}`}>{children}</main>
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
        <h1 className="m-0 text-display font-semibold tracking-[-0.01em] break-words">{title}</h1>
        {sub && <span className="text-[14px] text-muted">{sub}</span>}
      </div>
      {actions && <div className="flex flex-wrap gap-2">{actions}</div>}
    </div>
  );
}
