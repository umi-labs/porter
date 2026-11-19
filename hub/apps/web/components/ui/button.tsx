import type { ButtonHTMLAttributes } from "react";

export type Variant = "primary" | "secondary" | "dark" | "danger" | "ghost";
const VARIANTS: Record<Variant, string> = {
  primary: "border-accent bg-accent text-on-accent",
  secondary: "border-border-strong bg-bg text-ink hover:bg-hover",
  dark: "border-ink bg-ink text-bg",
  danger: "border-err bg-bg text-err hover:bg-err-bg",
  ghost: "border-transparent bg-transparent text-ink hover:bg-hover",
};

/** Classes for a button, or a link styled as one. 44px minimum height: the hub's touch target. */
export const buttonClass = (variant: Variant = "secondary", extra = "") =>
  `inline-flex min-h-11 items-center justify-center gap-2 rounded-md border px-4 text-[14px] font-semibold no-underline hover:no-underline disabled:cursor-not-allowed disabled:opacity-50 ${VARIANTS[variant]} ${extra}`;

export function Button({
  variant,
  className,
  type = "button",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return <button type={type} className={buttonClass(variant, className)} {...rest} />;
}
