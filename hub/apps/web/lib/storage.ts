import {
  CreateBucketCommand,
  GetObjectCommand,
  HeadBucketCommand,
  HeadObjectCommand,
  ListObjectsV2Command,
  PutObjectCommand,
  S3Client,
} from "@aws-sdk/client-s3";
import { getSignedUrl } from "@aws-sdk/s3-request-presigner";

/** Presigned URLs live 15 minutes. */
export const URL_TTL_SECONDS = 15 * 60;

export const keys = {
  snapshot: (projectId: string, uploadId: string) => `projects/${projectId}/snapshots/${uploadId}.tar.zst`,
  media: (projectId: string, sha256: string) => `projects/${projectId}/media/${sha256}`,
  /** Everything a project owns, so retention can delete one project in full. */
  projectPrefix: (projectId: string) => `projects/${projectId}/`,
};

const need = (name: string) => {
  const v = process.env[name];
  if (!v) throw new Error(`${name} is not set`);
  return v;
};

const make = (endpoint: string) =>
  new S3Client({
    endpoint,
    region: process.env.S3_REGION || "us-east-1",
    forcePathStyle: true, // MinIO
    credentials: { accessKeyId: need("S3_ACCESS_KEY"), secretAccessKey: need("S3_SECRET_KEY") },
    // Only checksum what we ask for; the default CRC32 would make presigned PUTs demand extra headers.
    requestChecksumCalculation: "WHEN_REQUIRED",
    responseChecksumValidation: "WHEN_REQUIRED",
  });
let client: S3Client | undefined;
let publicClient: S3Client | undefined;
/** Server-side calls (HEAD, list, CreateBucket) use the internal endpoint. */
const s3 = () => (client ??= make(need("S3_ENDPOINT")));
/** Presigning uses the endpoint clients can reach: the host is part of the signature. */
const s3Public = () => (publicClient ??= make(process.env.S3_PUBLIC_ENDPOINT || need("S3_ENDPOINT")));

let bucketReady: Promise<void> | undefined;
/** Creates the bucket if missing, once per process. */
export function ensureBucket() {
  bucketReady ??= (async () => {
    const Bucket = need("S3_BUCKET");
    try {
      await s3().send(new HeadBucketCommand({ Bucket }));
    } catch {
      try {
        await s3().send(new CreateBucketCommand({ Bucket }));
      } catch (e) {
        // A concurrent creator is fine.
        const name = (e as { name?: string }).name;
        if (name !== "BucketAlreadyOwnedByYou" && name !== "BucketAlreadyExists") throw e;
      }
    }
  })().catch((e) => {
    bucketReady = undefined;
    throw e;
  });
  return bucketReady;
}

export const hexToBase64 = (hex: string) => Buffer.from(hex, "hex").toString("base64");

/**
 * Presigned PUT. S3 itself rejects a body that doesn't hash to `sha256` (hex), and refuses to overwrite an
 * existing object (412). Both are signed *headers*, which the uploader must send:
 * `x-amz-checksum-sha256: <base64 of the hash>` and `if-none-match: *`. As query params S3 ignores them.
 */
export async function presignPut(key: string, sha256: string) {
  await ensureBucket();
  return getSignedUrl(
    s3Public(),
    new PutObjectCommand({
      Bucket: need("S3_BUCKET"),
      Key: key,
      IfNoneMatch: "*",
      ChecksumSHA256: hexToBase64(sha256),
    }),
    { expiresIn: URL_TTL_SECONDS, unhoistableHeaders: new Set(["x-amz-checksum-sha256", "if-none-match"]) },
  );
}

export async function presignGet(key: string, expiresIn = URL_TTL_SECONDS) {
  await ensureBucket();
  return getSignedUrl(s3Public(), new GetObjectCommand({ Bucket: need("S3_BUCKET"), Key: key }), {
    expiresIn,
  });
}

/** Size (and stored SHA-256, base64, if any) of an object, or null if it doesn't exist. */
export async function head(key: string) {
  await ensureBucket();
  try {
    const r = await s3().send(
      new HeadObjectCommand({ Bucket: need("S3_BUCKET"), Key: key, ChecksumMode: "ENABLED" }),
    );
    return { size: r.ContentLength ?? 0, checksumSha256: r.ChecksumSHA256, contentType: r.ContentType };
  } catch (e) {
    const x = e as { name?: string; $metadata?: { httpStatusCode?: number } };
    if (x.name === "NotFound" || x.$metadata?.httpStatusCode === 404) return null;
    throw e;
  }
}

/** Sizes of every object under `prefix`, keyed by the name after the prefix (1000 keys per S3 call). */
export async function listSizes(prefix: string) {
  await ensureBucket();
  const out = new Map<string, number>();
  let token: string | undefined;
  do {
    const r = await s3().send(
      new ListObjectsV2Command({ Bucket: need("S3_BUCKET"), Prefix: prefix, ContinuationToken: token }),
    );
    for (const o of r.Contents ?? []) if (o.Key) out.set(o.Key.slice(prefix.length), o.Size ?? 0);
    token = r.NextContinuationToken;
  } while (token);
  return out;
}
