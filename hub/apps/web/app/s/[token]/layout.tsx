import { getDb } from "@porter-hub/db";
import type { ReactNode } from "react";
import { SharedFrame } from "@/components/porter/share-bar";
import { resolveShare } from "@/lib/shares";

export const dynamic = "force-dynamic";

/** Every /s/ page, including its 404, sits under the "Shared view · read-only" bar. */
export default async function SharedLayout({
  children,
  params,
}: {
  children: ReactNode;
  params: Promise<{ token: string }>;
}) {
  const link = await resolveShare(getDb(), (await params).token);
  return (
    <SharedFrame expiresAt={link?.expiresAt ?? null} projectName={link?.project.name ?? null}>
      {children}
    </SharedFrame>
  );
}
