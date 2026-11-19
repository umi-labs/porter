# Prompt for Claude Design: Porter hub and desktop app

Design two related interfaces for **Porter**, a tool a small web agency (Umi) uses to move client websites from one CMS to another, for example WordPress to Payload or Sanity. It works in stages:

- **analyse:** what the old site has
- **extract:** pull everything out
- **normalise:** a platform-neutral format
- **schema:** check or generate the destination's content model
- **load:** write into the new CMS. It's a dry run by default; applying it is a deliberate act.
- **verify:** check counts, and that old URLs land somewhere

The core value is **honesty**. Porter must never make a migration look finished when content is missing, mis-wired or failed. Its worst outcome is "everything moved, but connected wrongly". Design so that problems are impossible to overlook.

## 1. Desktop app (native macOS, built with GPUI, Zed's UI framework)

Constraints: GPUI draws flex-box divs, text, borders, rounded corners, icons and images. There's no CSS animation library and no web fonts beyond the system font, so keep it Zed/Linear-like: dense, calm, keyboard-friendly. Light and dark themes.

**Screens:**
- **Launcher:** recent projects (name plus a long folder path), *Open folder…*, *New project…*. Long paths must truncate in the middle with an ellipsis, never overflow.
- **Project window:** a left sidebar with Overview, Content, Media, URLs, Issues, Mapping, Destination and Runs. The top bar shows the project name, *source → destination* (the destination may be unset, so show that state), *Run all*, *Load (dry run)* and *Apply load…*.
  - *Apply* must not look like the primary action. It is gated: it needs a fresh dry run plus typing the destination host into a confirm sheet.
- **Overview:**
  - A pipeline strip of the 6 stages. Each stage has a state: not run, running with progress, clean, done with warnings, errors, failed, interrupted, stale, or dry run (nothing written).
  - Each stage also shows its last run (as a relative time), its counts, and Run and Cancel.
  - Below the strip: coverage by connector, and open issues by kind and severity.
- **Content:** a tree of documents by type, then by parent. Each document's detail shows:
  - the old URL and new path
  - status per stage
  - a fields table
  - a rich-text preview, with links marked resolved or unresolved
  - a layout block outline
  - its issues
- **Media:** a thumbnail grid. It shows placeholders for missing or undecodable files and usage counts.
- **URLs:** a large virtualised table of old URL → status → resolves to → new URL. Filters: unresolved, redirect chains, not found.
- **Issues:** a filterable table (stage, kind, severity, resolution), bulk resolve with a note, and jump to the item.
- **Mapping editor:** the headline feature. It holds three tables:
  - **Types:** source type → destination collection, path pattern, drop + reason.
  - **Fields:** for each type: field path, sample values, fill rate, target field, transforms, drop.
  - **Blocks:** builder modules → destination blocks.

  Validation problems show inline next to the row that causes them.
- **Destination:** schema-check results grouped by collection, with Generate schema, Write agent prompt and Write spec.
- **Runs:** history per stage, and a live log while a run is in progress.

## 2. Hub (web app: Next.js, plain server-rendered pages, Tailwind fine)

This is a central server that local projects push to. People who don't run Porter can see results and hand over files there.

**Users:** Umi staff (Google sign-in) and clients (invited, magic link). Per-project roles are owner, developer, viewer and client. Clients only see the overview and reports, and they can upload files. They never see raw data or issue details.

**Pages:**
- **Sign-in:** Google for staff; email magic link for clients; a password fallback.
- **Device approval page:** "Approve sign-in for porter CLI, code ABCD-EFGH".
- **Projects list.**
- **Project overview:** the latest version (v3, pushed by whom, when), pipeline state, issue counts, coverage, report links, and a Run button for developers.
- **Issues and URLs tables:** paged, 100 per page.
- **Report viewer:** porter's own HTML reports shown in a sandboxed iframe.
- **Runs:** a remote run with a live streaming log, a cancel button, and the exit result (success, issues found, failed, interrupted).
- **Uploads:** for clients. Upload a database dump, WordPress export or media archive, up to 20 GB.
  - The upload can resume after a dropped connection.
  - It shows hashing progress, upload progress, then verifying, then ready or failed.
- **Settings:**
  - Members and roles, and invitations.
  - **Share links:** scope is "reports only" or "read-only project", with an expiry. The token is shown once, with a copy button. Links can be revoked.
  - Run secrets: write-only, never shown again.
  - Retention.
- **Public share-link view (`/s/[token]`):** read-only and clearly labelled as a shared view, with an expiry notice.

## What I want back

- One cohesive visual language across both. The desktop app is denser; the hub is a little friendlier for clients.
- A colour and state system for the stage and issue states above. Green must mean only "clean and actually written". "Dry run" and "skipped" must look clearly different from done.
- Mockups for:
  - desktop: Launcher, Overview, Mapping editor, URLs, Apply confirm sheet
  - hub: Project overview, Uploads (mid-upload and failed-verification states), Share-link creation (with the token shown once), Device approval, Run log
- Empty, loading, error and very-long-content states for each.
- Design tokens (colours, type scale, spacing, radii) that I can port to GPUI and Tailwind.
