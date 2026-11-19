import type { ReactNode } from "react";
import { fmtDate } from "../../lib/format";
import { Frame } from "./frame";

/** The permanent bar on every public /s/ page. `expiresAt` is null when the link didn't resolve. */
export function ShareBar({ expiresAt, now = new Date() }: { expiresAt: Date | null; now?: Date }) {
  const days = expiresAt ? Math.max(0, Math.ceil((expiresAt.getTime() - now.getTime()) / 86_400_000)) : null;
  return (
    <div
      data-share-bar
      className="sticky top-0 z-10 flex flex-wrap items-center gap-2.5 bg-[#18181B] px-4 py-2.5 text-small text-white"
    >
      <svg
        width="14"
        height="14"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        aria-hidden="true"
      >
        <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" />
        <circle cx="12" cy="12" r="3" />
      </svg>
      <span className="font-semibold">Shared view · read-only</span>
      {expiresAt && (
        <span className="opacity-80">
          Shared by Umi · expires {days === 0 ? "today" : `in ${days} day${days === 1 ? "" : "s"}`} (
          {fmtDate(expiresAt)})
        </span>
      )}
    </div>
  );
}

/** Chrome for every public /s/[token] page (its layout): the share bar, then the page. No links into the app. */
export function SharedFrame({
  expiresAt,
  projectName,
  children,
}: {
  expiresAt: Date | null;
  projectName: string | null;
  children?: ReactNode;
}) {
  return (
    <>
      <ShareBar expiresAt={expiresAt} />
      <Frame home={false} crumbs={projectName ? [{ label: projectName }] : []}>
        {children}
      </Frame>
    </>
  );
}
