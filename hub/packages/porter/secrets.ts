import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";

/** Env var names a run secret may use. */
export const SECRET_NAME = /^[A-Z][A-Z0-9_]{0,63}$/;
/**
 * Names that would change how porter, the loader, libc or TLS behave rather than pass a credential.
 * Run secrets are still arbitrary env names otherwise: `api_key_env` can name anything.
 */
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
  "SSLKEYLOGFILE",
  "OPENSSL_CONF",
  "GCONV_PATH",
  "GLIBC_TUNABLES",
  "HOSTALIASES",
  "RES_OPTIONS",
  "LOCALDOMAIN",
  "LOCPATH",
  "NLSPATH",
  "MALLOC_CHECK_",
  "ZDOTDIR",
]);
const RESERVED_PREFIX = [
  "NODE_",
  "LD_",
  "DYLD_",
  "LC_",
  "RUST_",
  "CARGO_",
  "MALLOC_",
  "XDG_",
  "SSL_",
  "OPENSSL_",
  "GCONV_",
  "GLIBC_",
  "GIT_",
  "PYTHON",
  "PERL",
  "RUBY",
  "JAVA_",
  "CURL_",
  "REQUESTS_",
];
/** HTTP_PROXY, HTTPS_PROXY, ALL_PROXY, NO_PROXY, FTP_PROXY, ... */
const RESERVED_SUFFIX = ["_PROXY"];

/** Null if `name` is fine, else why not. */
export function secretNameProblem(name: string) {
  if (!SECRET_NAME.test(name)) return "must match ^[A-Z][A-Z0-9_]{0,63}$";
  if (
    RESERVED.has(name) ||
    RESERVED_PREFIX.some((p) => name.startsWith(p)) ||
    RESERVED_SUFFIX.some((x) => name.endsWith(x))
  )
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

/** Values shorter than this aren't redacted: they'd match ordinary text. */
export const REDACT_MIN = 6;

/**
 * Replaces every secret value (>= 6 chars) in `text` with `[secret:NAME]`, also in its JSON-escaped form, so
 * nothing porter echoes (stdout events, stderr, error messages) stores a value. Longest values first.
 */
export function redactor(secrets: Record<string, string>) {
  const pairs = Object.entries(secrets)
    .filter(([, v]) => v.length >= REDACT_MIN)
    .sort((a, b) => b[1].length - a[1].length)
    .flatMap(([name, v]) => {
      const escaped = JSON.stringify(v).slice(1, -1);
      return escaped === v
        ? [[v, name]]
        : [
            [v, name],
            [escaped, name],
          ];
    });
  return (text: string) => {
    let out = text;
    for (const [v, name] of pairs) out = out.split(v).join(`[secret:${name}]`);
    return out;
  };
}
