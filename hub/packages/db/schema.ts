import { relations, sql } from "drizzle-orm";
import {
  bigint,
  boolean,
  foreignKey,
  index,
  integer,
  jsonb,
  pgEnum,
  pgTable,
  primaryKey,
  text,
  timestamp,
  uniqueIndex,
  uuid,
} from "drizzle-orm/pg-core";
import { organization, user } from "./auth-schema";

export * from "./auth-schema";

const id = () => uuid("id").primaryKey().default(sql`gen_random_uuid()`);
const createdAt = () => timestamp("created_at", { withTimezone: true }).defaultNow().notNull();
const updatedAt = () =>
  timestamp("updated_at", { withTimezone: true })
    .defaultNow()
    .$onUpdate(() => new Date())
    .notNull();

export const projectRole = pgEnum("project_role", ["owner", "developer", "viewer", "client"]);
export const runState = pgEnum("run_state", ["queued", "running", "finished", "failed", "cancelled"]);
export const shareScope = pgEnum("share_scope", ["report", "project-readonly"]);
export const uploadState = pgEnum("upload_state", ["pending", "uploading", "complete", "failed", "deleted"]);
export const uploadPurpose = pgEnum("upload_purpose", ["dump", "wxr", "media-archive", "other"]);

export const project = pgTable(
  "project",
  {
    id: id(),
    orgId: text("org_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    slug: text("slug").notNull(),
    /** Human summary, e.g. "WordPress → Payload". */
    summary: text("summary"),
    createdBy: text("created_by").references(() => user.id, { onDelete: "set null" }),
    archivedAt: timestamp("archived_at", { withTimezone: true }),
    retentionDays: integer("retention_days").notNull().default(90),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [uniqueIndex("project_slug_uq").on(t.slug), index("project_org_idx").on(t.orgId)],
);

/** Per-project roles. Org membership alone means staff; this grants access to a project. */
export const projectMember = pgTable(
  "project_member",
  {
    projectId: uuid("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    userId: text("user_id")
      .notNull()
      .references(() => user.id, { onDelete: "cascade" }),
    role: projectRole("role").notNull(),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [primaryKey({ columns: [t.projectId, t.userId] }), index("project_member_user_idx").on(t.userId)],
);

/** Pending per-project invitation by email; accepted on the invitee's first verified sign-in. */
export const projectInvitation = pgTable(
  "project_invitation",
  {
    id: id(),
    projectId: uuid("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    /** Stored lowercased. */
    email: text("email").notNull(),
    role: projectRole("role").notNull().default("client"),
    invitedBy: text("invited_by").references(() => user.id, { onDelete: "set null" }),
    expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
    acceptedAt: timestamp("accepted_at", { withTimezone: true }),
    createdAt: createdAt(),
  },
  (t) => [index("project_invitation_email_idx").on(t.email)],
);

export const projectVersion = pgTable(
  "project_version",
  {
    id: id(),
    projectId: uuid("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    number: integer("number").notNull(),
    /** Object key of the tar.zst snapshot (porter.toml, project.db, raw/, reports/); media is separate. */
    snapshotKey: text("snapshot_key").notNull(),
    porterVersion: text("porter_version"),
    createdBy: text("created_by").references(() => user.id, { onDelete: "set null" }),
    size: bigint("size", { mode: "number" }).notNull().default(0),
    /** Output of `porter status --json`. */
    summary: jsonb("summary"),
    message: text("message"),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [uniqueIndex("project_version_number_uq").on(t.projectId, t.number)],
);

/** Content-addressed media: one row per (project, sha256). */
export const mediaObject = pgTable(
  "media_object",
  {
    projectId: uuid("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    sha256: text("sha256").notNull(),
    size: bigint("size", { mode: "number" }).notNull(),
    contentType: text("content_type"),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [primaryKey({ columns: [t.projectId, t.sha256] })],
);

/** Which media hashes a version references (media_object is per project, not per version). */
export const versionMedia = pgTable(
  "version_media",
  {
    versionId: uuid("version_id")
      .notNull()
      .references(() => projectVersion.id, { onDelete: "cascade" }),
    projectId: uuid("project_id").notNull(),
    sha256: text("sha256").notNull(),
  },
  (t) => [
    primaryKey({ columns: [t.versionId, t.sha256] }),
    foreignKey({
      columns: [t.projectId, t.sha256],
      foreignColumns: [mediaObject.projectId, mediaObject.sha256],
    }).onDelete("cascade"),
  ],
);

/** An init'd push awaiting commit; its id names the snapshot object. Bound to one project and user. */
export const versionUpload = pgTable("version_upload", {
  id: id(),
  projectId: uuid("project_id")
    .notNull()
    .references(() => project.id, { onDelete: "cascade" }),
  userId: text("user_id")
    .notNull()
    .references(() => user.id, { onDelete: "cascade" }),
  /** Every hash the push references, and the subset that was missing at init. */
  mediaHashes: text("media_hashes").array().notNull(),
  missing: text("missing").array().notNull(),
  createdAt: createdAt(),
});

export const run = pgTable(
  "run",
  {
    id: id(),
    projectId: uuid("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    baseVersionId: uuid("base_version_id").references(() => projectVersion.id, { onDelete: "set null" }),
    stages: text("stages").array().notNull().default(sql`'{}'::text[]`),
    apply: boolean("apply").notNull().default(false),
    state: runState("state").notNull().default("queued"),
    exitCode: integer("exit_code"),
    resultVersionId: uuid("result_version_id").references(() => projectVersion.id, { onDelete: "set null" }),
    requestedBy: text("requested_by").references(() => user.id, { onDelete: "set null" }),
    startedAt: timestamp("started_at", { withTimezone: true }),
    finishedAt: timestamp("finished_at", { withTimezone: true }),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [
    index("run_project_idx").on(t.projectId, t.createdAt),
    index("run_state_idx").on(t.state),
    // One active run per project at a time.
    uniqueIndex("run_one_active_per_project").on(t.projectId).where(sql`${t.state} in ('queued','running')`),
  ],
);

export const runEvent = pgTable(
  "run_event",
  {
    runId: uuid("run_id")
      .notNull()
      .references(() => run.id, { onDelete: "cascade" }),
    seq: integer("seq").notNull(),
    event: jsonb("event").notNull(),
    createdAt: createdAt(),
  },
  (t) => [primaryKey({ columns: [t.runId, t.seq] })],
);

/** Only the hash of the token is stored; the raw token is shown once. */
export const shareLink = pgTable(
  "share_link",
  {
    id: id(),
    projectId: uuid("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    tokenHash: text("token_hash").notNull(),
    scope: shareScope("scope").notNull().default("report"),
    expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
    createdBy: text("created_by").references(() => user.id, { onDelete: "set null" }),
    revokedAt: timestamp("revoked_at", { withTimezone: true }),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [
    uniqueIndex("share_link_token_hash_uq").on(t.tokenHash),
    index("share_link_project_idx").on(t.projectId),
  ],
);

export const upload = pgTable(
  "upload",
  {
    id: id(),
    projectId: uuid("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    uploaderId: text("uploader_id").references(() => user.id, { onDelete: "set null" }),
    filename: text("filename").notNull(),
    size: bigint("size", { mode: "number" }),
    sha256: text("sha256"),
    objectKey: text("object_key").notNull(),
    /** S3 multipart upload id while in flight. */
    multipartId: text("multipart_id"),
    state: uploadState("state").notNull().default("pending"),
    purpose: uploadPurpose("purpose").notNull().default("other"),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [index("upload_project_idx").on(t.projectId, t.createdAt)],
);

/** Ciphertext only (AES-256-GCM with HUB_SECRET_KEY); plaintext is never stored or returned. */
export const runSecret = pgTable(
  "run_secret",
  {
    id: id(),
    projectId: uuid("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    ciphertext: text("ciphertext").notNull(),
    nonce: text("nonce").notNull(),
    createdBy: text("created_by").references(() => user.id, { onDelete: "set null" }),
    createdAt: createdAt(),
    updatedAt: updatedAt(),
  },
  (t) => [uniqueIndex("run_secret_name_uq").on(t.projectId, t.name)],
);

export const auditLog = pgTable(
  "audit_log",
  {
    id: id(),
    projectId: uuid("project_id").references(() => project.id, { onDelete: "set null" }),
    actorId: text("actor_id").references(() => user.id, { onDelete: "set null" }),
    /** e.g. upload.create, share.revoke, retention.delete */
    action: text("action").notNull(),
    targetType: text("target_type"),
    targetId: text("target_id"),
    detail: jsonb("detail"),
    createdAt: createdAt(),
  },
  (t) => [
    index("audit_log_project_idx").on(t.projectId, t.createdAt),
    index("audit_log_actor_idx").on(t.actorId),
  ],
);

export const projectRelations = relations(project, ({ many }) => ({
  members: many(projectMember),
  versions: many(projectVersion),
  runs: many(run),
}));
