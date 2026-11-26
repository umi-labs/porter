import { getDb } from "@porter-hub/db";
import { ProjectFrame } from "@/components/porter/app-frame";
import { PageTitle } from "@/components/porter/frame";
import { Uploader } from "@/components/uploader";
import { roleCan } from "@/lib/permissions";
import { requireUser } from "@/lib/session";
import { maxBytes } from "@/lib/uploads";
import { orStatus } from "@/lib/view-parts";
import { loadProject } from "@/lib/views";

export const dynamic = "force-dynamic";

export default async function Uploads({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const user = await requireUser();
  const { project, role } = await orStatus(loadProject(getDb(), user, slug, "view"));
  const gb = Math.round(maxBytes() / 1024 ** 3);
  return (
    <ProjectFrame project={project} role={role} user={user} active="Uploads">
      <PageTitle
        title="Uploads"
        sub={`Send us your site’s files. Up to ${gb} GB each. If your connection drops, come back to this page, pick the same file, and it carries on from where it stopped.`}
      />
      <Uploader
        slug={slug}
        canUpload={roleCan(role, "upload")}
        canDownload={roleCan(role, "push")}
        maxBytes={maxBytes()}
      />
    </ProjectFrame>
  );
}
