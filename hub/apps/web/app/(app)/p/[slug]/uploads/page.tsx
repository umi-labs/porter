import { getDb } from "@porter-hub/db";
import { Uploader } from "../../../../../components/uploader";
import { roleCan } from "../../../../../lib/permissions";
import { requireUser } from "../../../../../lib/session";
import { orStatus, Shell } from "../../../../../lib/view-parts";
import { loadProject } from "../../../../../lib/views";

export const dynamic = "force-dynamic";

export default async function Uploads({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const { role } = await orStatus(loadProject(getDb(), await requireUser(), slug, "view"));
  return (
    <Shell title={`${slug}: uploads`}>
      <Uploader slug={slug} canUpload={roleCan(role, "upload")} canDownload={roleCan(role, "pull")} />
    </Shell>
  );
}
