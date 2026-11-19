-- Schema version 1. Bump PRAGMA user_version and add a migration step in
-- store/mod.rs for any change.
CREATE TABLE items (
    id          TEXT PRIMARY KEY,           -- ItemId (UUIDv5)
    kind        TEXT NOT NULL,
    connector   TEXT NOT NULL,
    source_id   TEXT NOT NULL,
    url         TEXT,
    data        TEXT NOT NULL,              -- JSON
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    deleted_at  TEXT
);
CREATE INDEX items_kind ON items(kind);

CREATE TABLE statuses (
    stage       TEXT NOT NULL,
    item_id     TEXT NOT NULL,
    run_id      TEXT NOT NULL,
    status      TEXT NOT NULL,              -- JSON ItemStatus
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    deleted_at  TEXT,
    PRIMARY KEY (stage, item_id)
);

CREATE TABLE issues (
    id          TEXT PRIMARY KEY,           -- UUIDv7
    run_id      TEXT NOT NULL,              -- last run that opened it
    kind        TEXT NOT NULL,
    severity    TEXT NOT NULL,
    item_id     TEXT NOT NULL DEFAULT '',   -- '' when the issue has no item
    stage       TEXT NOT NULL,
    message     TEXT NOT NULL,
    details     TEXT NOT NULL,              -- JSON
    resolution  TEXT NOT NULL,              -- wire tag, for filtering
    resolution_json TEXT NOT NULL,          -- full Resolution (keeps the note)
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    deleted_at  TEXT,
    UNIQUE (kind, item_id, stage, message)
);
CREATE INDEX issues_stage ON issues(stage, resolution);

CREATE TABLE runs (
    id          TEXT PRIMARY KEY,           -- UUIDv7
    stage       TEXT NOT NULL,
    porter_version TEXT NOT NULL,
    started_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    ended_at    TEXT,
    summary     TEXT,                       -- JSON Summary
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    deleted_at  TEXT
);

PRAGMA user_version = 1;
