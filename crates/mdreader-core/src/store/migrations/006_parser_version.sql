ALTER TABLE documents ADD COLUMN parser_version INTEGER NOT NULL DEFAULT 1;
CREATE INDEX documents_parser_version ON documents(parser_version, source_id);
PRAGMA user_version=6;
