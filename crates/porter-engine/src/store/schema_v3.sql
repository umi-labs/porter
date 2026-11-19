-- v2 -> v3: where each loaded item landed in the destination.
CREATE TABLE dest_ids (
    item_id     TEXT PRIMARY KEY,           -- ItemId
    collection  TEXT NOT NULL,
    dest_id     TEXT NOT NULL,              -- JSON (Payload ids are numbers or strings)
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    deleted_at  TEXT
);

PRAGMA user_version = 3;
