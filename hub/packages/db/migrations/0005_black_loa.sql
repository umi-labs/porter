ALTER TYPE "public"."upload_state" ADD VALUE 'verifying';--> statement-breakpoint
ALTER TYPE "public"."upload_state" ADD VALUE 'ready';--> statement-breakpoint
ALTER TYPE "public"."upload_state" ADD VALUE 'aborted';