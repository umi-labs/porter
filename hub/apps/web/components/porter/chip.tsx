import type { ReactNode } from "react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/** The stage states from docs/design/README.md. Stale is not a state: it is a modifier (see `Stale`). */
export const STATES = [
  "not-run",
  "running",
  "clean",
  "warnings",
  "errors",
  "failed",
  "interrupted",
  "dry-run",
  "skipped",
] as const;
export type State = (typeof STATES)[number];

type Look = { glyph: string; word: string; chip: string; dot: string };
/** Glyph + word + classes per state. Only `clean` uses the `ok` colours: green means written. */
export const LOOKS: Record<State, Look> = {
  "not-run": {
    glyph: "",
    word: "Not run",
    chip: "text-muted-foreground border-dashed border-border bg-transparent",
    dot: "rounded-full border-dashed border-muted-foreground",
  },
  running: {
    glyph: "",
    word: "Running",
    chip: "text-primary bg-primary/10 dark:text-orange-300 dark:bg-primary/25",
    dot: "rounded-full border-dotted border-primary",
  },
  clean: {
    glyph: "✓",
    word: "Clean",
    chip: "text-ok bg-ok-soft",
    dot: "rounded-full bg-ok border-ok text-ok-foreground",
  },
  warnings: {
    glyph: "!",
    word: "Warnings",
    chip: "text-warn bg-warn-soft",
    dot: "rounded-[3px] bg-warn-fill border-warn-fill text-warn-foreground",
  },
  errors: {
    glyph: "!",
    word: "Errors",
    chip: "text-err bg-err-soft border-err/50",
    dot: "rounded-full bg-err border-err text-white",
  },
  failed: {
    glyph: "✕",
    word: "Failed",
    chip: "text-white bg-err",
    dot: "rounded-full bg-white border-white text-err",
  },
  interrupted: {
    glyph: "‖",
    word: "Interrupted",
    chip: "text-int bg-int-soft",
    dot: "rounded-full border-int",
  },
  "dry-run": {
    glyph: "",
    word: "Dry run",
    chip: "text-dry bg-dry-hatch border-dashed border-dry-line",
    dot: "rounded-full border-dashed border-dry",
  },
  skipped: {
    glyph: "–",
    word: "Skipped",
    chip: "text-muted-foreground border-border bg-transparent",
    dot: "rounded-full border-muted-foreground",
  },
};

/** A state chip: always a glyph and a word, never colour alone. `label` replaces the word (keep it a word). */
export function Chip({ state, label, title }: { state: State; label?: ReactNode; title?: string }) {
  const l = LOOKS[state];
  return (
    <Badge
      data-state={state}
      title={title}
      className={cn("h-6 max-w-full self-start rounded-md px-2 pl-1.5 text-xs font-medium", l.chip)}
    >
      <span
        aria-hidden="true"
        className={cn(
          "inline-flex size-3.5 flex-none items-center justify-center border-[1.5px] text-[9px] leading-none font-extrabold",
          l.dot,
        )}
      >
        {l.glyph}
      </span>
      <span className="truncate">{label ?? l.word}</span>
    </Badge>
  );
}

/**
 * Stale is a modifier on the previous result, not a replacement: the result stays (dimmed to 62%) and the reason
 * is shown beside it, undimmed.
 */
export function Stale({
  reason,
  plain,
  children,
}: {
  reason: string | null;
  /** Client copy: the dim stays, the reason is just "Needs re-running". */
  plain?: boolean;
  children: ReactNode;
}) {
  if (!reason) return <>{children}</>;
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <div className="flex min-w-0 flex-col gap-1.5 opacity-[.62]">{children}</div>
      <span className="text-xs text-muted-foreground">
        <span aria-hidden="true">↻ </span>
        {plain ? "Needs re-running" : `Stale: ${reason}`}
      </span>
    </div>
  );
}
