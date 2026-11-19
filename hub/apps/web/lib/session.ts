import { headers } from "next/headers";
import { redirect } from "next/navigation";
import { getAuth } from "./auth";

/** The signed-in user for a server component or action; sends everyone else to sign-in. */
export async function requireUser() {
  const s = await getAuth().api.getSession({ headers: await headers() });
  if (!s) redirect("/sign-in");
  return { id: s.user.id, name: s.user.name, email: s.user.email };
}
