-- v5 -> v6: the fingerprint of the porter.toml sections a run used (see fingerprint.rs).
-- NULL for older rows: unknown, which reads as stale.
ALTER TABLE runs ADD COLUMN config_fp TEXT;

PRAGMA user_version = 6;
