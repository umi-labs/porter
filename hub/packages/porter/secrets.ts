import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";

/** Env var names a run secret may use. */
export const SECRET_NAME = /^[A-Z][A-Z0-9_]{0,63}$/;
/** Names that would change how porter or the OS behaves rather than pass a credential. */
const RESERVED = new Set([
  "PATH",
  "HOME",
  "LANG",
  "LANGUAGE",
  "SHELL",
  "USER",
  "LOGNAME",
  "PWD",
  "OLDPWD",
  "TMPDIR",
  "TMP",
  "TEMP",
  "IFS",
  "ENV",
  "BASH_ENV",
  "TERM",
  "TZ",
  "HTTP_PROXY",
  "HTTPS_PROXY",
  "ALL_PROXY",
  "NO_PROXY",
  "SSL_CERT_FILE",
  "SSL_CERT_DIR",
]);
const RESERVED_PREFIX = ["NODE_", "LD_", "DYLD_", "LC_", "RUST_", "CARGO_", "MALLOC_", "XDG_"];

/** Null if `name` is fine, else why not. */
export function secretNameProblem(name: string) {
  if (!SECRET_NAME.test(name)) return "must match ^[A-Z][A-Z0-9_]{0,63}$";
  if (RESERVED.has(name) || RESERVED_PREFIX.some((p) => name.startsWith(p)))
    return `${name} is reserved and can't be a run secret`;
  return null;
}

function key() {
  const k = Buffer.from(process.env.HUB_SECRET_KEY ?? "", "base64");
  if (k.length !== 32) throw new Error("HUB_SECRET_KEY must be 32 bytes, base64 (openssl rand -base64 32)");
  return k;
}
/** Binds the ciphertext to its row, so a value can't be moved to another project or name. */
const aad = (projectId: string, name: string) => Buffer.from(`${projectId}\0${name}`);

/** AES-256-GCM under HUB_SECRET_KEY. `ciphertext` is base64(ct || 16-byte tag), `nonce` base64 (12 bytes). */
export function encryptSecret(projectId: string, name: string, value: string) {
  const nonce = randomBytes(12);
  const c = createCipheriv("aes-256-gcm", key(), nonce).setAAD(aad(projectId, name));
  const ct = Buffer.concat([c.update(value, "utf8"), c.final(), c.getAuthTag()]);
  return { ciphertext: ct.toString("base64"), nonce: nonce.toString("base64") };
}

export function decryptSecret(projectId: string, name: string, row: { ciphertext: string; nonce: string }) {
  const ct = Buffer.from(row.ciphertext, "base64");
  const d = createDecipheriv("aes-256-gcm", key(), Buffer.from(row.nonce, "base64")).setAAD(
    aad(projectId, name),
  );
  d.setAuthTag(ct.subarray(ct.length - 16));
  return Buffer.concat([d.update(ct.subarray(0, ct.length - 16)), d.final()]).toString("utf8");
}
