import { EyeIcon } from "lucide-react";
import type { ReactNode } from "react";
import { fmtDate } from "@/lib/format";
import { Frame } from "./frame";

/** The permanent bar on every public /s/ page. `expiresAt` is null when the link didn't resolve. */
export function ShareBar({ expiresAt, now = new Date() }: { expiresAt: Date | null; now?: Date }) {
  const days = expiresAt ? Math.max(0, Math.ceil((expiresAt.getTime() - now.getTime()) / 86_400_000)) : null;
  return (
    <div
      data-share-bar
      className="sticky top-0 z-10 flex flex-wrap items-center gap-2.5 bg-foreground px-4 py-2.5 text-xs text-background"
    >
      <EyeIcon className="size-3.5" aria-hidden="true" />
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
