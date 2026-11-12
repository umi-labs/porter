import type { ReactNode } from "react";

/** A table that scrolls sideways inside its card instead of overflowing the page. */
export function Table({ head, children }: { head: ReactNode[]; children: ReactNode }) {
  return (
    <div className="-mx-5 overflow-x-auto">
      <table className="w-full border-collapse text-[14px] [&_td]:border-t [&_td]:border-panel [&_td]:px-5 [&_td]:py-2.5 [&_td]:align-middle [&_th]:bg-surface [&_th]:px-5 [&_th]:py-2.5 [&_th]:text-left [&_th]:text-small [&_th]:font-semibold [&_th]:text-muted">
        <thead>
          <tr>
            {head.map((h, i) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: fixed column list
              <th key={i}>{h}</th>
            ))}
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}
