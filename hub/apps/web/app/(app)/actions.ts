"use server";
import { getDb } from "@porter-hub/db";
import { redirect } from "next/navigation";
import * as svc from "@/lib/projects";
import { requireUser } from "@/lib/session";

/** Staff only (createProject checks). The new project opens on its settings, to add people. */
export async function createProject(_prev: unknown, f: FormData) {
  let slug: string;
  try {
    const slugIn = String(f.get("slug") ?? "").trim();
    const input = svc.parse(svc.schemas.createProject, {
      name: String(f.get("name") ?? ""),
      slug: slugIn || undefined,
    });
    slug = (await svc.createProject(getDb(), await requireUser(), input)).slug;
  } catch (e) {
    if (e instanceof svc.HttpError) return { error: e.message };
    throw e;
  }
  redirect(`/p/${slug}/settings`);
}
