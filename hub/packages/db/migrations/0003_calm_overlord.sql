CREATE TABLE "version_media" (
	"version_id" uuid NOT NULL,
	"project_id" uuid NOT NULL,
	"sha256" text NOT NULL,
	CONSTRAINT "version_media_version_id_sha256_pk" PRIMARY KEY("version_id","sha256")
);
--> statement-breakpoint
CREATE TABLE "version_upload" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"project_id" uuid NOT NULL,
	"user_id" text NOT NULL,
	"media_hashes" text[] NOT NULL,
	"missing" text[] NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "version_media" ADD CONSTRAINT "version_media_version_id_project_version_id_fk" FOREIGN KEY ("version_id") REFERENCES "public"."project_version"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "version_media" ADD CONSTRAINT "version_media_project_id_sha256_media_object_project_id_sha256_fk" FOREIGN KEY ("project_id","sha256") REFERENCES "public"."media_object"("project_id","sha256") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "version_upload" ADD CONSTRAINT "version_upload_project_id_project_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."project"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "version_upload" ADD CONSTRAINT "version_upload_user_id_user_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."user"("id") ON DELETE cascade ON UPDATE no action;