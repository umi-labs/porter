import type { ReactNode } from "react";

export type Tone = "err" | "warn" | "info" | "ok" | "int" | "dry";
const TONES: Record<Tone, { box: string; icon: string; glyph: string }> = {
  err: { box: "border-err/40 bg-err-bg", icon: "rounded-full bg-err text-on-err", glyph: "!" },
  warn: { box: "border-warn/40 bg-warn-bg", icon: "rounded-[4px] bg-warn text-bg", glyph: "!" },
  info: { box: "border-border bg-bg", icon: "rounded-full bg-muted text-bg", glyph: "i" },
  // Only for "written and clean".
  ok: { box: "border-ok/40 bg-ok-bg", icon: "rounded-full bg-ok text-on-ok", glyph: "✓" },
  int: { box: "border-int/40 bg-int-bg", icon: "rounded-full bg-int text-bg", glyph: "‖" },
  dry: { box: "border-dashed border-dry-border bg-dry-bg", icon: "rounded-full bg-dry text-bg", glyph: "–" },
};

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
    <div role={role} className={`flex items-start gap-3.5 rounded-lg border px-4.5 py-4 ${t.box}`}>
      <span
        aria-hidden="true"
        className={`flex size-5.5 flex-none items-center justify-center text-small font-extrabold ${t.icon}`}
      >
        {t.glyph}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-heading font-semibold">{title}</span>
        {children && <span className="text-ink/80">{children}</span>}
      </div>
      {aside}
    </div>
  );
}
