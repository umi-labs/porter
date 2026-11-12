import type { ReactNode } from "react";

/** The stage states from docs/design/README.md. Stale is not a state: it is a modifier (see `Chip`'s `stale`). */
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
    chip: "text-muted border-dashed border-border-strong",
    dot: "rounded-full border-dashed border-faint",
  },
  running: {
    glyph: "",
    word: "Running",
    chip: "text-accent bg-accent-bg border-accent-bg",
    dot: "rounded-full border-dotted border-accent",
  },
  clean: {
    glyph: "✓",
    word: "Clean",
    chip: "text-ok bg-ok-bg border-ok-bg",
    dot: "rounded-full bg-ok border-ok text-on-ok",
  },
  warnings: {
    glyph: "!",
    word: "Warnings",
    chip: "text-warn bg-warn-bg border-warn-bg",
    dot: "rounded-[3px] border-warn",
  },
  errors: {
    glyph: "!",
    word: "Errors",
    chip: "text-err bg-err-bg border-err",
    dot: "rounded-full bg-err border-err text-on-err",
  },
  failed: {
    glyph: "✕",
    word: "Failed",
    chip: "text-white bg-err-solid border-err-solid",
    dot: "rounded-full bg-white border-white text-err-solid",
  },
  interrupted: {
    glyph: "‖",
    word: "Interrupted",
    chip: "text-int bg-int-bg border-int-bg",
    dot: "rounded-full border-int",
  },
  "dry-run": {
    glyph: "",
    word: "Dry run",
    chip: "text-dry bg-dry-hatch border-dashed border-dry-border",
    dot: "rounded-full border-dashed border-dry",
  },
  skipped: {
    glyph: "–",
    word: "Skipped",
    chip: "text-faint border-border",
    dot: "rounded-full border-faint",
  },
};

/** A state chip: always a glyph and a word, never colour alone. `label` replaces the word (keep it a word). */
export function Chip({ state, label, title }: { state: State; label?: ReactNode; title?: string }) {
  const l = LOOKS[state];
  return (
    <span
      data-state={state}
      title={title}
      className={`inline-flex h-6 max-w-full items-center gap-1.5 self-start whitespace-nowrap rounded-sm border pr-2 pl-1.5 text-small font-medium ${l.chip}`}
    >
      <span
        aria-hidden="true"
        className={`inline-flex size-3.5 flex-none items-center justify-center border-[1.5px] text-[9px] leading-none font-extrabold ${l.dot}`}
      >
        {l.glyph}
      </span>
      <span className="truncate">{label ?? l.word}</span>
    </span>
  );
}

/**
 * Stale is a modifier on the previous result, not a replacement: the result stays (dimmed to 62%) and the reason
 * is shown beside it, undimmed.
 */
export function Stale({ reason, children }: { reason: string | null; children: ReactNode }) {
  if (!reason) return <>{children}</>;
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <div className="flex min-w-0 flex-col gap-1.5 opacity-[.62]">{children}</div>
      <span className="text-small text-muted">
        <span aria-hidden="true">↻ </span>Stale: {reason}
      </span>
    </div>
  );
}
