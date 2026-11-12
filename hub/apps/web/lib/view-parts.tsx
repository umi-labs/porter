import { forbidden, notFound } from "next/navigation";
import type { ReactNode } from "react";
import { HttpError } from "./projects";
import type { Page, Snapshot } from "./snapshot-reader";
import { issuesPage, listReports, overview, urlsPage } from "./snapshot-reader";

/** Turns the service layer's 404/403 into Next's. */
export async function orStatus<T>(p: Promise<T>) {
  try {
    return await p;
  } catch (e) {
    if (e instanceof HttpError) {
      if (e.status === 403) forbidden();
      if (e.status === 404) notFound();
    }
    throw e;
  }
}

export const NA = <em>not available in this version</em>;

export function NoSnapshot() {
  return <p>Nothing has been pushed to this project yet.</p>;
}

export function Pager({ base, p }: { base: string; p: Page<unknown> }) {
  return (
    <p>
      Page {p.page} of {p.pages} ({p.total} total){" "}
      {p.page > 1 && <a href={`${base}?page=${p.page - 1}`}>Previous</a>}{" "}
      {p.page < p.pages && <a href={`${base}?page=${p.page + 1}`}>Next</a>}
    </p>
  );
}

/** Pipeline state, latest version, issue counts and coverage. `reportBase` is where report names link to. */
export function Overview({ snap, reportBase }: { snap: Snapshot; reportBase: string }) {
  const o = overview(snap.dir);
  const reports = listReports(snap.dir);
  return (
    <>
      <h2>Latest version</h2>
      <p>
        v{snap.version.number}, pushed {snap.version.createdAt.toISOString()}
        {snap.version.porterVersion ? ` with porter ${snap.version.porterVersion}` : ""}
        {snap.version.message ? `: ${snap.version.message}` : ""}
      </p>
      <h2>Pipeline</h2>
      {o.stages === null ? (
        <p>{NA}</p>
      ) : o.stages.length === 0 ? (
        <p>No stage has run yet.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Stage</th>
              <th>Last run started</th>
              <th>State</th>
            </tr>
          </thead>
          <tbody>
            {o.stages.map((s) => (
              <tr key={s.stage}>
                <td>{s.stage}</td>
                <td>{s.startedAt}</td>
                <td>
                  {s.endedAt ? `finished ${s.endedAt}` : "not finished"}
                  {s.dryRun ? " (dry run)" : ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h2>Open issues</h2>
      {o.issueCounts === null ? (
        <p>{NA}</p>
      ) : o.issueCounts.length === 0 ? (
        <p>None.</p>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Kind</th>
              <th>Severity</th>
              <th>Count</th>
            </tr>
          </thead>
          <tbody>
            {o.issueCounts.map((c) => (
              <tr key={`${c.kind}/${c.severity}`}>
                <td>{c.kind}</td>
                <td>{c.severity}</td>
                <td>{c.n}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h2>Coverage</h2>
      {o.coverage === null ? (
        <p>{NA}</p>
      ) : (
        <p>
          {o.coverage.mapped} of {o.coverage.routes} URLs map to migrated content.
        </p>
      )}
      <h2>Reports</h2>
      {reports.length === 0 ? (
        <p>No reports in this version.</p>
      ) : (
        <>
          <ul>
            {reports.map((r) => (
              <li key={r}>
                <a href={`${reportBase}/${encodeURIComponent(r)}`}>{r}</a>
              </li>
            ))}
          </ul>
          {reports
            .filter((r) => r.endsWith(".html"))
            .map((r) => (
              <iframe
                key={r}
                title={r}
                sandbox=""
                src={`${reportBase}/${encodeURIComponent(r)}`}
                style={{ width: "100%", height: 480, border: "1px solid #ccc", marginBottom: 12 }}
              />
            ))}
        </>
      )}
    </>
  );
}

export function IssuesView({
  snap,
  page,
  base,
  full,
}: {
  snap: Snapshot;
  page: number;
  base: string;
  full: boolean;
}) {
  const p = issuesPage(snap.dir, page, full);
  if (!p) return <p>{NA}</p>;
  return (
    <>
      <Pager base={base} p={p} />
      <table>
        <thead>
          <tr>
            <th>Kind</th>
            <th>Severity</th>
            <th>Message</th>
            <th>Item</th>
            {full && <th>Stage</th>}
            {full && <th>Resolution</th>}
          </tr>
        </thead>
        <tbody>
          {p.rows.map((r, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: rows are a static page
            <tr key={i}>
              <td>{r.kind}</td>
              <td>{r.severity}</td>
              <td>{r.message}</td>
              <td>{r.itemTitle ?? ""}</td>
              {full && <td>{"stage" in r ? r.stage : ""}</td>}
              {full && <td>{"resolution" in r ? r.resolution : ""}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

export function UrlsView({ snap, page, base }: { snap: Snapshot; page: number; base: string }) {
  const p = urlsPage(snap.dir, page);
  if (!p) return <p>{NA}</p>;
  return (
    <>
      <Pager base={base} p={p} />
      <table>
        <thead>
          <tr>
            <th>URL</th>
            <th>Old status</th>
            <th>Maps to</th>
          </tr>
        </thead>
        <tbody>
          {p.rows.map((r, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: rows are a static page
            <tr key={i}>
              <td>{r.url}</td>
              <td>{r.status ?? ""}</td>
              <td>{r.target}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

export const Shell = ({ title, children }: { title: string; children: ReactNode }) => (
  <main>
    <h1>{title}</h1>
    {children}
  </main>
);
