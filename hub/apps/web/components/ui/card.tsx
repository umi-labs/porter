import type { ReactNode } from "react";

export function Card({
  title,
  aside,
  children,
  className = "",
  id,
}: {
  title?: ReactNode;
  aside?: ReactNode;
  children: ReactNode;
  className?: string;
  id?: string;
}) {
  return (
    <section
      id={id}
      className={`flex min-w-0 flex-col gap-3 rounded-lg border border-border bg-bg p-5 ${className}`}
    >
      {(title || aside) && (
        <div className="flex flex-wrap items-baseline gap-2.5">
          {title && <h2 className="m-0 text-heading font-semibold">{title}</h2>}
          {aside && <div className="text-small text-muted">{aside}</div>}
        </div>
      )}
      {children}
    </section>
  );
}
