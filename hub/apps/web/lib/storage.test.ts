import { expect, test } from "vitest";

// Env must be set before storage.ts builds its (memoised) clients.
process.env.S3_PUBLIC_ENDPOINT = "https://s3.public.test";
const { presignGet, presignPut, head, keys } = await import("./storage");

test("presigned urls use the public endpoint; server-side calls use the internal one", async () => {
  const key = keys.media("00000000-0000-0000-0000-000000000000", "a".repeat(64));
  expect(new URL(await presignGet(key)).host).toBe("s3.public.test");
  expect(new URL(await presignPut(key, "a".repeat(64))).host).toBe("s3.public.test");
  // head() would fail to resolve s3.public.test; it works because it uses S3_ENDPOINT.
  expect(await head(key)).toBeNull();
});

test("an expired presigned url is refused", async () => {
  process.env.S3_PUBLIC_ENDPOINT = "";
  // fresh module so the public client targets the real endpoint
  const { vi } = await import("vitest");
  vi.resetModules();
  const s = await import("./storage");
  const url = await s.presignGet("projects/x/never", 1);
  await new Promise((r) => setTimeout(r, 2500));
  const res = await fetch(url);
  expect(res.status).toBe(403);
  expect(await res.text()).toContain("expired");
});
