"use client";

import { useCallback, useEffect, useRef, useState } from "react";

const PURPOSES = {
  dump: ".sql .sql.gz .gz .zip .tar.gz .tgz",
  wxr: ".xml .xml.gz .zip",
  "media-archive": ".zip .tar .tar.gz .tgz",
  other: "",
};
type Purpose = keyof typeof PURPOSES;
type Row = { id: string; filename: string; size: number; purpose: string; state: string; created_at: string };
const PART_RETRIES = 5;
const PARALLEL = 3;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function call<T>(url: string, body?: unknown): Promise<T> {
  const r = await fetch(
    url,
    body === undefined
      ? undefined
      : { method: "POST", body: JSON.stringify(body), headers: { "content-type": "application/json" } },
  );
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error((j as { error?: string }).error ?? `HTTP ${r.status}`);
  return j as T;
}

/** sha256 in a Web Worker, streaming: the main thread stays free and a 20 GB file is never held in memory. */
function hashInWorker(file: File) {
  const w = new Worker(new URL("./hash.worker.ts", import.meta.url), { type: "module" });
  const promise = new Promise<string>((resolve, reject) => {
    w.onmessage = (e: MessageEvent<{ sha256?: string; error?: string }>) => {
      w.terminate();
      if (e.data.sha256) resolve(e.data.sha256);
      else reject(new Error(e.data.error ?? "hashing failed"));
    };
    w.onerror = (e) => {
      w.terminate();
      reject(new Error(e.message));
    };
    w.postMessage(file);
  });
  promise.catch(() => {});
  return { promise, terminate: () => w.terminate() };
}

/** sha256 of one part (at most 16 MiB, so held in memory): hex for the server, base64 for the S3 header. */
async function digestPart(blob: Blob) {
  const bytes = new Uint8Array(await crypto.subtle.digest("SHA-256", await blob.arrayBuffer()));
  return {
    hex: Array.from(bytes, (x) => x.toString(16).padStart(2, "0")).join(""),
    b64: btoa(String.fromCharCode(...bytes)),
  };
}

export function Uploader({
  slug,
  canUpload,
  canDownload,
}: {
  slug: string;
  canUpload: boolean;
  canDownload: boolean;
}) {
  const api = `/api/projects/${slug}/uploads`;
  const [rows, setRows] = useState<Row[]>([]);
  const [purpose, setPurpose] = useState<Purpose>("dump");
  const [file, setFile] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [pct, setPct] = useState(0);
  const [activeId, setActiveId] = useState<string | null>(null);
  const abort = useRef(false);
  const stopHash = useRef<(() => void) | null>(null);

  const refresh = useCallback(() => call<Row[]>(api).then(setRows, (e) => setError(e.message)), [api]);
  useEffect(() => {
    refresh();
    // Keep verifying uploads fresh.
    const t = setInterval(refresh, 5000);
    return () => clearInterval(t);
  }, [refresh]);

  const storeKey = file && `porter-upload:${slug}:${file.name}:${file.size}:${file.lastModified}`;
  const saved = () => {
    try {
      return storeKey ? localStorage.getItem(storeKey) : null;
    } catch {
      return null;
    }
  };
  const remember = (id: string | null) => {
    try {
      if (storeKey) id ? localStorage.setItem(storeKey, id) : localStorage.removeItem(storeKey);
    } catch {}
  };

  async function start() {
    if (!file) return;
    setBusy(true);
    setError("");
    setPct(0);
    abort.current = false;
    try {
      // Resume if this exact file was started before and the server still has it open.
      let id = saved();
      let have = new Set<number>();
      let partSize = 0;
      let count = 0;
      if (id) {
        try {
          const u = await call<{
            state: string;
            part_size: number;
            part_count: number;
            parts?: { part_number: number }[];
          }>(`${api}/${id}`);
          if (u.state === "uploading") {
            have = new Set((u.parts ?? []).map((p) => p.part_number));
            partSize = u.part_size;
            count = u.part_count;
            setStatus(`Resuming: ${have.size} of ${count} parts already uploaded`);
          } else id = null;
        } catch {
          id = null;
        }
      }
      if (!id) {
        const c = await call<{ id: string; part_size: number; part_count: number }>(api, {
          filename: file.name,
          size: file.size,
          purpose,
        });
        id = c.id;
        partSize = c.part_size;
        count = c.part_count;
        remember(id);
        setStatus("Uploading");
      }
      setActiveId(id);
      const uploadId = id;
      const hasher = hashInWorker(file);
      stopHash.current = hasher.terminate;

      let doneBytes = 0;
      const size = (n: number) => Math.min(partSize, file.size - (n - 1) * partSize);
      for (const n of have) doneBytes += size(n);
      setPct(Math.floor((doneBytes / file.size) * 100));
      const missing = Array.from({ length: count }, (_, i) => i + 1).filter((n) => !have.has(n));

      const sums = new Map<number, { hex: string; b64: string }>();
      const presign = async (ns: number[]) =>
        (
          await call<{ urls: Record<string, string> }>(`${api}/${uploadId}/parts`, {
            parts: ns.map((n) => ({ part_number: n, sha256: sums.get(n)?.hex })),
          })
        ).urls;
      const sendPart = async (n: number, url: string) => {
        const blob = file.slice((n - 1) * partSize, n * partSize);
        for (let attempt = 0; ; attempt++) {
          try {
            // S3 rejects the part unless it hashes to the signed checksum. A stalled connection times out.
            const r = await fetch(url, {
              method: "PUT",
              body: blob,
              headers: { "x-amz-checksum-sha256": sums.get(n)?.b64 ?? "" },
              signal: AbortSignal.timeout(120_000),
            });
            if (!r.ok) throw new Error(`part ${n}: HTTP ${r.status}`);
            return;
          } catch (e) {
            if (abort.current || attempt >= PART_RETRIES - 1) throw e;
            await sleep(500 * 2 ** attempt);
            // URLs expire after 15 minutes, so ask for a fresh one for the retry (itself retried by the loop).
            try {
              url = (await presign([n]))[n];
            } catch {}
          }
        }
      };
      for (let i = 0; i < missing.length; i += PARALLEL) {
        if (abort.current) return;
        const group = missing.slice(i, i + PARALLEL);
        for (const n of group) sums.set(n, await digestPart(file.slice((n - 1) * partSize, n * partSize)));
        const urls = await presign(group);
        await Promise.all(
          group.map(async (n) => {
            await sendPart(n, urls[n]);
            doneBytes += size(n);
            setPct(Math.floor((doneBytes / file.size) * 100));
          }),
        );
      }
      if (abort.current) return;
      setStatus("Hashing");
      const sha256 = await hasher.promise;
      await call(`${api}/${uploadId}/complete`, { sha256 });
      remember(null);
      setStatus("Uploaded; verifying");
      setFile(null);
      setActiveId(null);
    } catch (e) {
      stopHash.current?.();
      // Progress is kept in localStorage: pick the same file again and press Resume.
      setError((e as Error).message);
      setStatus("");
    } finally {
      setBusy(false);
      refresh();
    }
  }

  async function cancel() {
    abort.current = true;
    stopHash.current?.();
    const id = activeId ?? saved();
    if (!id) return;
    try {
      await call(`${api}/${id}/abort`, {});
      remember(null);
      setActiveId(null);
      setStatus("Aborted");
    } catch (e) {
      setError((e as Error).message);
    }
    refresh();
  }

  async function download(id: string) {
    try {
      window.location.href = (await call<{ url: string }>(`${api}/${id}/download`)).url;
    } catch (e) {
      setError((e as Error).message);
    }
  }

  const resumable = !!file && !busy && !!saved();
  return (
    <section>
      {canUpload && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            start();
          }}
        >
          <p>
            <label>
              Purpose{" "}
              <select value={purpose} onChange={(e) => setPurpose(e.target.value as Purpose)} disabled={busy}>
                {Object.keys(PURPOSES).map((p) => (
                  <option key={p} value={p}>
                    {p}
                  </option>
                ))}
              </select>
            </label>{" "}
            <small>{PURPOSES[purpose] || "any file type"}</small>
          </p>
          <p>
            <input
              type="file"
              accept={PURPOSES[purpose].split(" ").join(",") || undefined}
              disabled={busy}
              onChange={(e) => setFile(e.target.files?.[0] ?? null)}
            />
          </p>
          <p>
            <button type="submit" disabled={!file || busy}>
              {resumable ? "Resume" : "Upload"}
            </button>{" "}
            {(busy || resumable) && (
              <button type="button" onClick={cancel}>
                Abort
              </button>
            )}
          </p>
          {(busy || status) && (
            <p>
              <progress value={pct} max={100} /> {pct}% {status}
            </p>
          )}
        </form>
      )}
      {error && <p role="alert">{error}</p>}
      <h2>Uploads</h2>
      <table>
        <thead>
          <tr>
            <th>File</th>
            <th>Purpose</th>
            <th>Size</th>
            <th>State</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.id}>
              <td>{r.filename}</td>
              <td>{r.purpose}</td>
              <td>{(r.size / 1024 ** 2).toFixed(1)} MiB</td>
              <td>{r.state}</td>
              <td>
                {canDownload && r.state === "ready" && (
                  <button type="button" onClick={() => download(r.id)}>
                    Download
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {rows.length === 0 && <p>No uploads yet.</p>}
    </section>
  );
}
