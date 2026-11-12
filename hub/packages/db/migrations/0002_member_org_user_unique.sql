-- One membership per (organization, user). Hand-written: Better Auth's generated schema doesn't carry it.
DELETE FROM "member" a USING "member" b
  WHERE a.organization_id = b.organization_id AND a.user_id = b.user_id AND a.ctid > b.ctid;
--> statement-breakpoint
CREATE UNIQUE INDEX "member_org_user_uq" ON "member" ("organization_id", "user_id");
