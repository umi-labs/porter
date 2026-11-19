-- v4 -> v5: the content tree reads documents by parent and by type.
CREATE INDEX items_doc_parent ON items(json_extract(data, '$.parent')) WHERE kind = 'document';
CREATE INDEX items_doc_type ON items(json_extract(data, '$.type')) WHERE kind = 'document';

PRAGMA user_version = 5;
