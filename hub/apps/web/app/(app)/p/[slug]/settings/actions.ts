"use server";
import { connect } from "@porter-hub/db";
import { revalidatePath } from "next/cache";
import * as svc from "../../../../../lib/projects";
import { requireUser } from "../../../../../lib/session";

const done = (slug: string) => revalidatePath(`/p/${slug}/settings`);
const str = (f: FormData, k: string) => String(f.get(k) ?? "");

export async function invite(slug: string, f: FormData) {
  const u = await requireUser();
  const input = svc.parse(svc.schemas.invite, { email: str(f, "email"), role: str(f, "role") });
  await svc.createInvitation(connect().db, u, slug, input);
  done(slug);
}
export async function revoke(slug: string, id: string) {
  await svc.revokeInvitation(connect().db, await requireUser(), slug, id);
  done(slug);
}
export async function setRole(slug: string, userId: string, f: FormData) {
  const { role } = svc.parse(svc.schemas.setRole, { role: str(f, "role") });
  await svc.setRole(connect().db, await requireUser(), slug, userId, role);
  done(slug);
}
export async function removeMember(slug: string, userId: string) {
  await svc.removeMember(connect().db, await requireUser(), slug, userId);
  done(slug);
}
