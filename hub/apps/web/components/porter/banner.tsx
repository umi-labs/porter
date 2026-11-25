import type { ReactNode } from "react";
import { Alert, AlertAction, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { cn } from "@/lib/utils";

export type Tone = "err" | "warn" | "info" | "ok" | "int" | "dry";
type Look = { box: string; square?: boolean; shape: string; ink: string; glyph: string };
const TONES: Record<Tone, Look> = {
  err: { box: "border-err/40 bg-err-soft", shape: "fill-err", ink: "fill-white", glyph: "!" },
  warn: {
    box: "border-warn/40 bg-warn-soft",
    square: true,
    shape: "fill-warn-fill",
    ink: "fill-warn-foreground",
    glyph: "!",
  },
  info: { box: "", shape: "fill-muted-foreground", ink: "fill-background", glyph: "i" },
  // Only for "written and clean".
  ok: { box: "border-ok/40 bg-ok-soft", shape: "fill-ok", ink: "fill-ok-foreground", glyph: "✓" },
  int: { box: "border-int/40 bg-int-soft", shape: "fill-int", ink: "fill-background", glyph: "‖" },
  dry: {
    box: "border-dashed border-dry-line bg-dry-soft",
    shape: "fill-dry",
    ink: "fill-background",
    glyph: "–",
  },
};

/** A status line: glyph + title (+ body), toned by state. Warnings are the only square glyph. */
export function Banner({
  tone,
  title,
  children,
  aside,
  role = "status",
}: {
  tone: Tone;
  title: ReactNode;
  children?: ReactNode;
  aside?: ReactNode;
  role?: "status" | "alert";
}) {
  const t = TONES[tone];
  return (
    <Alert role={role} className={cn("animate-enter gap-x-3! px-4 py-3 text-sm", t.box)}>
      <svg viewBox="0 0 20 20" className="size-5!" aria-hidden="true">
        {t.square ? (
          <rect width="20" height="20" rx="4" className={t.shape} />
        ) : (
          <circle cx="10" cy="10" r="10" className={t.shape} />
        )}
        <text x="10" y="14.5" textAnchor="middle" fontSize="12" fontWeight="800" className={t.ink}>
          {t.glyph}
        </text>
      </svg>
      <AlertTitle className="text-sm font-semibold text-foreground">{title}</AlertTitle>
      {children && <AlertDescription className="text-sm text-foreground/80">{children}</AlertDescription>}
      {aside && <AlertAction>{aside}</AlertAction>}
    </Alert>
  );
}
