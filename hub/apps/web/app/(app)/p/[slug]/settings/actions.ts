"use server";
import { getDb } from "@porter-hub/db";
import { revalidatePath } from "next/cache";
import * as svc from "../../../../../lib/projects";
import { requireUser } from "../../../../../lib/session";
import * as shares from "../../../../../lib/shares";

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

export async function createShare(slug: string, _prev: unknown, f: FormData) {
  try {
    const r = await shares.createShare(getDb(), await requireUser(), slug, {
      scope: str(f, "scope"),
      days: str(f, "days"),
    });
    done(slug);
    // Returned to the form (shown once); never stored or put in a URL.
    return { token: r.token, expiresAt: r.expiresAt.toISOString() };
  } catch (e) {
    if (e instanceof svc.HttpError) return { error: e.message };
    throw e;
  }
}
export async function revokeShare(slug: string, id: string) {
  try {
    await shares.revokeShare(getDb(), await requireUser(), slug, id);
  } catch (e) {
    if (!(e instanceof svc.HttpError && e.status === 404)) throw e; // already revoked in another tab
  }
  done(slug);
}
