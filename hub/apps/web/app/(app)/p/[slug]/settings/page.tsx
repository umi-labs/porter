import { getDb } from "@porter-hub/db";
import { notFound } from "next/navigation";
import { PageTitle } from "@/components/porter/frame";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Separator } from "@/components/ui/separator";
import { TableCell, TableRow } from "@/components/ui/table";
import { fmtDate } from "@/lib/format";
import { roleCan } from "@/lib/permissions";
import { access, HttpError, listInvitations, listMembers } from "@/lib/projects";
import { listSecrets } from "@/lib/secrets";
import { requireUser } from "@/lib/session";
import { listShares } from "@/lib/shares";
import { DataTable, ProjectFrame, Section } from "@/lib/view-parts";
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
  const row = "flex flex-wrap items-center gap-2.5 border-t py-2.5 first:border-t-0";
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
                className="flex min-h-11 items-center rounded-md px-3 text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              >
                {label}
              </a>
            ))}
        </nav>
        <div className="flex min-w-0 flex-[999_1_640px] flex-col gap-5">
          {canMembers && (
            <Section id="members" title="Members and roles">
              <ul className="m-0 flex list-none flex-col p-0">
                {members.map((m) => (
                  <li key={m.userId} className={row}>
                    <span className="min-w-[180px] flex-1 break-words">
                      {m.name} <span className="text-muted-foreground">&lt;{m.email}&gt;</span>
                    </span>
                    <form action={setRole.bind(null, slug, m.userId)} className="flex items-center gap-2">
                      <NativeSelect name="role" defaultValue={m.role} aria-label={`Role for ${m.email}`}>
                        {ROLES.map((r) => (
                          <NativeSelectOption key={r}>{r}</NativeSelectOption>
                        ))}
                      </NativeSelect>
                      <Button type="submit" variant="outline" className="press">
                        Change role
                      </Button>
                    </form>
                    <form action={removeMember.bind(null, slug, m.userId)}>
                      <Button type="submit" variant="destructive" className="press">
                        Remove
                      </Button>
                    </form>
                  </li>
                ))}
              </ul>
              <Separator />
              <h3 className="m-0 text-sm font-semibold">Invitations</h3>
              {invitations.length === 0 ? (
                <p className="m-0 text-xs text-muted-foreground">No open invitations.</p>
              ) : (
                <ul className="m-0 flex list-none flex-col p-0">
                  {invitations.map((i) => (
                    <li key={i.id} className={row}>
                      <span className="min-w-[180px] flex-1 break-words">
                        {i.email}{" "}
                        <span className="text-muted-foreground">
                          ({i.role}), expires {fmtDate(i.expiresAt)}
                        </span>
                      </span>
                      <form action={revoke.bind(null, slug, i.id)}>
                        <Button type="submit" variant="destructive" className="press">
                          Revoke
                        </Button>
                      </form>
                    </li>
                  ))}
                </ul>
              )}
              <form action={invite.bind(null, slug)} className="flex flex-wrap items-end gap-2">
                <div className="flex min-w-0 flex-[1_1_240px] flex-col gap-1.5">
                  <Label htmlFor="invite-email">Email</Label>
                  <Input id="invite-email" name="email" type="email" required />
                </div>
                <div className="flex flex-col gap-1.5">
                  <Label htmlFor="invite-role">Role</Label>
                  <NativeSelect id="invite-role" name="role" defaultValue="client">
                    {ROLES.map((r) => (
                      <NativeSelectOption key={r}>{r}</NativeSelectOption>
                    ))}
                  </NativeSelect>
                </div>
                <Button type="submit" className="press">
                  Invite
                </Button>
              </form>
            </Section>
          )}
          <Section
            id="shares"
            title="Share links"
            aside="Let someone without an account see this project. Every link expires, and you can revoke it at any time."
          >
            {canShare ? (
              <>
                <ShareForm slug={slug} />
                {shares.length > 0 && (
                  <DataTable head={["Fingerprint", "Scope", "Created", "Expires", ""]}>
                    {shares.map((l) => {
                      const active = !l.revokedAt && l.expiresAt >= now;
                      return (
                        <TableRow key={l.id} className={active ? "" : "text-muted-foreground"}>
                          <TableCell className={`font-mono text-xs ${l.revokedAt ? "line-through" : ""}`}>
                            …/s/{l.fingerprint}···
                          </TableCell>
                          <TableCell>{l.scope === "report" ? "Reports only" : "Read-only project"}</TableCell>
                          <TableCell>{fmtDate(l.createdAt)}</TableCell>
                          <TableCell>{fmtDate(l.expiresAt)}</TableCell>
                          <TableCell className="text-right">
                            {active ? (
                              <form action={revokeShare.bind(null, slug, l.id)}>
                                <Button type="submit" variant="destructive" className="press">
                                  Revoke
                                </Button>
                              </form>
                            ) : (
                              <span className="text-xs">{l.revokedAt ? "Revoked" : "Expired"}</span>
                            )}
                          </TableCell>
                        </TableRow>
                      );
                    })}
                  </DataTable>
                )}
                <p className="m-0 text-xs text-muted-foreground">
                  A link can’t be shown again. If it’s lost, revoke it and create a new one; the old link
                  stops working immediately.
                </p>
              </>
            ) : (
              <p className="m-0 text-muted-foreground">You can’t manage share links.</p>
            )}
          </Section>
          <Section
            id="secrets"
            title="Run secrets"
            aside="Passed to hub runs as environment variables only. Write-only: values are encrypted and never shown again, including to you. Replace or delete them here."
          >
            {canSecrets ? (
              <>
                <ul className="m-0 flex list-none flex-col p-0">
                  {secrets.map((x) => (
                    <li key={x.name} className={row}>
                      <code className="min-w-[180px] flex-1 break-all text-xs">{x.name}</code>
                      <span className="text-xs text-muted-foreground">Set {fmtDate(x.updated_at)}</span>
                      <SecretForm slug={slug} name={x.name} />
                      <form action={deleteSecret.bind(null, slug, x.name)}>
                        <Button type="submit" variant="destructive" className="press">
                          Delete
                        </Button>
                      </form>
                    </li>
                  ))}
                </ul>
                {secrets.length > 0 && <Separator />}
                <SecretForm slug={slug} />
              </>
            ) : (
              <p className="m-0 text-muted-foreground">You can’t manage run secrets.</p>
            )}
          </Section>
          <Section id="retention" title="Retention">
            <p className="m-0 text-muted-foreground">Coming soon.</p>
          </Section>
        </div>
      </div>
    </ProjectFrame>
  );
}
