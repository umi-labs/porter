import { getDb } from "@porter-hub/db";
import { AppFrame } from "@/components/porter/app-frame";
import { PageTitle } from "@/components/porter/frame";
import { Badge } from "@/components/ui/badge";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { signInWays } from "@/lib/account";
import { fmtAgo } from "@/lib/format";
import { requireUser } from "@/lib/session";

export const dynamic = "force-dynamic";

export default async function AccountPage() {
  const user = await requireUser();
  const ways = await signInWays(getDb(), user.id);
  return (
    <AppFrame user={user} narrow>
      <PageTitle title="Account" sub={`${user.name} · ${user.email}`} />
      <section className="flex flex-col gap-3">
        <div>
          <h2 className="text-base font-semibold">Linked accounts</h2>
          <p className="text-sm text-muted-foreground">
            Ways you can sign in. To link one that isn’t linked, sign out and sign in with it using{" "}
            {user.email}.
          </p>
        </div>
        <ItemGroup className="animate-enter overflow-hidden rounded-lg bg-card ring-1 ring-foreground/10">
          {ways.map((w) => (
            <Item key={w.id} size="sm" className="rounded-none border-0 border-t px-4 py-3 first:border-t-0">
              <ItemContent className="min-w-0">
                <ItemTitle className="text-sm font-semibold">{w.label}</ItemTitle>
                {w.linkedAt && <ItemDescription>Linked {fmtAgo(w.linkedAt)}</ItemDescription>}
              </ItemContent>
              <ItemActions>
                {w.linkedAt ? (
                  <Badge variant="outline">Linked</Badge>
                ) : (
                  <Badge variant="outline" className="border-dashed text-muted-foreground">
                    Not linked
                  </Badge>
                )}
              </ItemActions>
            </Item>
          ))}
        </ItemGroup>
        <p className="text-xs text-muted-foreground">You can also sign in with a link sent to your email.</p>
      </section>
    </AppFrame>
  );
}
