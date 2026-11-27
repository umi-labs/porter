"use client";

import { DownloadIcon, UploadIcon } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Chip, type State } from "@/components/porter/chip";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";

const PURPOSES = {
  dump: ".sql .sql.gz .gz .zip .tar.gz .tgz",
  wxr: ".xml .xml.gz .zip",
  "media-archive": ".zip .tar .tar.gz .tgz",
  "sanity-export": ".tar.gz .tgz",
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
  maxBytes,
}: {
  slug: string;
  canUpload: boolean;
  canDownload: boolean;
  /** The server's limit; a bigger file is rejected here, before anything is sent. */
  maxBytes: number;
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
  const [hashed, setHashed] = useState(false);
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

  const tooBig = !!file && file.size > maxBytes;
  async function start() {
    if (!file || file.size > maxBytes) return;
    setBusy(true);
    setError("");
    setPct(0);
    setHashed(false);
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
      hasher.promise.then(
        () => setHashed(true),
        () => {},
      );

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
  const uploaded = pct >= 100;
  // checking (fingerprint, alongside the upload) → uploading → verifying (server) → ready.
  const steps: [string, "done" | "active" | "pending", number][] = [
    [hashed ? "Checked ✓" : "Checking file", hashed ? "done" : "active", hashed ? 100 : 0],
    [uploaded ? "Uploaded ✓" : `Uploading · ${pct}%`, uploaded ? "done" : "active", pct],
    ["Verifying", status.startsWith("Uploaded") ? "active" : "pending", 0],
    ["Ready", "pending", 0],
  ];
  return (
    <section className="flex flex-col gap-5 text-sm">
      {canUpload && (
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            start();
          }}
        >
          <div className="flex flex-wrap items-end gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="upload-purpose">What is it?</Label>
              <NativeSelect
                id="upload-purpose"
                value={purpose}
                onChange={(e) => setPurpose(e.target.value as Purpose)}
                disabled={busy}
              >
                {Object.keys(PURPOSES).map((p) => (
                  <NativeSelectOption key={p} value={p}>
                    {LABELS[p as Purpose]}
                  </NativeSelectOption>
                ))}
              </NativeSelect>
            </div>
            <small className="pb-1.5 text-xs text-muted-foreground">
              {PURPOSES[purpose] || "any file type"}
            </small>
          </div>
          <label
            className="relative flex cursor-pointer flex-col items-center gap-2.5 rounded-xl border-2 border-dashed bg-card px-5 py-7 text-center transition-colors hover:bg-muted/50 has-focus-visible:border-ring has-focus-visible:ring-2 has-focus-visible:ring-ring/30"
            onDragOver={(e) => e.preventDefault()}
            onDrop={(e) => {
              e.preventDefault();
              if (!busy) setFile(e.dataTransfer.files?.[0] ?? null);
            }}
          >
            <UploadIcon className="size-8 text-primary" strokeWidth={1.8} aria-hidden="true" />
            <span className="text-base font-semibold">
              {file ? (
                file.name
              ) : (
                <>
                  Drop a file here, or <span className="text-primary underline">choose a file</span>
                </>
              )}
            </span>
            <span className="text-muted-foreground">
              {file
                ? `${fmtSize(file.size)}${resumable ? " · started before: Resume sends only what’s missing" : ""}`
                : `Up to ${fmtSize(maxBytes)} each`}
            </span>
            <input
              type="file"
              className="absolute size-px opacity-0"
              accept={PURPOSES[purpose].split(" ").join(",") || undefined}
              disabled={busy}
              onChange={(e) => setFile(e.target.files?.[0] ?? null)}
            />
          </label>
          {tooBig && (
            <p role="alert" className="m-0 rounded-md bg-err-soft p-3 text-err">
              This file is over the {fmtSize(maxBytes)} limit, so nothing was uploaded. Split it into smaller
              archives, or ask your Umi contact for another way to send it.
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button type="submit" size="lg" disabled={!file || busy || tooBig} className="press h-9 px-4">
              {resumable ? "Resume" : "Upload"}
            </Button>
            {(busy || resumable) && (
              <Button variant="outline" size="lg" onClick={cancel} className="press h-9 px-4">
                Cancel upload
              </Button>
            )}
          </div>
          {(busy || status) && (
            <div className="animate-enter flex flex-col gap-3 rounded-lg bg-card p-4 ring-1 ring-foreground/10">
              <ol className="m-0 grid list-none grid-cols-2 gap-1.5 p-0 sm:grid-cols-4">
                {steps.map(([label, st, w]) => (
                  <li key={label} className="flex flex-col gap-1.5">
                    <Progress
                      aria-hidden="true"
                      value={st === "done" ? 100 : w}
                      className={cn(st === "done" && "[&_[data-slot=progress-indicator]]:bg-foreground/80")}
                    />
                    <span
                      className={cn(
                        "text-xs",
                        st === "pending" ? "text-muted-foreground" : "text-foreground",
                        st === "active" && "font-semibold",
                      )}
                    >
                      {label}
                    </span>
                  </li>
                ))}
              </ol>
              {status && <span className="text-muted-foreground">{status}</span>}
              <progress value={pct} max={100} className="sr-only">
                {pct}%
              </progress>
            </div>
          )}
        </form>
      )}
      {error && (
        <p role="alert" className="m-0 rounded-md bg-err-soft p-3 text-err">
          {error}
        </p>
      )}
      <section className="flex flex-col gap-3">
        <h2 className="m-0 text-base font-semibold">Files</h2>
        {rows.length === 0 && (
          <Empty className="border bg-card">
            <EmptyHeader>
              <EmptyTitle className="text-sm">Nothing uploaded yet</EmptyTitle>
              <EmptyDescription className="text-sm">
                Your Umi contact will tell you what to send. Usually a database export and a zip of your
                images.
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        )}
        {rows.map((r) => {
          const look = ROW_LOOK[r.state] ?? { state: "not-run" as const, label: r.state };
          return (
            <article
              key={r.id}
              className={cn(
                "animate-enter flex flex-col gap-3 rounded-lg border bg-card px-4.5 py-4",
                r.state === "failed" && "border-err/40",
              )}
            >
              <div className="flex flex-wrap items-start gap-3">
                <span className="flex size-9 flex-none items-center justify-center rounded-md bg-muted text-[11px] font-bold text-muted-foreground uppercase">
                  {ext(r.filename)}
                </span>
                <div className="flex min-w-0 flex-[1_1_280px] flex-col">
                  <span className="truncate font-semibold" title={r.filename}>
                    {r.filename}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {fmtSize(r.size)} · {LABELS[r.purpose as Purpose] ?? r.purpose} · added{" "}
                    {new Date(r.created_at).toLocaleString("en-GB", {
                      dateStyle: "medium",
                      timeStyle: "short",
                    })}
                  </span>
                </div>
                <Chip state={look.state} label={look.label} />
              </div>
              {look.note && (
                <p
                  className={cn(
                    "m-0 rounded-md px-3 py-2.5",
                    r.state === "failed" ? "bg-err-soft text-err" : "bg-muted text-foreground/80",
                  )}
                >
                  {look.note}
                </p>
              )}
              {canDownload && r.state === "ready" && (
                <div>
                  <Button variant="outline" size="lg" onClick={() => download(r.id)} className="press h-9">
                    <DownloadIcon data-icon="inline-start" />
                    Download
                  </Button>
                </div>
              )}
            </article>
          );
        })}
      </section>
    </section>
  );
}

const LABELS: Record<Purpose, string> = {
  dump: "Database dump",
  wxr: "WordPress export",
  "media-archive": "Media archive",
  "sanity-export": "Sanity dataset export",
  other: "Other",
};
const ext = (name: string) => (name.match(/\.([a-z0-9]{1,4})(\.gz)?$/i)?.[1] ?? "file").slice(0, 4);
const fmtSize = (b: number) =>
  b >= 1024 ** 3
    ? `${(b / 1024 ** 3).toFixed(1)} GB`
    : b >= 1024 ** 2
      ? `${(b / 1024 ** 2).toFixed(1)} MB`
      : `${Math.ceil(b / 1024)} KB`;

/** Server states as chips. Ready is green: verified and stored. */
const ROW_LOOK: Record<string, { state: State; label: string; note?: string }> = {
  pending: { state: "not-run", label: "Waiting" },
  uploading: {
    state: "running",
    label: "Uploading",
    note: "Not finished. Pick the same file above to carry on; only the missing parts are sent.",
  },
  complete: { state: "running", label: "Verifying" },
  verifying: {
    state: "running",
    label: "Verifying",
    note: "Checking the copy we received matches the file on your computer.",
  },
  ready: { state: "clean", label: "Ready", note: "Verified and stored. Your Umi team can use it now." },
  failed: {
    state: "failed",
    label: "Failed verification",
    note: "The file we received doesn’t match the one on your computer, so we can’t use it. This usually means the file changed while it was uploading. We’ve deleted the damaged copy. Please upload it again.",
  },
  aborted: { state: "skipped", label: "Cancelled" },
  deleted: { state: "skipped", label: "Deleted" },
};
