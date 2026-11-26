CREATE TABLE "service_key" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"name" text NOT NULL,
	"prefix" text NOT NULL,
	"key_hash" text NOT NULL,
	"user_id" text NOT NULL,
	"created_by" text,
	"last_used_at" timestamp with time zone,
	"revoked_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "service_key_key_hash_unique" UNIQUE("key_hash")
);
--> statement-breakpoint
ALTER TABLE "project" ADD COLUMN "foundry_ref" text;--> statement-breakpoint
ALTER TABLE "project" ADD COLUMN "foundry_url" text;--> statement-breakpoint
ALTER TABLE "service_key" ADD CONSTRAINT "service_key_user_id_user_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."user"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "service_key" ADD CONSTRAINT "service_key_created_by_user_id_fk" FOREIGN KEY ("created_by") REFERENCES "public"."user"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
CREATE UNIQUE INDEX "project_foundry_ref_uq" ON "project" USING btree ("foundry_ref");