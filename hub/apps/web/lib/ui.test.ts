import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { createElement as h } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterAll, describe, expect, test } from "vitest";
import { Chip, STATES } from "../components/ui/chip";
import { SharedFrame } from "../components/ui/share-bar";
import { type RunRow, stageViews } from "./stage-state";
import { Overview } from "./view-parts";

const OK = /(?<![\w-])(?:bg|text|border)-(?:on-)?ok(?:-bg)?(?![\w-])/;

test("clean is the only state that renders the ok class", () => {
  for (const s of STATES) {
    const html = renderToStaticMarkup(h(Chip, { state: s }));
    expect(OK.test(html), s).toBe(s === "clean");
    expect(html, `${s} has a word`).toMatch(/>[A-Z][a-z]+( [a-z]+)?</);
  }
});

describe("stage states from a snapshot's runs table", () => {
  const sum = (o: Partial<RunRow["summary"]> = {}) => ({
    done: 1,
    partial: 0,
    skipped: 0,
    failed: 0,
    errors: 0,
    warnings: 0,
    infos: 0,
    interrupted: false,
    ...o,
  });
  const row = (id: string, stage: string, o: Partial<RunRow> = {}): RunRow => ({
    id,
    stage,
    startedAt: "t",
    endedAt: "t",
    summary: sum(),
    dryRun: false,
    ...o,
  });
  const states = (rows: RunRow[]) => Object.fromEntries(stageViews(rows).map((v) => [v.stage, v.state]));

  test("dry-run load is never clean, and verify after it is skipped", () => {
    expect(states([row("1", "analyse"), row("2", "load", { dryRun: true })])).toMatchObject({
      analyse: "clean",
      extract: "not-run",
      load: "dry-run",
      verify: "skipped",
    });
  });
  test("errors beat dry run; no summary is interrupted; warnings", () => {
    expect(
      states([
        row("1", "load", { dryRun: true, summary: sum({ errors: 2 }) }),
        row("2", "extract", { summary: null, endedAt: null }),
        row("3", "analyse", { summary: sum({ warnings: 1 }) }),
      ]),
    ).toMatchObject({ load: "errors", extract: "interrupted", analyse: "warnings" });
  });
  test("an older snapshot without dry_run can't call a load clean", () => {
    expect(states([row("1", "load", { dryRun: null })]).load).toBeNull();
  });
  test("stale is a modifier: an upstream re-run keeps the state and gives the reason", () => {
    const v = stageViews([row("1", "normalise"), row("2", "analyse")]).find((x) => x.stage === "normalise");
    expect(v).toMatchObject({ state: "clean", stale: "analyse re-ran since" });
    // A dry-run load wrote nothing, so verify still stands.
    const verify = stageViews([row("1", "verify"), row("2", "load", { dryRun: true })]).find(
      (x) => x.stage === "verify",
    );
    expect(verify?.stale).toBeNull();
  });
});

describe("overview", () => {
  const dir = mkdtempSync(join(tmpdir(), "porter-ui-"));
  afterAll(() => rmSync(dir, { recursive: true, force: true }));
  const db = new DatabaseSync(join(dir, "project.db"));
  db.exec(`
    CREATE TABLE items (id TEXT PRIMARY KEY, kind TEXT NOT NULL, data TEXT NOT NULL, deleted_at TEXT);
    CREATE TABLE runs (id TEXT PRIMARY KEY, stage TEXT NOT NULL, started_at TEXT NOT NULL, ended_at TEXT, summary TEXT, dry_run INTEGER NOT NULL DEFAULT 0, deleted_at TEXT);
    CREATE TABLE issues (id TEXT PRIMARY KEY, kind TEXT NOT NULL, severity TEXT NOT NULL, item_id TEXT NOT NULL DEFAULT '', stage TEXT NOT NULL, message TEXT NOT NULL, resolution TEXT NOT NULL, deleted_at TEXT);
  `);
  const s = JSON.stringify({
    done: 4,
    partial: 0,
    skipped: 0,
    failed: 0,
    errors: 0,
    warnings: 0,
    infos: 0,
    interrupted: false,
  });
  db.prepare(
    "INSERT INTO runs (id,stage,started_at,ended_at,summary,dry_run) VALUES ('1','analyse','a','b',?,0)",
  ).run(s);
  db.prepare(
    "INSERT INTO runs (id,stage,started_at,ended_at,summary,dry_run) VALUES ('2','load','a','b',?,1)",
  ).run(s);
  const ins = db.prepare(
    "INSERT INTO issues (id,kind,severity,stage,message,resolution) VALUES (?,'missing-media',?,'extract',?,'open')",
  );
  ins.run("1", "error", "SECRET-MESSAGE-1 field value hunter2");
  ins.run("2", "warning", "SECRET-MESSAGE-2 /wp-content/private.jpg");
  db.close();
  const snap = {
    dir,
    version: {
      id: "v",
      number: 3,
      createdAt: new Date("2026-11-10T10:00:00Z"),
      message: "push msg",
      porterVersion: "2.0.0",
    },
  };
  const render = (audience: "staff" | "client" | "share") =>
    renderToStaticMarkup(h(Overview, { snap, reportBase: "/r", audience }));

  test("the client overview contains no issue messages, kinds or porter terms", () => {
    const html = render("client");
    expect(html).not.toContain("SECRET-MESSAGE");
    expect(html).not.toContain("hunter2");
    expect(html).not.toContain("missing-media");
    expect(html).toContain("to fix");
    expect(html).toContain("Rehearsed, nothing moved yet");
    expect(html).toContain("Nothing has moved to your new site yet");
  });
  test("every audience: says which version and that it doesn't update live; never a message", () => {
    for (const a of ["staff", "client", "share"] as const) {
      const html = render(a);
      expect(html).toContain("This page shows v3 as pushed. It doesn’t update until someone pushes again.");
      expect(html).not.toContain("SECRET-MESSAGE");
    }
    expect(render("staff")).toContain("missing-media");
  });
  test("a dry-run load is not green", () => {
    const html = render("staff");
    expect(html).toMatch(/data-state="dry-run"[^>]*>.*Dry run · not written/);
    expect(html.match(/data-state="clean"/g)).toHaveLength(1); // analyse only
  });
});

describe("share bar", () => {
  const appDir = join(import.meta.dirname, "../app");
  const pages = (d: string): string[] =>
    readdirSync(d, { withFileTypes: true }).flatMap((e) =>
      e.isDirectory()
        ? pages(join(d, e.name))
        : /^(page|not-found|error|loading)\.tsx$/.test(e.name)
          ? [join(d, e.name)]
          : [],
    );

  test("every /s/ page sits under the layout that renders the share bar", () => {
    const found = pages(join(appDir, "s")).map((p) => relative(join(appDir, "s"), p));
    expect(found.length).toBeGreaterThanOrEqual(4);
    for (const p of found) expect(p.startsWith("[token]/"), p).toBe(true);
    const layout = readFileSync(join(appDir, "s/[token]/layout.tsx"), "utf8");
    expect(layout).toMatch(/<SharedFrame[\s>]/);
  });
  test("the frame shows the bar with the expiry, and without a project name for a dead link", () => {
    const live = renderToStaticMarkup(
      h(
        SharedFrame,
        {
          expiresAt: new Date(Date.now() + 6.5 * 86_400_000),
          projectName: "Acme",
        },
        "x",
      ),
    );
    expect(live).toContain("Shared view · read-only");
    expect(live).toMatch(/expires in 7 days/);
    const dead = renderToStaticMarkup(h(SharedFrame, { expiresAt: null, projectName: null }, "x"));
    expect(dead).toContain("Shared view · read-only");
    expect(dead).not.toContain("Acme");
  });
});
