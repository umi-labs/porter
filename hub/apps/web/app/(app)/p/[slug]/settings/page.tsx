import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { buttonClass } from "../../../../../components/ui/button";
import { Card } from "../../../../../components/ui/card";
import { PageTitle } from "../../../../../components/ui/frame";
import { Table } from "../../../../../components/ui/table";
import { fmtDate } from "../../../../../lib/format";
import { roleCan } from "../../../../../lib/permissions";
import { access, HttpError, listInvitations, listMembers } from "../../../../../lib/projects";
import { listSecrets } from "../../../../../lib/secrets";
import { requireUser } from "../../../../../lib/session";
import { listShares } from "../../../../../lib/shares";
import { ProjectFrame } from "../../../../../lib/view-parts";
import { deleteSecret, invite, removeMember, revoke, revokeShare, setRole } from "./actions";
import { SecretForm } from "./secret-form";
import { ShareForm } from "./share-form";

export const dynamic = "force-dynamic";
const ROLES = ["owner", "developer", "viewer", "client"];

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
  let ctx: Awaited<ReturnType<typeof access>>;
  try {
    ctx = await access(db, user, slug, "view");
    const { role } = ctx;
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
  const now = new Date();
  const sections: [string, string, boolean][] = [
    ["members", "Members and roles", canMembers],
    ["shares", "Share links", true],
    ["secrets", "Run secrets", true],
    ["retention", "Retention", true],
  ];
  return (
    <ProjectFrame project={ctx.project} role={ctx.role} user={user} active="Settings">
      <PageTitle title="Settings" />
      <div className="flex flex-wrap gap-7">
        <nav aria-label="Settings" className="flex max-w-[200px] flex-[1_1_180px] flex-col gap-0.5">
          {sections
            .filter(([, , on]) => on)
            .map(([id, label]) => (
              <a
                key={id}
                href={`#${id}`}
                className="flex min-h-11 items-center rounded-md px-3 text-[14px] text-muted no-underline hover:bg-hover hover:no-underline"
              >
                {label}
              </a>
            ))}
        </nav>
        <div className="flex min-w-0 flex-[999_1_640px] flex-col gap-5">
          {canMembers && (
            <Card id="members" title="Members and roles">
              <ul className="m-0 flex list-none flex-col p-0">
                {members.map((m) => (
                  <li
                    key={m.userId}
                    className="flex flex-wrap items-center gap-2.5 border-t border-panel py-2.5"
                  >
                    <span className="min-w-[180px] flex-1 break-words">
                      {m.name} <span className="text-muted">&lt;{m.email}&gt;</span>
                    </span>
                    <form action={setRole.bind(null, slug, m.userId)} className="flex items-center gap-2">
                      <select name="role" defaultValue={m.role} aria-label={`Role for ${m.email}`}>
                        {ROLES.map((r) => (
                          <option key={r}>{r}</option>
                        ))}
                      </select>
                      <button type="submit" className={buttonClass("secondary")}>
                        Change role
                      </button>
                    </form>
                    <form action={removeMember.bind(null, slug, m.userId)}>
                      <button type="submit" className={buttonClass("danger")}>
                        Remove
                      </button>
                    </form>
                  </li>
                ))}
              </ul>
              <h3 className="m-0 mt-2 text-[14px] font-semibold">Invitations</h3>
              {invitations.length === 0 ? (
                <p className="m-0 text-small text-muted">No open invitations.</p>
              ) : (
                <ul className="m-0 flex list-none flex-col p-0">
                  {invitations.map((i) => (
                    <li
                      key={i.id}
                      className="flex flex-wrap items-center gap-2.5 border-t border-panel py-2.5"
                    >
                      <span className="min-w-[180px] flex-1 break-words">
                        {i.email}{" "}
                        <span className="text-muted">
                          ({i.role}), expires {fmtDate(i.expiresAt)}
                        </span>
                      </span>
                      <form action={revoke.bind(null, slug, i.id)}>
                        <button type="submit" className={buttonClass("danger")}>
                          Revoke
                        </button>
                      </form>
                    </li>
                  ))}
                </ul>
              )}
              <form action={invite.bind(null, slug)} className="flex flex-wrap items-end gap-2">
                <label className="flex min-w-0 flex-[1_1_240px] flex-col gap-1.5 text-[14px] font-semibold">
                  Email
                  <input name="email" type="email" required className="font-normal" />
                </label>
                <label className="flex flex-col gap-1.5 text-[14px] font-semibold">
                  Role
                  <select name="role" defaultValue="client" className="font-normal">
                    {ROLES.map((r) => (
                      <option key={r}>{r}</option>
                    ))}
                  </select>
                </label>
                <button type="submit" className={buttonClass("dark")}>
                  Invite
                </button>
              </form>
            </Card>
          )}
          <Card
            id="shares"
            title="Share links"
            aside="Let someone without an account see this project. Every link expires, and you can revoke it at any time."
          >
            {canShare ? (
              <>
                <ShareForm slug={slug} />
                {shares.length > 0 && (
                  <Table head={["Fingerprint", "Scope", "Created", "Expires", ""]}>
                    {shares.map((l) => {
                      const active = !l.revokedAt && l.expiresAt >= now;
                      return (
                        <tr key={l.id} className={active ? "" : "text-muted"}>
                          <td className={`font-mono text-small ${l.revokedAt ? "line-through" : ""}`}>
                            …/s/{l.fingerprint}···
                          </td>
                          <td>{l.scope === "report" ? "Reports only" : "Read-only project"}</td>
                          <td className="whitespace-nowrap">{fmtDate(l.createdAt)}</td>
                          <td className="whitespace-nowrap">{fmtDate(l.expiresAt)}</td>
                          <td className="text-right">
                            {active ? (
                              <form action={revokeShare.bind(null, slug, l.id)}>
                                <button type="submit" className={buttonClass("danger")}>
                                  Revoke
                                </button>
                              </form>
                            ) : (
                              <span className="text-small">{l.revokedAt ? "Revoked" : "Expired"}</span>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </Table>
                )}
                <p className="m-0 text-small text-muted">
                  A link can’t be shown again. If it’s lost, revoke it and create a new one; the old link
                  stops working immediately.
                </p>
              </>
            ) : (
              <p className="m-0 text-muted">You can’t manage share links.</p>
            )}
          </Card>
          <Card
            id="secrets"
            title="Run secrets"
            aside="Passed to hub runs as environment variables only. Write-only: values are encrypted and never shown again, including to you. Replace or delete them here."
          >
            {canSecrets ? (
              <>
                <ul className="m-0 flex list-none flex-col p-0">
                  {secrets.map((x) => (
                    <li
                      key={x.name}
                      className="flex flex-wrap items-center gap-2.5 border-t border-panel py-2.5"
                    >
                      <code className="min-w-[180px] flex-1 break-all">{x.name}</code>
                      <span className="text-small text-muted">Set {fmtDate(x.updated_at)}</span>
                      <SecretForm slug={slug} name={x.name} />
                      <form action={deleteSecret.bind(null, slug, x.name)}>
                        <button type="submit" className={buttonClass("danger")}>
                          Delete
                        </button>
                      </form>
                    </li>
                  ))}
                </ul>
                <SecretForm slug={slug} />
              </>
            ) : (
              <p className="m-0 text-muted">You can’t manage run secrets.</p>
            )}
          </Card>
          <Card id="retention" title="Retention">
            <p className="m-0 text-muted">Coming soon.</p>
          </Card>
        </div>
      </div>
    </ProjectFrame>
  );
}
