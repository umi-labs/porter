// No dependencies: the browser imports this (setup form).

/** Where a connector's input comes from. */
export type Input = "upload" | "url" | "sanity-api" | "local-only" | "none";

export type Way = { kind: string; label: string; input: Input; hint: string };
export type Source = { adapter: string; label: string; ways: Way[] };

/**
 * Each porter source and the ways in to it, as the hub's setup form offers them. `kind` is the porter.toml
 * connector kind; "crawl-only" adds no connector beyond the live-site crawl. wp-db is never run on the hub (it
 * runs the client's PHP in Docker): it is listed so people see where it went.
 */
export const SOURCES: Source[] = [
  {
    adapter: "wordpress",
    label: "WordPress",
    ways: [
      {
        kind: "wxr",
        label: "WordPress export file",
        input: "upload",
        hint: "Tools → Export in wp-admin (.xml)",
      },
      {
        kind: "wp-rest",
        label: "WordPress REST API",
        input: "url",
        hint: "The site, or any /wp-json/ URL on it",
      },
      {
        kind: "crawl-only",
        label: "Crawl the live site only",
        input: "none",
        hint: "Needs the live site URL",
      },
      {
        kind: "wp-db",
        label: "Database dump",
        input: "local-only",
        hint: "Runs locally only: upload the dump here, then run porter on your machine and push",
      },
    ],
  },
  {
    adapter: "sanity",
    label: "Sanity",
    ways: [
      {
        kind: "sanity-export",
        label: "Sanity dataset export",
        input: "upload",
        hint: "sanity dataset export (.tar.gz)",
      },
      {
        kind: "sanity-api",
        label: "Sanity API",
        input: "sanity-api",
        hint: "Project id, dataset and a read token",
      },
    ],
  },
  {
    adapter: "umbraco",
    label: "Umbraco",
    ways: [
      { kind: "umbraco-json", label: "Umbraco JSON export", input: "upload", hint: "The JSON export file" },
    ],
  },
];

/** Run-secret names the setup form writes; porter.toml only ever names them. */
export const SANITY_TOKEN_ENV = "PORTER_SANITY_TOKEN";
export const PAYLOAD_KEY_ENV = "PAYLOAD_API_KEY";
/** porter's wp-rest connector reads these (basic auth: a WordPress application password). */
export const WP_USER_ENV = "PORTER_WP_USERNAME";
export const WP_PASSWORD_ENV = "PORTER_WP_PASSWORD";

/** What the setup form submits (and reads back from a porter.toml). */
export type Setup = {
  adapter: string;
  kind: string;
  /** Upload id, for `upload` ways. */
  upload?: string;
  /** Site root, for wp-rest. */
  url?: string;
  projectId?: string;
  dataset?: string;
  /** The old live site: crawled, and checked by verify. */
  siteUrl?: string;
  /** Payload admin URL. */
  destUrl?: string;
};

export const wayOf = (adapter: string, kind: string) =>
  SOURCES.find((s) => s.adapter === adapter)?.ways.find((w) => w.kind === kind);
