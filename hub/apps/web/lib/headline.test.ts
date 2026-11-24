import { expect, test } from "vitest";
import { projectHeadline } from "./headline";

const views = (o: Record<string, { state: string; stale?: string | null }>) => ({
  stage_views: Object.fromEntries(
    ["analyse", "extract", "normalise", "schema", "load", "verify"].map((s) => [
      s,
      o[s] ?? { state: "not-run", stale: null },
    ]),
  ),
});

test("no stored views (older porter or no push): no pill, never a guess", () => {
  expect(projectHeadline(null)).toBeNull();
  expect(projectHeadline({ stages: [] })).toBeNull();
});

test("open errors win, counted", () => {
  const s = {
    ...views({ load: { state: "dry-run" } }),
    open_issues: { error: 52, warning: 3 },
    ever_written: false,
  };
  expect(projectHeadline(s)).toEqual({ state: "errors", label: "52 errors" });
  expect(projectHeadline({ ...s, open_issues: { error: 1 } })).toEqual({ state: "errors", label: "1 error" });
});

test("an interrupted stage beats dry run", () => {
  const s = {
    ...views({ extract: { state: "interrupted" }, load: { state: "dry-run" } }),
    open_issues: { error: 0 },
  };
  expect(projectHeadline(s)).toEqual({ state: "interrupted", label: "Interrupted" });
});

test("dry run with nothing ever written", () => {
  const s = { ...views({ load: { state: "dry-run" } }), open_issues: { error: 0 }, ever_written: false };
  expect(projectHeadline(s)).toEqual({ state: "dry-run", label: "Dry run only" });
});

test("green only once written: verified, or loaded and not yet verified", () => {
  const loaded = { load: { state: "clean" } };
  expect(
    projectHeadline({
      ...views({ ...loaded, verify: { state: "clean" } }),
      ever_written: true,
      open_issues: { error: 0 },
    }),
  ).toEqual({ state: "clean", label: "Loaded · verified" });
  expect(projectHeadline({ ...views(loaded), ever_written: true, open_issues: { error: 0 } })).toEqual({
    state: "clean",
    label: "Loaded · not verified",
  });
  // A stale verify doesn't count as checked.
  expect(
    projectHeadline({
      ...views({ ...loaded, verify: { state: "clean", stale: "load re-ran" } }),
      ever_written: true,
      open_issues: { error: 0 },
    }),
  ).toEqual({ state: "clean", label: "Loaded · not verified" });
});

test("otherwise the furthest stage reached, in that stage's own state", () => {
  const s = {
    ...views({ analyse: { state: "clean" }, extract: { state: "warnings" } }),
    open_issues: { error: 0 },
  };
  expect(projectHeadline(s)).toEqual({ state: "warnings", label: "Up to extract" });
  // A stale furthest stage reads as not run: its result no longer stands.
  const stale = {
    ...views({ analyse: { state: "clean", stale: "porter.toml changed" } }),
    open_issues: { error: 0 },
  };
  expect(projectHeadline(stale)).toEqual({ state: "not-run", label: "Up to analyse (stale)" });
  expect(projectHeadline({ ...views({}), open_issues: { error: 0 } })).toEqual({
    state: "not-run",
    label: "Not run",
  });
});
