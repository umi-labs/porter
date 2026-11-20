# Moving a Sanity site with porter

Porter reads a Sanity dataset in full: documents, drafts, references, Portable Text, page-builder arrays, images and files, and documents that are really form submissions. It loads them into Payload. This page covers setting it up, what to expect, and how to tidy the structure on the way.

## Get a read token

Porter needs a token that can read the dataset. It never writes to Sanity.

1. Open [Sanity Manage](https://www.sanity.io/manage) and choose the project.
2. Go to API, then Tokens, then Add API token.
3. Name it (`porter`), give it the **Viewer** role, and save. Copy the token now; Sanity shows it once.
4. Put it in an environment variable. The default name is `PORTER_SANITY_TOKEN`:

```bash
export PORTER_SANITY_TOKEN=sk...
```

The token is never written to `porter.toml`. If Sanity answers 401 or 403, porter stops with exit code 1 and names the variable it read.

## Connectors

Use `[source] adapter = "sanity"` with one of two connectors.

**`sanity-api`** (the default) streams the dataset from Sanity's export API. Nothing else needs installing.

```toml
[[connectors]]
kind = "sanity-api"
project_id = "abc123"
dataset = "production"
# token_env = "PORTER_SANITY_TOKEN"
# api_version = "v2021-06-07"
assets = "referenced"        # referenced | all
```

**`sanity-export`** reads a `sanity dataset export` file someone sent you: a `.tar.gz`, or a folder holding `data.ndjson`. Use it when you have no token. A `--no-assets` export has no image references, so it cannot carry the images.

```toml
[[connectors]]
kind = "sanity-export"
path = "export/production.tar.gz"
assets = "referenced"
```

### Assets

`assets = "referenced"` (the default) downloads only the images and files that a document uses. Unused ones in the library are counted, with their size, in `porter analyse`. `assets = "all"` downloads those too.

Assets are fetched four at a time, retried on network errors and 5xx, and checked against Sanity's SHA-1 for the file. A mismatch is an issue, not a silent bad file. **The fetch resumes.** Media is stored by content hash, so a file already in `media/` with the right hash is skipped. If a large extract is interrupted, run `porter extract` again and it carries on.

## Drafts

- A document that exists only as a draft loads as a Payload draft.
- A draft of a document that is also published is **not loaded**. Porter raises one `unpublished-changes` warning per document, so someone can publish or discard the change in Sanity before the final run.

## Form submissions

Some Sanity datasets store form submissions as ordinary documents. Porter treats a type as submissions when it is detected (no slug, at least 50 documents, contact-like fields such as email, name, phone and message), or when you list it:

```toml
[mapping.forms]
submission_types = ["enquiryForm"]
```

List a type by hand when it has fewer than 50 documents. Add the line after `porter analyse` has run: analyse seeds an empty `[mapping.forms]` only while `porter.toml` has no `[mapping]`, so edit that table rather than adding a second one. Re-run `porter normalise` afterwards.

Each submission type becomes one form, with fields from the union of submitted names. Nested objects are flattened to dotted keys (`holidayDetails.destination`). Loading them needs the Payload form-builder plugin; `porter schema check` says so when it is missing.

Submissions are personal data. They show as counts only, and **never** appear in flow examples, reports or share links.

## URLs

Sanity holds no URLs. The site's frontend decides them, so you give porter the rules as path patterns. A token can follow references, and a lookup maps the value:

```toml
[mapping.types.cityPage]
path = "/destinations/{country.region|lookup:regions}/{country.slug}/{slug}"

[mapping.lookups.regions]
europe = "europe"
"further-afield" = "further-afield"
```

`{country.slug}` follows the `country` reference and reads its `slug`. Copy the patterns from the old frontend's routing code (for Gatsby, `getSlug()` in `gatsby-node`). A missing lookup entry or a reference with no target is an issue naming the document, not a wrong URL.

For a check on those patterns, add the `crawl` connector for the live site. Porter merges its URLs with the pattern-made routes: old URLs no pattern produced are a warning, and pattern URLs the old site does not have are info.

```toml
[[connectors]]
kind = "crawl"
url = "https://www.example.com"
```

Other mapping keys that help here: `title_field` picks the source field for a type's title (otherwise the first of `title`, `name`, `heading`, `label`), and `parent` names a reference field whose target becomes the document's parent.

## Schema into a Payload repo

```bash
porter schema generate --into ../acme-site
```

This writes `src/collections/porter/`, `src/blocks/porter/` and an `index.ts` exporting `porterCollections` and `porterGlobals`. It prints the two lines to add to `payload.config.ts` (one import, one spread of the arrays) and a one-file catch-all route, `app/(frontend)/[[...path]]/page.tsx`, that serves any document by its old path. Then run `payload migrate:create`.

Porter **never edits `payload.config.ts`** or any file outside those folders. Every generated collection has an indexed, unique `path` text field, which load fills from the path pattern.

If a generated collection has the slug of one the app already has (a `pages` collection against the template's `Pages`), porter writes nothing and exits 2. Either map onto the existing collection and add the missing fields from `porter schema check` (or give `porter schema prompt` to an agent), or rename the generated one with `to = "..."`.

## Tidy-ups

Porter can merge look-alike types or blocks on the way in. It is optional: with nothing switched on, the destination mirrors Sanity.

```bash
porter tidy list               # every rule, on or off
porter tidy suggest            # look for more
porter tidy enable <id>
porter tidy disable <id>
porter tidy dismiss <id>       # turn a suggestion down for good
```

`porter normalise` suggests rules by comparing field names (at least 70% overlap). Suggestions are written to `porter.toml` **disabled**, each with a plain label and a reason. You turn one on; nobody writes them by hand. Comments in `porter.toml` survive the edit.

To turn a suggestion down, dismiss it rather than deleting it: a deleted rule comes back at the next normalise. `porter tidy dismiss <id>` sets `dismissed = true` (and `enabled = false`). The rule stays in `porter.toml`, is never suggested again, and is left out of the flow. `porter tidy enable <id>` takes the dismissal back.

```toml
[[tidy]]
id = "destinations"
kind = "merge-types"          # or merge-blocks
enabled = true
label = "Merge Country pages and Region pages into Destinations"
from = ["regionPage", "countryPage"]
into = "destinations"
discriminator = { field = "kind", values = { regionPage = "region", countryPage = "country" } }
parent = { countryPage = "region" }   # parent from a reference field
```

- `merge-types` puts several document types in one collection. A discriminator field records what each was.
- `merge-blocks` puts several block types in one block, with `fields = { "typeA.cards" = "items" }` to align differing names.

Porter accounts for every field. A field is carried, renamed (listed), or dropped (listed with a reason in `drop`). A field that holds data and is none of those is an error that blocks load, as is a shape conflict between fields merged under one name. A discriminator may not reuse the name of a mapped field. Normalise output stays a faithful copy of Sanity; tidy-ups change where things go, not what the source said.

## The flow

```bash
porter flow            # a readable summary
porter flow --json     # one NDJSON event holding the graph
```

The flow is a graph of what goes where: a source node per type and block, a tidy node per rule (off rules drawn as passing straight through), and a destination node per collection, global and block. Each shows counts and, where it is safe, one real before-and-after example, with a plain-English summary. Counts come from the last normalise and load; before any dry run they read "not counted" rather than zero.

Normalise and every load write the same graph to `reports/flow.json`. Form submissions never carry an example.

## Example: a Kirker-shaped project

```toml
[project]
name = "kirker"

[source]
adapter = "sanity"

[[connectors]]
kind = "sanity-api"
project_id = "abc123"
dataset = "production"
assets = "referenced"

[[connectors]]
kind = "crawl"
url = "https://www.example.com"

[destination]
adapter = "payload"
url = "http://localhost:3000"
api_key_env = "PORTER_PAYLOAD_API_KEY"
auth_collection = "users"
config_dir = "../kirker-site/src"

[verify]
site_url = "http://localhost:3000"

[mapping.forms]
collection = "forms"
submissions = "form-submissions"
submission_types = ["enquiryForm"]

[mapping.types.homepage]
to = "pages"
path = "/"

[mapping.types.hotel]
to = "hotels"
path = "/hotel/{slug}"

[mapping.types.cityPage]
to = "destinations"
path = "/destinations/{country.region|lookup:regions}/{country.slug}/{slug}"

[mapping.lookups.regions]
europe = "europe"
"further-afield" = "further-afield"
```

A typical run:

```bash
export PORTER_SANITY_TOKEN=...
porter init kirker --adapter sanity
porter analyse                 # seeds [mapping]; then edit it
porter extract                 # resumable
porter normalise
porter tidy list               # enable what you want
porter flow
porter schema generate --into ../kirker-site
porter load                    # dry run
porter load --apply
porter verify
```
