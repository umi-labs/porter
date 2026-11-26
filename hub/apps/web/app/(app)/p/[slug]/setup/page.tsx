import { getDb } from "@porter-hub/db";
import { ProjectFrame } from "@/components/porter/app-frame";
import { PageTitle } from "@/components/porter/frame";
import { requireUser } from "@/lib/session";
import { getSetup } from "@/lib/setup";
import { orStatus, Section } from "@/lib/view-parts";
import { SetupForm } from "./setup-form";

export const dynamic = "force-dynamic";

export default async function SetupPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const user = await requireUser();
  const s = await orStatus(getSetup(getDb(), user, slug));
  return (
    <ProjectFrame project={s.project} role={s.role} user={user} active="Setup">
      <PageTitle
        title="Setup"
        sub={
          s.version
            ? `Saving writes porter.toml as a new version on top of v${s.version}. Mapping and tidy rules are kept.`
            : "Saving writes the project’s porter.toml as its first version, ready to run from the overview."
        }
      />
      <Section title="Source and destination">
        <SetupForm
          slug={slug}
          current={s.current}
          uploads={s.uploads}
          hasSanityToken={s.hasSanityToken}
          hasPayloadKey={s.hasPayloadKey}
          hasWpLogin={s.hasWpLogin}
        />
      </Section>
    </ProjectFrame>
  );
}
