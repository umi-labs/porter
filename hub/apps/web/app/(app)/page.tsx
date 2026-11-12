import { getDb } from "@porter-hub/db";
import Link from "next/link";
import { listProjects } from "../../lib/projects";
import { requireUser } from "../../lib/session";

export const dynamic = "force-dynamic";

export default async function Home() {
  const user = await requireUser();
  const projects = await listProjects(getDb(), user);
  return (
    <main>
      <h1>Projects</h1>
      {projects.length === 0 ? (
        <p>You are not on any projects yet.</p>
      ) : (
        <ul>
          {projects.map((p) => (
            <li key={p.id}>
              <strong>{p.name}</strong> ({p.role}){p.summary ? ` - ${p.summary}` : ""}
              {p.role === "owner" && (
                <>
                  {" "}
                  <Link href={`/p/${p.slug}/settings`}>Settings</Link>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}
