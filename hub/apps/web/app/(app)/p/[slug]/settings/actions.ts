"use server";
import { getDb } from "@porter-hub/db";
import { revalidatePath } from "next/cache";
import * as svc from "../../../../../lib/projects";
import { requireUser } from "../../../../../lib/session";

const done = (slug: string) => revalidatePath(`/p/${slug}/settings`);
const str = (f: FormData, k: string) => String(f.get(k) ?? "");

export async function invite(slug: string, f: FormData) {
  const u = await requireUser();
  await svc.access(getDb(), u, slug, "manage_members");
  const input = svc.parse(svc.schemas.invite, { email: str(f, "email"), role: str(f, "role") });
  await svc.createInvitation(getDb(), u, slug, input);
  done(slug);
}
export async function revoke(slug: string, id: string) {
  await svc.revokeInvitation(getDb(), await requireUser(), slug, id);
  done(slug);
}
export async function setRole(slug: string, userId: string, f: FormData) {
  const u = await requireUser();
  await svc.access(getDb(), u, slug, "manage_members");
  const { role } = svc.parse(svc.schemas.setRole, { role: str(f, "role") });
  await svc.setRole(getDb(), u, slug, userId, role);
  done(slug);
}
export async function removeMember(slug: string, userId: string) {
  await svc.removeMember(getDb(), await requireUser(), slug, userId);
  done(slug);
}
