import { sha256Blob } from "../lib/hash-stream";

/** Hashes the File it is sent, posting `{progress}` then `{sha256}` (or `{error}`). */
self.onmessage = async (e: MessageEvent<File>) => {
  try {
    const sha256 = await sha256Blob(e.data, (progress) => self.postMessage({ progress }));
    self.postMessage({ sha256 });
  } catch (err) {
    self.postMessage({ error: (err as Error).message });
  }
};
