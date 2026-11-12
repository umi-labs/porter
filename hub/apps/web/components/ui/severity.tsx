export type Severity = "error" | "warning" | "info";

const LOOK: Record<Severity, { dot: string; text: string }> = {
  error: { dot: "rounded-full bg-err", text: "text-err" },
  warning: { dot: "rounded-[2px] bg-warn", text: "text-warn" },
  info: { dot: "rounded-full border-[1.5px] border-faint", text: "text-muted" },
};

/** Issue severity: error red circle, warning amber square, info grey ring. Always with the word. */
export function SeverityTag({ severity }: { severity: string }) {
  const l = LOOK[severity as Severity] ?? LOOK.info;
  return (
    <span className={`inline-flex items-center gap-1.5 whitespace-nowrap font-medium ${l.text}`}>
      <span aria-hidden="true" className={`inline-block size-2.5 flex-none ${l.dot}`} />
      {severity}
    </span>
  );
}

/** `open` is loudest; fixed-by-rerun and handled-manually are muted; wont-fix is struck through. */
export function Resolution({ value }: { value: string }) {
  const cls =
    value === "open"
      ? "font-semibold text-ink"
      : value === "wont-fix"
        ? "text-muted line-through"
        : "text-muted";
  return <span className={cls}>{value}</span>;
}
