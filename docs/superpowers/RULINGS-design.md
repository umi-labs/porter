# Design application: rulings and backlog

These are taken from the build ledger on 2026-11-12. The design came from Isaac (`docs/design/`), and the plan is `docs/superpowers/plans/2026-11-12-porter-design-apply.md`.

## What changed beyond the visual design
- **Engine stage views.** Dry run is now a real state, skipped is derived, and stale is a modifier with a reason. The CLI, TUI, desktop and hub all show the same states. `porter status --json` gains `stage_views` and `ever_written`.
- **Config fingerprints (store schema v6).** Each run row records a hash of exactly the porter.toml sections that stage reads. Staleness and the dry-run gate compare those hashes, instead of the file's modified time.
  - The engine refuses `porter load --apply` (exit 1) when a dry run is on record and either its fingerprint no longer matches or an upstream stage has started since. `porter run --apply` is deliberately unguarded, and the README documents this.
  - After upgrading, every stage reads stale ("ran before settings were recorded") until it re-runs, and an older porter can't open a v6 project. This is in README and UPGRADING.
- **Hub apply.** It refuses "all stages", refuses when there are open errors, and refuses without a dry-run load. The confirm host must be typed.
- **Apply sheet (desktop).** Three gates: a fresh dry run, no open errors, and warnings acknowledged by issue id. The host must be typed with paste blocked. Cancel has focus, and Apply has no shortcut.

## Needs eyes (no display in the build environment)
- The dry-run hatch density, the dashed rings, and the ⚿ lock glyph.
- Contrast and glyph sizes in light and dark mode, and the 3px problem bar.
- The README smoke checklists in `crates/porter-desktop/README.md`.

## Rulings
- Ruling: adopt design's added Apply gates (no open errors, warnings ack, paste disabled, Cancel focus) as requirements — they tighten honesty — cost: one more click to apply.
- Ruling: desktop D1-D3 run now (hub plan T7 fix is in hub/); H1 runs after hub T7 completes, parallel to hub T8 (Rust CLI) — cost: none.
- Ruling D2: engine gains stage-view data: dry-run as a state, Skipped, and stale as modifier with reason (upstream stage newer, or porter.toml newer for normalise+); TUI must keep compiling and show dry run/stale honestly (minimal); Apply gates per design — cost if wrong: TUI glyph tweaks.
- Task H1: dispatched (base cf5f2a9, opus) after hub T7 complete. Ruling: hub states derived from snapshot runs (incl. dry_run) only; restyle must not weaken any permission/header/scope rule.
- Ruling: per-stage config fingerprint on run rows (migration v6), staleness + apply gate compare fingerprints and upstream run starts; NULL fp fails closed; seed mapping right after analyse — cost if wrong: existing projects show stale once after upgrade.
- Ruling: canonical key sort (arrays keep order); evidence-based per-stage sections; engine-level apply check vs dry-run fp for all front ends (when dry-run row exists).
- Task D3 fix round 1: d5e25f2. Ruling: stage-card chips must open rows matching their count -> add Issues stage filter (spec lists it). Sent as round 1b.
- Ruling: one fix wave after hub T9 fix lands (both touch hub). status --json emits state+stale from stage_view; versions store it (push + worker); hub shows stored views, NULL fp => stale; clients get dim + plain reason; hub rejects apply with "all" and with open errors; everWritten in overview; README/UPGRADING/DEPLOY notes.
- Ruling: F1 (hub fallback: non-null config_fp without stage_views must read stale "settings not checked on the hub") and F2 (createRun must refuse apply with no dry-run load row in base version) are honesty Importants — folded into the hub final fix wave (same files), with m1 (status --json v/event first), m2 (README ever_written wording), m3 (client copy "Needs re-running" not "Being redone"). Cost: none, single hub wave.

## Parked minors

- Per-task reviews in the design ledger list the parked minors. Left out of the boards because there's no data yet:
  - URL text filter and Loops chip
  - mapping fix suggestions
  - F8 shortcut
  - share-link labels and last used
  - per-type coverage
  - project status pills
  - upload pause
  - Run again and Download log
