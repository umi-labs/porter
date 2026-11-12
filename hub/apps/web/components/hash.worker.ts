import { sha256Blob } from "../lib/hash-stream";

/** Hashes the File it is sent and posts `{sha256}` (or `{error}`). */
self.onmessage = async (e: MessageEvent<File>) => {
  try {
    self.postMessage({ sha256: await sha256Blob(e.data) });
  } catch (err) {
    self.postMessage({ error: (err as Error).message });
  }
};
