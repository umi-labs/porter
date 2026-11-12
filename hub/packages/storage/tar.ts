import type { createHash } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { mkdir, readdir, stat } from "node:fs/promises";
import { dirname, join, resolve, sep } from "node:path";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import { createZstdCompress, createZstdDecompress } from "node:zlib";
import * as tar from "tar-stream";

// zlib's zstd needs Node >= 22.15.

const MAX_ENTRIES = 200_000;
const maxBytes = () => Number(process.env.HUB_SNAPSHOT_MAX_BYTES) || 2 * 1024 ** 3;

/** Throws unless `name` is a relative path with no `..`; returns it without a leading ./ or trailing /. "" is the archive root ("./"). */
function safeName(name: string) {
  const n = name.replace(/^(\.\/)+/, "").replace(/\/+$/, "");
  if (
    name.startsWith("/") ||
    n.includes("\0") ||
    n.split("/").some((s) => s === ".." || (s === "" && n !== ""))
  ) {
    throw new Error(`unsafe path in snapshot: ${JSON.stringify(name)}`);
  }
  return n === "." ? "" : n;
}

/** Passes bytes through, failing once more than `limit` have gone by. */
const capBytes = (limit: number, what: string, seen: { n: number }) =>
  new Transform({
    transform(chunk, _enc, cb) {
      seen.n += chunk.length;
      cb(seen.n > limit ? new Error(`${what} is larger than ${limit} bytes`) : null, chunk);
    },
  });

/**
 * Streams a tar.zst (zstd -> tar parser -> files) into `dest`; memory stays O(chunk), the event loop is never blocked.
 * Only regular files and directories are accepted: a symlink, hardlink or device entry, an absolute path or a
 * `..` rejects the whole archive. The cap counts bytes actually decompressed and written, not sizes the archive declares.
 * If `hash` is given it is fed the compressed bytes.
 */
export async function extractTarZst(
  source: Buffer | Readable,
  dest: string,
  limit = maxBytes(),
  hash?: ReturnType<typeof createHash>,
) {
  const root = resolve(dest);
  await mkdir(root, { recursive: true });
  const input = Buffer.isBuffer(source) ? Readable.from([source]) : source;
  if (hash) input.on("data", (c) => hash.update(c));
  const extract = tar.extract();
  const written = { n: 0 };
  const run = pipeline(
    input,
    createZstdDecompress(),
    capBytes(limit + 1024 * 1024, "snapshot", { n: 0 }),
    extract,
  );
  const walk = (async () => {
    let entries = 0;
    for await (const entry of extract) {
      const { name, type } = entry.header;
      if (++entries > MAX_ENTRIES) throw new Error("snapshot has too many entries");
      if (type !== "file" && type !== "directory") {
        throw new Error(
          `snapshot entry ${JSON.stringify(name)} has unsupported type ${JSON.stringify(type)}`,
        );
      }
      const rel = safeName(name);
      const out = resolve(root, rel);
      if (!rel && type === "directory") {
        entry.resume();
        continue;
      }
      if (!rel || !out.startsWith(root + sep))
        throw new Error(`unsafe path in snapshot: ${JSON.stringify(name)}`);
      if (type === "directory") {
        await mkdir(out, { recursive: true, mode: 0o755 });
        entry.resume();
      } else {
        await mkdir(dirname(out), { recursive: true, mode: 0o755 });
        await pipeline(entry, capBytes(limit, "snapshot", written), createWriteStream(out, { mode: 0o644 }));
      }
    }
  })();
  // Whichever side fails first rejects; tear the source down so S3 stops sending.
  try {
    await Promise.all([run, walk]);
  } catch (e) {
    input.destroy();
    extract.destroy();
    throw e;
  }
}

/**
 * Packs `dir` into a tar.zst at `out`: regular files and directories only (symlinks and the like are skipped),
 * paths relative to `dir`. `skip(rel)` leaves out an entry (and, for a directory, everything under it);
 * `replace` swaps a file's bytes for the given buffer (the worker packs the original porter.toml).
 */
export async function packTarZst(
  dir: string,
  out: string,
  skip: (rel: string) => boolean = () => false,
  replace: Record<string, Buffer> = {},
) {
  const pack = tar.pack();
  const write = pipeline(pack, createZstdCompress(), createWriteStream(out));
  const walk = async (rel: string) => {
    const entries = await readdir(join(dir, rel), { withFileTypes: true });
    entries.sort((a, b) => (a.name < b.name ? -1 : 1));
    for (const e of entries) {
      const name = rel ? `${rel}/${e.name}` : e.name;
      if (skip(name)) continue;
      if (e.isDirectory()) {
        pack.entry({ name, type: "directory", mode: 0o755 });
        await walk(name);
      } else if (e.isFile()) {
        const body = replace[name];
        if (body) {
          pack.entry({ name, mode: 0o644 }, body);
          continue;
        }
        const { size } = await stat(join(dir, name));
        const entry = pack.entry({ name, size, mode: 0o644 });
        await pipeline(createReadStream(join(dir, name)), entry);
      }
    }
  };
  try {
    await walk("");
    pack.finalize();
  } catch (e) {
    pack.destroy(e as Error);
  }
  await write;
}
