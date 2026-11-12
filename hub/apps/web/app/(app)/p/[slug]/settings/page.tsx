import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { roleCan } from "../../../../../lib/permissions";
import { access, HttpError, listInvitations, listMembers } from "../../../../../lib/projects";
import { listSecrets } from "../../../../../lib/secrets";
import { requireUser } from "../../../../../lib/session";
import { listShares } from "../../../../../lib/shares";
import { deleteSecret, invite, removeMember, revoke, revokeShare, setRole } from "./actions";
import { SecretForm } from "./secret-form";
import { ShareForm } from "./share-form";

export const dynamic = "force-dynamic";
const ROLES = ["owner", "developer", "viewer", "client"];
const inline = { display: "inline" };

export default async function Settings({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const user = await requireUser();
  const db = getDb();
  let members: Awaited<ReturnType<typeof listMembers>> = [];
  let invitations: Awaited<ReturnType<typeof listInvitations>> = [];
  let shares: Awaited<ReturnType<typeof listShares>> = [];
  let secrets: Awaited<ReturnType<typeof listSecrets>> = [];
  let canShare: boolean;
  let canMembers: boolean;
  let canSecrets: boolean;
  try {
    const { role } = await access(db, user, slug, "view");
    canShare = roleCan(role, "share");
    canMembers = roleCan(role, "manage_members");
    canSecrets = roleCan(role, "manage_secrets");
    if (!canShare && !canMembers && !canSecrets) notFound();
    if (canSecrets) secrets = await listSecrets(db, user, slug);
    if (canMembers)
      [members, invitations] = await Promise.all([
        listMembers(db, user, slug),
        listInvitations(db, user, slug),
      ]);
    if (canShare) shares = await listShares(db, user, slug);
  } catch (e) {
    if (e instanceof HttpError) notFound(); // 404 and 403 look the same here
    throw e;
  }
  return (
    <main>
      <h1>{slug} settings</h1>
      {canMembers && (
        <>
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
        </>
      )}
      <h2>Share links</h2>
      {canShare ? (
        <>
          <ul>
            {shares.map((l) => (
              <li key={l.id}>
                {l.scope}, expires {l.expiresAt.toISOString().slice(0, 10)}
                {l.revokedAt ? " (revoked)" : l.expiresAt < new Date() ? " (expired)" : ""}{" "}
                {!l.revokedAt && (
                  <form action={revokeShare.bind(null, slug, l.id)} style={inline}>
                    <button type="submit">Revoke</button>
                  </form>
                )}
              </li>
            ))}
          </ul>
          <ShareForm slug={slug} />
        </>
      ) : (
        <p>You can't manage share links.</p>
      )}
      <h2>Run secrets</h2>
      {canSecrets ? (
        <>
          <p>
            Passed to remote runs as environment variables only. Values are encrypted and never shown again;
            replace or delete them here.
          </p>
          <ul>
            {secrets.map((x) => (
              <li key={x.name}>
                <code>{x.name}</code> set <SecretForm slug={slug} name={x.name} />{" "}
                <form action={deleteSecret.bind(null, slug, x.name)} style={inline}>
                  <button type="submit">Delete</button>
                </form>
              </li>
            ))}
          </ul>
          <SecretForm slug={slug} />
        </>
      ) : (
        <p>You can't manage run secrets.</p>
      )}
      <h2>Retention</h2>
      <p>Coming soon.</p>
    </main>
  );
}
