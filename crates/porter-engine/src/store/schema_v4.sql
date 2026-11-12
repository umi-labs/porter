-- v3 -> v4: whether a load run was a dry run (nothing written to the destination).
ALTER TABLE runs ADD COLUMN dry_run INTEGER NOT NULL DEFAULT 0;

PRAGMA user_version = 4;
