ALTER TABLE "version_media" DROP CONSTRAINT "version_media_project_id_sha256_media_object_project_id_sha256_fk";
--> statement-breakpoint
ALTER TABLE "project_version" ADD COLUMN "snapshot_sha256" text;--> statement-breakpoint
ALTER TABLE "version_upload" ADD COLUMN "base_version" integer;--> statement-breakpoint
ALTER TABLE "version_upload" ADD COLUMN "snapshot_sha256" text NOT NULL;--> statement-breakpoint
ALTER TABLE "version_media" ADD CONSTRAINT "version_media_project_id_sha256_media_object_project_id_sha256_fk" FOREIGN KEY ("project_id","sha256") REFERENCES "public"."media_object"("project_id","sha256") ON DELETE no action ON UPDATE no action;