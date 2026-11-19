import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";

/**
 * Streaming sha256 of a Blob/File, hex. Web Crypto has no incremental digest, so a 20 GB file can't go through
 * it without being held in memory; this reads the stream chunk by chunk. Runs in Node and in a Web Worker.
 */
export async function sha256Blob(blob: Blob, onProgress?: (bytesDone: number) => void) {
  const h = sha256.create();
  let done = 0;
  const reader = blob.stream().getReader();
  for (;;) {
    const { value, done: end } = await reader.read();
    if (end) break;
    h.update(value);
    done += value.length;
    onProgress?.(done);
  }
  return bytesToHex(h.digest());
}
