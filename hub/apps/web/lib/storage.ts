import {
  CreateBucketCommand,
  GetObjectCommand,
  HeadBucketCommand,
  HeadObjectCommand,
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

let client: S3Client | undefined;
const s3 = () =>
  (client ??= new S3Client({
    endpoint: need("S3_ENDPOINT"),
    region: process.env.S3_REGION || "us-east-1",
    forcePathStyle: true, // MinIO
    credentials: { accessKeyId: need("S3_ACCESS_KEY"), secretAccessKey: need("S3_SECRET_KEY") },
    // Only checksum what we ask for; the default CRC32 would make presigned PUTs demand extra headers.
    requestChecksumCalculation: "WHEN_REQUIRED",
    responseChecksumValidation: "WHEN_REQUIRED",
  }));

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

/** Presigned PUT. With `sha256` (hex), S3 itself rejects a body that doesn't hash to it. */
export async function presignPut(key: string, sha256?: string) {
  await ensureBucket();
  return getSignedUrl(
    s3(),
    new PutObjectCommand({
      Bucket: need("S3_BUCKET"),
      Key: key,
      ...(sha256 ? { ChecksumSHA256: hexToBase64(sha256) } : {}),
    }),
    // Keep the checksum a signed *header*: as a query param S3 ignores it. The uploader must send
    // `x-amz-checksum-sha256: <base64 of the hash>` or the signature fails.
    {
      expiresIn: URL_TTL_SECONDS,
      ...(sha256 ? { unhoistableHeaders: new Set(["x-amz-checksum-sha256"]) } : {}),
    },
  );
}

export async function presignGet(key: string) {
  await ensureBucket();
  return getSignedUrl(s3(), new GetObjectCommand({ Bucket: need("S3_BUCKET"), Key: key }), {
    expiresIn: URL_TTL_SECONDS,
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
