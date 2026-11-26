"use server";
import { getDb } from "@porter-hub/db";
import { revalidatePath } from "next/cache";
import { HttpError } from "@/lib/projects";
import { requireUser } from "@/lib/session";
import { saveSetup } from "@/lib/setup";

const FIELDS = [
  "adapter",
  "kind",
  "upload",
  "url",
  "projectId",
  "dataset",
  "siteUrl",
  "destUrl",
  "sanityToken",
  "payloadKey",
  "wpUser",
  "wpPassword",
];

/** Saves the source setup as a new version. */
export async function save(slug: string, _prev: unknown, f: FormData) {
  try {
    const input = Object.fromEntries(FIELDS.map((k) => [k, String(f.get(k) ?? "")]));
    const { number } = await saveSetup(getDb(), await requireUser(), slug, input);
    revalidatePath(`/p/${slug}`, "layout");
    return { saved: number };
  } catch (e) {
    if (e instanceof HttpError) return { error: e.message };
    throw e;
  }
}
