import { parse } from "smol-toml";
import { describe, expect, test } from "vitest";
import { applySetup, readSetup, uploadRefs } from "./config";

const ID = "0193c1a2-7b3c-7d4e-8f90-123456789abc";

describe("setup toml", () => {
  test("a fresh setup round-trips through readSetup", () => {
    const toml = applySetup(
      null,
      "Acme",
      {
        adapter: "wordpress",
        kinds: ["wxr", "crawl"],
        upload: ID,
        siteUrl: "https://old.example.com",
        destUrl: "https://cms.example.com",
      },
      "export.xml",
    );
    const doc = parse(toml) as Record<string, any>;
    expect(doc.project.name).toBe("Acme");
    expect(doc.connectors).toEqual([
      { kind: "wxr", path: `uploads/${ID}/export.xml` },
      { kind: "crawl", url: "https://old.example.com" },
    ]);
    expect(doc.destination).toEqual({
      adapter: "payload",
      url: "https://cms.example.com",
      api_key_env: "PAYLOAD_API_KEY",
    });
    expect(readSetup(toml)).toMatchObject({
      adapter: "wordpress",
      kinds: ["wxr", "crawl"],
      upload: ID,
      uploadName: "export.xml",
      siteUrl: "https://old.example.com",
      destUrl: "https://cms.example.com",
    });
    expect(uploadRefs(toml)).toEqual([{ id: ID, path: `uploads/${ID}/export.xml` }]);
  });

  test("re-setup replaces connectors and keeps mapping, tidy and the existing name", () => {
    const prev = `[project]\nname = "Old"\n[source]\nadapter = "wordpress"\n[[connectors]]\nkind = "wp-db"\ndump = "d.sql"\n[mapping.types.post]\ncollection = "posts"\n[[tidy]]\nkind = "merge-types"\n`;
    const toml = applySetup(prev, "New", {
      adapter: "sanity",
      kinds: ["sanity-api"],
      projectId: "abc",
      dataset: "production",
    });
    const doc = parse(toml) as Record<string, any>;
    expect(doc.project.name).toBe("Old");
    expect(doc.source.adapter).toBe("sanity");
    expect(doc.connectors).toEqual([
      { kind: "sanity-api", project_id: "abc", dataset: "production", token_env: "PORTER_SANITY_TOKEN" },
    ]);
    expect(doc.mapping.types.post.collection).toBe("posts");
    expect(doc.tidy).toHaveLength(1);
    expect(uploadRefs(toml)).toEqual([]);
    // Clearing the live site drops its crawl and its verify check.
    const withSite = applySetup(prev, "New", {
      adapter: "wordpress",
      kinds: ["crawl"],
      siteUrl: "https://old.example.com",
    });
    const cleared = parse(
      applySetup(withSite, "New", {
        adapter: "wordpress",
        kinds: ["wp-rest"],
        url: "https://old.example.com",
      }),
    ) as Record<string, any>;
    expect(cleared.verify).toBeUndefined();
    expect(cleared.connectors).toEqual([{ kind: "wp-rest", url: "https://old.example.com" }]);
  });

  test("REST and a crawl together; a pasted REST URL as the live site crawls the site root", () => {
    const rest = "https://hotel.example.com/wp-json/wp/v2/pages/";
    const toml = applySetup(null, "Hotel", {
      adapter: "wordpress",
      kinds: ["wp-rest", "crawl"],
      url: rest,
      siteUrl: rest,
    });
    const doc = parse(toml) as Record<string, any>;
    expect(doc.connectors).toEqual([
      { kind: "wp-rest", url: rest },
      { kind: "crawl", url: "https://hotel.example.com" },
    ]);
    expect(doc.verify.site_url).toBe("https://hotel.example.com");
    expect(readSetup(toml)).toMatchObject({
      kinds: ["wp-rest", "crawl"],
      url: rest,
      siteUrl: "https://hotel.example.com",
    });
  });

  test("uploadRefs only takes uploads/<uuid>/<name> paths", () => {
    const toml = `[[connectors]]\nkind = "umbraco-json"\npaths = ["uploads/${ID}/a.json", "uploads/${ID}/../x", "local.json", "uploads/nope/a.json"]\n`;
    expect(uploadRefs(toml)).toEqual([{ id: ID, path: `uploads/${ID}/a.json` }]);
  });
});
