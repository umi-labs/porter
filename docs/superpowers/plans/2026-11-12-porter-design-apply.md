# Apply the Porter design: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Restyle the desktop app and the hub to the approved design in `docs/design/`, and add the behaviour the design adds to Apply and to the stage states.

**Architecture:**
- Tokens are defined once in `docs/design/tokens/tokens.json`.
  - The desktop app gets a `theme` module, ported from `tokens/theme.rs`, and a small set of shared components: state chip, severity mark, path label, buttons.
  - The hub gets Tailwind v4 with `tokens/tailwind-theme.css` and matching React components.
- The views keep their view models. Only rendering changes, except for the Apply gates and the copy below.

**Spec:** `docs/design/README.md` (the rules, the stage-state table, Apply, and the desktop and hub notes) plus the boards in `docs/design/canvas/*.dc.html`.
- The boards are HTML with `{{t.*}}` token placeholders. Read them for layout, copy and states.
- Sample data in the boards is invented.

## Global Constraints

- **Green means written.** Only `clean` uses `ok`. A dry run is never green.
- **Every state is a glyph plus a word.** Never colour alone.
- **Desktop:**
  - System font, 13px body, 28px table rows, 32px sidebar rows.
  - No animation.
  - Light and dark themes from the same token names.
  - Long paths truncate in the middle and keep the last two segments; the full path shows in a tooltip.
- **Hub:**
  - Inter, 15px body, 10px card radii, 44px minimum touch targets.
  - Clients see only Overview, Reports and Uploads, written in plain-English copy.
  - Share views carry a permanent "Shared view · read-only" bar.
- **Apply is never the primary action.** It is a dashed ghost button with a lock. *Load (dry run)* is primary.
- **No behaviour regressions.** Existing tests stay green. GPUI stays out of the default build.

## Review Focus

1. **Stale is a modifier on the previous result, not a replacement.** The card keeps its old chip and counts at 62% opacity and shows a reason.
2. **Dry run vs clean vs skipped** must never share a shape or colour, in either theme.
3. **The Apply gates:**
   - a fresh dry run
   - no open errors
   - warnings acknowledged
   - the typed host, with paste disabled
   - Cancel has focus
   - Apply has no shortcut
4. **The client role in the hub** never sees issue details or raw data, after the restyle as before.
5. **Narrow windows and long content** (paths, names, counts) never overflow.

---

### Task D1: Desktop theme and shared components

**Files:**
- `crates/porter-desktop/src/theme.rs`, from `docs/design/tokens/theme.rs`, wired as a GPUI global with light and dark that follow the system appearance.
- `src/ui/{chip.rs,severity.rs,path_label.rs,buttons.rs}`.

**Steps:**
- **Step 1:** Write the failing view-model tests. They test the pure logic:
  - `stage_look(state) -> Look { glyph, word, tone, hatched, dashed, dimmed }` for every state in the README table
  - `middle_truncate(path, max_chars)` keeps the last two segments
- **Step 2:** Implement. The chip uses `pattern_slash` for dry run.
- **Step 3:** Restyle the launcher (board `Desktop-Launcher`) using it.
- **Step 4:** Run `cargo test -p porter-desktop` and clippy.
- **Step 5:** Commit: `feat(desktop): design tokens, theme and state components`.

### Task D2: Desktop overview, top bar and Apply sheet

**Files:** `views/overview.rs`, `project_window.rs`, `vm/apply.rs`, `vm/pipeline.rs`

**Behaviour** (boards `Desktop-Overview`, `Desktop-Apply`):
- The pipeline strip uses the chips. A stale stage shows as a modifier, with its reason.
- Load after apply shows "Loaded, not yet verified" until verify runs.
- Interrupted stages offer Resume.
- **The Apply sheet:**
  - It shows the three gates: fresh dry run, no open errors, warnings acknowledged (with a checkbox).
  - It lists exactly what will and will not be written.
  - A typed-host field with paste disabled.
  - A red *Apply N writes* button, enabled only when every gate passes.
  - Cancel has focus and Apply has no shortcut.
  - With a stale or missing dry run, the sheet shows *Run dry run now* in place of the field.

**Tests:**
- `apply_blocked_by_open_errors`
- `apply_requires_warning_ack`
- `loaded_not_verified_label`
- `stale_keeps_previous_counts`

**Commit:** `feat(desktop): overview and apply sheet to the design`.

### Task D3: Desktop mapping, URLs, the remaining sections and edge states

**Files:** `views/{mapping,urls,content,media,issues,destination,runs}.rs`, `sidebar.rs`

**Behaviour** (boards `Desktop-Mapping`, `Desktop-URLs`, `Desktop-States`):
- Invalid mapping rows say why, inline.
- Every count links to its rows.
- Empty, loading, error and long-content states for each section.

**Tests:** view-model tests for any new derivations. The rest is rendering.

**Commit:** `feat(desktop): sections and edge states to the design`.

### Task H1: Hub design system and pages

**Files:**
- `hub/apps/web`: Tailwind v4 set up with `tokens/tailwind-theme.css` and Inter.
- `components/ui/*`: chip, severity, card, button, table, banner.
- Every page under `app/`.

**Behaviour** (boards `Hub-Overview` with its role tweak, `Hub-Uploads`, `Hub-Share`, `Hub-Device`, `Hub-RunLog`, `Hub-States`):
- **Overview:** says which pushed version it shows and that it doesn't update live. Client copy is in plain English.
- **Share links:** the token is shown once with a copy button, then only a fingerprint.
- **The public share view:** a permanent shared-view bar showing the expiry.
- **Secrets:** write-only, with replace and delete.
- **Uploads:** states go checking → uploading → verifying → ready. A file over 20 GB is rejected before upload starts.

**Tests:**
- Existing tests stay green.
- New tests:
  - a component test that `clean` is the only state using the `ok` class
  - the client overview renders none of the issue messages
  - the share bar appears on every `/s/` page

**Commit:** `feat(hub): design system and pages to the design`.
