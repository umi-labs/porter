# Porter design

The visual language for the desktop app (`crates/porter-desktop`, GPUI) and the hub (`hub/`, Next.js). The brief it answers is [claude-design-prompt.md](claude-design-prompt.md).

Live canvas: https://claude.ai/artifact/MRP5fJu6xixb7wyUcJaoq4 (private; share from its Share menu).

## Contents

| Path | What |
| --- | --- |
| `tokens/tokens.json` | Source of truth: colours (light, dark), type, spacing, radii, row heights, stage states |
| `tokens/theme.rs` | GPUI port of the tokens, ready to drop into `porter-desktop` |
| `tokens/tailwind-theme.css` | Tailwind v4 `@theme` port for the hub |
| `canvas/` | Source of every mockup board (`.dc.html`) and the canvas layout (`canvas.json`). They render in the Claude Design canvas, not standalone. |

## The rules

1. **Green means written.** Green appears only when a stage finished, wrote its output and opened no issues. For load that means applied to the destination. A dry run is never green.
2. **Unfinished looks unfinished.** Dry run is hatched with a dashed border. Skipped is hollow. Stale is dimmed with ↻ and a reason. None share a shape or colour with done.
3. **Problems sit next to their cause.** Every count links to its rows. Every invalid mapping row says why, inline. Every state is glyph + word, never colour alone.

## Stage states

| State | Look | Means |
| --- | --- | --- |
| not run | hollow circle, dashed border | never run, or cleaned |
| running | accent, dotted ring, % | in progress |
| clean | green, filled ✓ | finished, written, zero issues; load only after apply |
| warnings | amber, **square** ! | finished and written, with warnings |
| errors | red outline, filled ! | finished with error issues or failed items (exit 2); blocks Apply |
| failed | solid red, ✕ | could not run (exit 1); engine error shown verbatim |
| interrupted | purple, ‖ | cancelled (exit 3); offers Resume |
| stale | dimmed previous result + ↻ | upstream stage or mapping changed since |
| dry run | hatched, dashed | load without `--apply`; counts are "would write" |
| skipped | hollow, – | not run on purpose, e.g. verify after a dry run |

Stale is a modifier on the previous result, not a replacement: the card keeps its old chip and counts at 62% opacity and says why.

Issue severity: error (red circle), warning (amber square), info (grey ring). Resolutions: `open` is the loudest; `fixed-by-rerun` and `handled-manually` drop to muted; `wont-fix` is struck through. Nothing is hidden, only filtered.

## Apply

- *Apply load…* is a dashed ghost button with a lock, never the primary action. *Load (dry run)* is primary.
- The sheet checks three gates: a fresh dry run (model, mapping and schema unchanged since), no open errors, and warnings acknowledged.
- It lists exactly what will be written (from the dry run) and what will **not** be (dropped, missing media).
- Typing the destination host enables a red *Apply N writes* button. Paste is disabled. Cancel has focus; Apply has no shortcut.
- A stale or missing dry run replaces the confirm field with *Run dry run now*.
- After apply, load turns green but the header reads "Loaded, not yet verified" until verify runs.

## Desktop (GPUI)

- System font, 13px body, 28px table rows, 32px sidebar rows. Dense, Zed/Linear-like.
- No animation: skeletons are flat; the running ring is static and only the % repaints.
- Long paths truncate in the middle: the head shrinks with an ellipsis, the last two segments stay (`flex_none`). Full path in the tooltip.
- Dry-run hatch: `pattern_slash` where available, otherwise dashed border + `dry_bg`.
- Light and dark themes from the same token names.

## Hub (web)

- Inter, 15px body, 10px card radii, 44px minimum touch targets. Same colours and chips as desktop.
- Clients see Overview, Reports and Uploads only. Client copy is plain English ("to fix", "rehearsed, nothing moved yet"); issue details and raw data never appear.
- The overview always says which pushed version it shows and that it doesn't update live.
- Share-link tokens are shown once with a copy button, then only a fingerprint. Every link expires (max 90 days). The public `/s/[token]` view has a permanent "Shared view · read-only" bar with the expiry.
- Run secrets are write-only: replace or delete, never reveal.
- Uploads: checking (hash) → uploading (resumable) → verifying → ready. A checksum mismatch deletes the copy and asks for a re-upload; over 20 GB is rejected before upload.

## Boards on the canvas

Foundations; desktop Launcher, Overview, Mapping editor, URLs, Apply sheet, and edge states; hub Project overview (Role tweak: developer / client), Uploads, Share links (with run secrets and public view), Device approval, Run log (Outcome tweak), and edge states.

Sample data (Seaview Hotels, counts, people) is invented. The hub host `porter.umidigital.co.uk` is a placeholder.
