import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { HttpError, listInvitations, listMembers } from "../../../../../lib/projects";
import { requireUser } from "../../../../../lib/session";
import { invite, removeMember, revoke, setRole } from "./actions";

export const dynamic = "force-dynamic";
const ROLES = ["owner", "developer", "viewer", "client"];
const inline = { display: "inline" };

export default async function Settings({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const user = await requireUser();
  const db = getDb();
  let members: Awaited<ReturnType<typeof listMembers>>;
  let invitations: Awaited<ReturnType<typeof listInvitations>>;
  try {
    [members, invitations] = await Promise.all([
      listMembers(db, user, slug),
      listInvitations(db, user, slug),
    ]);
  } catch (e) {
    if (e instanceof HttpError) notFound(); // 404 and 403 look the same here
    throw e;
  }
  return (
    <main>
      <h1>{slug} settings</h1>
      <h2>Members</h2>
      <ul>
        {members.map((m) => (
          <li key={m.userId}>
            {m.name} &lt;{m.email}&gt;{" "}
            <form action={setRole.bind(null, slug, m.userId)} style={inline}>
              <select name="role" defaultValue={m.role}>
                {ROLES.map((r) => (
                  <option key={r}>{r}</option>
                ))}
              </select>
              <button type="submit">Change role</button>
            </form>{" "}
            <form action={removeMember.bind(null, slug, m.userId)} style={inline}>
              <button type="submit">Remove</button>
            </form>
          </li>
        ))}
      </ul>
      <h2>Invitations</h2>
      <ul>
        {invitations.map((i) => (
          <li key={i.id}>
            {i.email} ({i.role}), expires {i.expiresAt.toISOString().slice(0, 10)}{" "}
            <form action={revoke.bind(null, slug, i.id)} style={inline}>
              <button type="submit">Revoke</button>
            </form>
          </li>
        ))}
      </ul>
      <form action={invite.bind(null, slug)}>
        <label>
          Email <input name="email" type="email" required />
        </label>{" "}
        <select name="role" defaultValue="client">
          {ROLES.map((r) => (
            <option key={r}>{r}</option>
          ))}
        </select>{" "}
        <button type="submit">Invite</button>
      </form>
      <h2>Share links</h2>
      <p>Coming soon.</p>
      <h2>Run secrets</h2>
      <p>Coming soon.</p>
      <h2>Retention</h2>
      <p>Coming soon.</p>
    </main>
  );
}
