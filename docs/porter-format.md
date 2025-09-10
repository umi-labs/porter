# Porter Format (Spec Stub)

This document describes the normalized intermediate representation ("porter format") used by Porter to make sources consistent before targeting specific outputs.

## Goals
- Consistent structure across sources (WordPress API/WXR, Umbraco JSON, etc.)
- Stable field names and semantics to reduce mapping churn
- Friendly to transformation and validation

## Document Shape (per collection)
- id: string | number
- type: string (e.g., "post", "page", "media")
- slug: string | null
- title: string | null
- content: string | null (rich HTML or plain)
- excerpt: string | null
- status: string | null (e.g., publish, draft)
- created_at: string (ISO8601) | null
- updated_at: string (ISO8601) | null
- author: string | number | null
- categories: number[] | string[] (IDs or slugs)
- tags: number[] | string[] (IDs or slugs)
- featured_media: number | string | null
- media?: {
  - url: string
  - filename: string
  - mime_type: string
  - width?: number
  - height?: number
  - filesize?: number
}
- meta?: object (source-specific metadata)
- acf?: object (source-specific custom fields)

Notes:
- Additional fields are allowed; unknowns should be preserved under `meta`/`acf`.
- Dates normalized to ISO8601 where possible.
- Coordinates expressed as GeoJSON when combined into a point:
```
{ "type": "Point", "coordinates": [lng, lat] }
```

## File Layout
- One JSON file per collection under the configured output directory
- Array of documents: `[ { ... }, { ... } ]`
- Deterministic ordering for re-runs

## Versioning
- Include `porter_format_version` in file metadata when embedding in envelopes later (TBD)

## Next Steps
- Finalize per-collection field catalogs (pages, posts, media)
- Add validation schema (JSON Schema)
- Document transform catalog used during mapping

