CREATE INDEX "project_invitation_project_idx" ON "project_invitation" USING btree ("project_id");--> statement-breakpoint
CREATE INDEX "run_base_version_idx" ON "run" USING btree ("base_version_id");--> statement-breakpoint
CREATE INDEX "run_result_version_idx" ON "run" USING btree ("result_version_id");--> statement-breakpoint
CREATE INDEX "version_media_media_idx" ON "version_media" USING btree ("project_id","sha256");--> statement-breakpoint
CREATE INDEX "version_upload_project_idx" ON "version_upload" USING btree ("project_id");