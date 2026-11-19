import { forbidden, notFound } from "next/navigation";
import type { ReactNode } from "react";
import { Banner, type Tone } from "../components/ui/banner";
import { buttonClass } from "../components/ui/button";
import { Card } from "../components/ui/card";
import { Chip, Stale, type State } from "../components/ui/chip";
import { Frame, type Tab } from "../components/ui/frame";
import { Resolution, SeverityTag } from "../components/ui/severity";
import { Table } from "../components/ui/table";
import { fmtDateTime, fmtNum } from "./format";
import { type Role, roleCan } from "./permissions";
import { HttpError } from "./projects";
import type { Page, Snapshot } from "./snapshot-reader";
import { issuesPage, listReports, overview, urlsPage } from "./snapshot-reader";
import type { StageView } from "./stage-state";

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

export const NA = <em className="text-muted not-italic">Not available in this version.</em>;

export function Empty({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-start gap-2 rounded-lg border border-border bg-bg p-6">
      <span className="text-heading font-semibold">{title}</span>
      {children && <span className="text-muted">{children}</span>}
    </div>
  );
}

export function NoSnapshot({ plain }: { plain?: boolean }) {
  return plain ? (
    <Empty title="Nothing to show yet">
      Your Umi team hasn’t shared results for this project yet. You can already send us your site files under
      Uploads.
    </Empty>
  ) : (
    <Empty title="Nothing has been pushed to this project yet">
      It appears here when someone pushes it from porter (<code>porter hub push</code>).
    </Empty>
  );
}

export function Pager({ base, p }: { base: string; p: Page<unknown> }) {
  return (
    <nav aria-label="Pages" className="flex flex-wrap items-center gap-3 text-[14px] text-muted">
      <span>
        Page {p.page} of {p.pages} ({fmtNum(p.total)} total)
      </span>
      {p.page > 1 && (
        <a className={buttonClass("secondary")} href={`${base}?page=${p.page - 1}`}>
          Previous
        </a>
      )}
      {p.page < p.pages && (
        <a className={buttonClass("secondary")} href={`${base}?page=${p.page + 1}`}>
          Next
        </a>
      )}
    </nav>
  );
}

/** The project's tabs for this role. Clients get Overview and Uploads only. */
export function projectTabs(slug: string, role: Role, active: string): Tab[] {
  const tabs: [string, string, boolean][] = [
    ["Overview", "", true],
    ["Issues", "/issues", roleCan(role, "view_raw")],
    ["URLs", "/urls", roleCan(role, "view_raw")],
    ["Uploads", "/uploads", roleCan(role, "view")],
    [
      "Settings",
      "/settings",
      roleCan(role, "share") || roleCan(role, "manage_members") || roleCan(role, "manage_secrets"),
    ],
  ];
  return tabs
    .filter(([, , ok]) => ok)
    .map(([label, path]) => ({ label, href: `/p/${slug}${path}`, active: label === active }));
}

/** A signed-in project page: header, the role's tabs, then the content. */
export function ProjectFrame({
  project,
  role,
  user,
  active,
  children,
}: {
  project: { name: string; slug: string };
  role: Role;
  user: { name: string; email: string };
  active: string;
  children: ReactNode;
}) {
  return (
    <Frame
      crumbs={[{ label: "Projects", href: "/" }, { label: project.name }]}
      who={`${user.name || user.email} · ${role}`}
      tabs={projectTabs(project.slug, role, active)}
      narrow={role === "client"}
    >
      {children}
    </Frame>
  );
}

const NAMES: Record<string, string> = {
  analyse: "Analyse",
  extract: "Extract",
  normalise: "Normalise",
  schema: "Schema",
  load: "Load",
  verify: "Verify",
};

/** Chip label and line under it. `plain` is client copy: no porter terms. */
function stageText(v: StageView, plain: boolean): { label?: string; sub: string } {
  const s = v.run?.summary;
  const done = fmtNum((s?.done ?? 0) + (s?.partial ?? 0));
  switch (v.state) {
    case null:
      if (v.unknown)
        return { sub: plain ? "Status not available here yet" : `unknown (newer porter): ${v.unknown}` };
      return { sub: "Finished. This version doesn’t record whether it was a dry run." };
    case "not-run":
      return { label: plain ? "Not started" : undefined, sub: plain ? "Not started yet" : "Never run" };
    case "skipped":
      return {
        label: plain ? "Not yet" : undefined,
        sub: plain ? "Waits for the real move" : "Waits for a real load",
      };
    case "dry-run":
      return {
        label: plain ? "Rehearsed" : "Dry run · not written",
        sub: plain ? "Rehearsed, nothing moved yet" : `Would write ${done}`,
      };
    case "interrupted":
      return {
        label: plain ? "Paused" : undefined,
        sub: plain ? "Stopped part-way" : `${done} done before it stopped`,
      };
    case "errors": {
      const n = (s?.errors ?? 0) + (s?.failed ?? 0);
      // A dry run's counts are what it would write: never "done".
      const did = v.run?.dryRun
        ? plain
          ? "Rehearsed, nothing moved yet"
          : `Would write ${done} · nothing written`
        : `${done} done`;
      return {
        label: plain ? `${fmtNum(n)} to fix` : `${fmtNum(n)} error${n === 1 ? "" : "s"}`,
        sub: plain ? did : `${did}${s?.failed ? ` · ${fmtNum(s.failed)} failed` : ""}`,
      };
    }
    case "warnings":
      return { label: plain ? "Done · to review" : "Done · warnings", sub: `${done} done` };
    case "clean":
      return { label: plain ? "Done" : "Clean · written", sub: `${done} done` };
    default:
      return { sub: "" };
  }
}

type Writes = { everWritten: boolean; lastReal: State | null } | null;

/**
 * The site-level claim. It comes from every load on record, not the last row: a dry run after an apply doesn't
 * unwrite the site, and an apply that stopped or failed part-way has not "moved" anything cleanly.
 */
function statusBanner(views: StageView[], writes: Writes, errors: number, n: number, plain: boolean) {
  const load = views.find((v) => v.stage === "load");
  const verify = views.find((v) => v.stage === "verify");
  const dry = load?.run?.dryRun === true;
  const real = writes?.lastReal ?? null; // the latest load that was not a dry run
  const unfinished = real === "interrupted" || real === "errors";
  const written = !!writes?.everWritten && (real === "clean" || real === "warnings");
  // Verify ran after the last real load (a real load since would make it stale) and finished without errors.
  const checked = (verify?.state === "clean" || verify?.state === "warnings") && !verify.stale;
  const things = `${fmtNum(errors)} ${plain ? "thing" : "open error"}${errors === 1 ? "" : "s"}`;
  let tone: Tone;
  let title: string;
  let body: string;
  if (plain) {
    if (unfinished) {
      tone = "err";
      title = "Moving your content to the new site didn’t finish.";
      body = `Some of it may already be there. Your Umi team is on it.${errors ? ` ${things} need fixing.` : ""}`;
    } else if (written && dry) {
      tone = errors ? "err" : "info";
      title = "Your content is on the new site. We’re rehearsing the next update.";
      body = errors ? `${things} need fixing before we move it for real.` : "";
    } else if (!written) {
      tone = errors ? "err" : "info";
      title = "In progress. Nothing has moved to your new site yet.";
      body = `${dry ? "We’ve copied and converted your content and rehearsed the move." : "We’re copying and converting your content."}${errors ? ` ${things} need fixing before we move it for real.` : ""}`;
    } else if (checked && !errors) {
      tone = "ok";
      title = "Your content is on the new site and has been checked.";
      body = "";
    } else {
      tone = errors ? "err" : "info";
      title = "Your content has moved to the new site.";
      body = errors ? `${things} still need fixing.` : "We’re checking it now.";
    }
  } else if (unfinished) {
    tone = "err";
    title = "An apply did not finish.";
    body = `The last load that wrote to the site ${real === "interrupted" ? "stopped part-way" : "ended with errors"}, so the new site may be partly written.${dry ? " A dry run has run since." : ""}${errors ? ` ${things} in v${n}.` : ""}`;
  } else if (written && dry) {
    tone = errors ? "err" : "dry";
    title = "Last load was a dry run; an earlier load wrote to the site.";
    body = errors ? `${things} in v${n}.` : `No open errors in v${n}.`;
  } else if (errors) {
    tone = "err";
    title = written
      ? "Loaded, with open errors."
      : "Not ready to apply. Nothing has been written to the new site.";
    body = `${things} in v${n}.${dry && verify?.state === "skipped" ? " Load has only been dry-run, so verify hasn’t run either." : ""}`;
  } else if (!written) {
    tone = dry ? "dry" : "info";
    title = "Nothing has been written to the new site.";
    body = dry ? `Load ran as a dry run with no open errors in v${n}.` : `Load hasn’t run in v${n}.`;
  } else if (checked) {
    tone = verify?.state === "clean" ? "ok" : "warn";
    title = verify?.state === "clean" ? "Loaded and verified." : "Loaded and verified, with warnings.";
    body = `No open errors in v${n}.`;
  } else {
    tone = "info";
    title = "Loaded, not yet verified.";
    body = verify?.stale ? `Verify is stale: ${verify.stale}.` : "Verify hasn’t run since the load.";
  }
  return (
    <Banner tone={tone} title={title}>
      {body || undefined}
    </Banner>
  );
}

const humanReport = (r: string) => {
  const base = r.replace(/\.(html|json)$/, "").replace(/[-_]+/g, " ");
  return base.charAt(0).toUpperCase() + base.slice(1);
};

/**
 * Pipeline state, latest version, issue counts, coverage and reports. `reportBase` is where report names link to.
 * `audience: "client"` gets plain-English copy and no per-kind breakdown; nobody gets issue messages here.
 */
export function Overview({
  snap,
  reportBase,
  audience,
  issuesHref,
}: {
  snap: Snapshot;
  reportBase: string;
  audience: "staff" | "client" | "share";
  issuesHref?: string;
}) {
  const plain = audience === "client";
  const o = overview(snap.dir, snap.version.summary);
  const reports = listReports(snap.dir);
  const n = snap.version.number;
  const sev = (s: string) =>
    (o.issueCounts ?? []).filter((c) => c.severity === s).reduce((a, c) => a + c.n, 0);
  const kinds = new Map<string, { error: number; warning: number; info: number }>();
  for (const c of o.issueCounts ?? []) {
    const k = kinds.get(c.kind) ?? { error: 0, warning: 0, info: 0 };
    if (c.severity in k) k[c.severity as keyof typeof k] += c.n;
    kinds.set(c.kind, k);
  }
  return (
    <>
      <div className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-bg px-4 py-3 text-[14px]">
        <span className="inline-flex h-6 items-center rounded-sm bg-ink px-2 text-small font-semibold text-bg">
          v{n}
        </span>
        <span>
          Pushed {fmtDateTime(snap.version.createdAt)}
          {!plain && snap.version.porterVersion ? ` from porter ${snap.version.porterVersion}` : ""}
        </span>
        {/* Push messages are internal notes: staff only, never on a share link. */}
        {audience === "staff" && snap.version.message && (
          <span className="text-muted">“{snap.version.message}”</span>
        )}
        <span className="flex-1" />
        <span className="text-small text-muted">
          This page shows v{n} as pushed. It doesn’t update until someone pushes again.
        </span>
      </div>

      {o.views && statusBanner(o.views, o.writes, sev("error"), n, plain)}

      <Card title="Pipeline" aside="Six stages, left to right. Only a green stage has written its result.">
        {o.views === null ? (
          <p className="m-0">{NA}</p>
        ) : (
          <ol className="m-0 grid list-none grid-cols-[repeat(auto-fit,minmax(160px,1fr))] gap-2.5 p-0">
            {o.views.map((v, i) => {
              const t = stageText(v, plain);
              return (
                <li
                  key={v.stage}
                  className="flex min-w-0 flex-col gap-2.5 rounded-lg border border-border p-3.5"
                  data-stage={v.stage}
                >
                  <span className="text-small text-muted">
                    {i + 1} · <b className="font-semibold text-ink">{NAMES[v.stage]}</b>
                  </span>
                  <Stale reason={v.stale} plain={plain}>
                    {v.state && <Chip state={v.state as State} label={t.label} />}
                    <span className="text-small text-muted">{t.sub}</span>
                  </Stale>
                </li>
              );
            })}
          </ol>
        )}
      </Card>

      <div className="flex flex-wrap gap-5">
        <Card title={plain ? "What’s been found" : "Coverage"} className="flex-[1_1_420px]">
          {o.coverage === null ? (
            <p className="m-0">{NA}</p>
          ) : (
            <div className="flex flex-col gap-1.5">
              <div className="flex gap-2 text-[14px]">
                <span className="flex-1 font-medium">{plain ? "Old web addresses" : "URLs"}</span>
                <span>{fmtNum(o.coverage.mapped)}</span>
                <span className="text-muted">of {fmtNum(o.coverage.routes)} mapped</span>
              </div>
              <div className="flex h-2 overflow-hidden rounded-xs bg-panel">
                <div
                  className="bg-ink/80"
                  style={{
                    width: `${o.coverage.routes ? (o.coverage.mapped / o.coverage.routes) * 100 : 0}%`,
                  }}
                />
              </div>
              <span className="text-small text-muted">
                {o.coverage.routes - o.coverage.mapped === 0
                  ? plain
                    ? "Every old address leads somewhere."
                    : "Every URL maps to migrated content."
                  : plain
                    ? `${fmtNum(o.coverage.routes - o.coverage.mapped)} old links don’t lead anywhere yet.`
                    : `${fmtNum(o.coverage.routes - o.coverage.mapped)} URLs don’t map to migrated content.`}
              </span>
            </div>
          )}
        </Card>

        <Card
          title="Issues"
          className="flex-[1_1_360px]"
          aside={!plain && issuesHref ? <a href={issuesHref}>Open issues table</a> : undefined}
        >
          {o.issueCounts === null ? (
            <p className="m-0">{NA}</p>
          ) : (
            <>
              <div className="grid grid-cols-3 gap-2.5">
                {(
                  [
                    ["error", plain ? "to fix" : "errors", "bg-err-bg text-err"],
                    ["warning", plain ? "to review" : "warnings", "bg-warn-bg text-warn"],
                    ["info", "info", "bg-panel text-muted"],
                  ] as const
                ).map(([s, label, cls]) => (
                  <div key={s} className={`flex flex-col rounded-md p-3 ${cls}`}>
                    <span className="text-[24px] leading-8 font-semibold">{fmtNum(sev(s))}</span>
                    <span className="text-small">{label}</span>
                  </div>
                ))}
              </div>
              {plain ? (
                <span className="text-[14px] text-ink/80">
                  Your Umi team is working through these. Details stay with the team; you’ll see the counts
                  drop as they’re fixed. Anything that needs your help will show up under Uploads.
                </span>
              ) : kinds.size === 0 ? (
                <span className="text-[14px] text-muted">No open issues.</span>
              ) : (
                <ul className="m-0 flex list-none flex-col p-0 text-[14px]">
                  {[...kinds].map(([kind, k]) => (
                    <li key={kind} className="flex gap-2 border-t border-panel py-1.5">
                      <span className="min-w-0 flex-1 truncate font-mono text-small">{kind}</span>
                      <span
                        className={`font-semibold ${k.error ? "text-err" : k.warning ? "text-warn" : "text-muted"}`}
                      >
                        {[
                          k.error && `${fmtNum(k.error)} error${k.error === 1 ? "" : "s"}`,
                          k.warning && `${fmtNum(k.warning)} warning${k.warning === 1 ? "" : "s"}`,
                          k.info && `${fmtNum(k.info)} info`,
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </Card>
      </div>

      <Card title={`Reports from v${n}`}>
        {reports.length === 0 ? (
          <p className="m-0 text-muted">No reports in this version.</p>
        ) : (
          <>
            <div className="grid grid-cols-[repeat(auto-fit,minmax(240px,1fr))] gap-2.5">
              {reports.map((r) => (
                <a
                  key={r}
                  href={`${reportBase}/${encodeURIComponent(r)}`}
                  className="flex min-h-11 flex-col gap-0.5 rounded-lg border border-border p-3.5 text-ink no-underline hover:border-border-strong hover:no-underline"
                >
                  <span className="text-[14px] font-semibold">{humanReport(r)}</span>
                  <span className="truncate text-small text-muted">
                    {r} · v{n}
                  </span>
                </a>
              ))}
            </div>
            {reports
              .filter((r) => r.endsWith(".html"))
              .map((r) => (
                <figure key={r} className="m-0 overflow-hidden rounded-lg border border-border">
                  <figcaption className="flex flex-wrap items-center gap-3 border-b border-border bg-surface px-3.5 py-2 text-small">
                    <span className="font-semibold">
                      {r} · v{n}
                    </span>
                    <span className="text-muted">Sandboxed: no scripts, no network</span>
                  </figcaption>
                  <iframe
                    title={r}
                    sandbox=""
                    src={`${reportBase}/${encodeURIComponent(r)}`}
                    className="block h-[480px] w-full border-0 bg-white"
                  />
                </figure>
              ))}
          </>
        )}
      </Card>
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
    <Card
      title={full ? `Issues in v${snap.version.number}` : `Open issues in v${snap.version.number}`}
      aside={<Pager base={base} p={p} />}
    >
      {p.total === 0 ? (
        <p className="m-0 text-muted">No issues in this version.</p>
      ) : (
        <Table
          head={[
            "Kind",
            "Severity",
            ...(full ? ["Message"] : []),
            "Item",
            ...(full ? ["Stage", "Resolution"] : []),
          ]}
        >
          {p.rows.map((r, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: rows are a static page
            <tr key={i}>
              <td className="font-mono text-small">{r.kind}</td>
              <td>
                <SeverityTag severity={r.severity} />
              </td>
              {full && <td className="min-w-[240px] break-words">{"message" in r ? r.message : ""}</td>}
              <td className="break-words">{r.itemTitle ?? ""}</td>
              {full && <td>{"stage" in r ? r.stage : ""}</td>}
              {full && <td>{"resolution" in r ? <Resolution value={r.resolution ?? ""} /> : ""}</td>}
            </tr>
          ))}
        </Table>
      )}
    </Card>
  );
}

export function UrlsView({ snap, page, base }: { snap: Snapshot; page: number; base: string }) {
  const p = urlsPage(snap.dir, page);
  if (!p) return <p>{NA}</p>;
  return (
    <Card title={`URLs in v${snap.version.number}`} aside={<Pager base={base} p={p} />}>
      {p.total === 0 ? (
        <p className="m-0 text-muted">No URLs in this version.</p>
      ) : (
        <Table head={["URL", "Old status", "Maps to"]}>
          {p.rows.map((r, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: rows are a static page
            <tr key={i}>
              <td className="max-w-[480px] font-mono text-small break-all">{r.url}</td>
              <td>{r.status ?? ""}</td>
              <td className={r.target === "none" ? "text-err" : ""}>{r.target}</td>
            </tr>
          ))}
        </Table>
      )}
    </Card>
  );
}
