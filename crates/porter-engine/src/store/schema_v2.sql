-- v1 -> v2: media URL variants. Media items themselves live in `items`.
CREATE TABLE media_variants (
    url_key     TEXT PRIMARY KEY,           -- normalised URL
    url         TEXT NOT NULL,
    media_id    TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    deleted_at  TEXT
);

PRAGMA user_version = 2;
