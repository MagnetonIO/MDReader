CREATE TABLE reader_editor_draft_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1));
INSERT INTO reader_editor_draft_schema_v1 VALUES(1,1);
CREATE TABLE reader_editor_drafts_v1(draft_id TEXT PRIMARY KEY,key_json TEXT NOT NULL UNIQUE,generation INTEGER NOT NULL CHECK(generation>=0 AND generation<=9007199254740991),body TEXT,metadata TEXT,CHECK((body IS NULL AND metadata IS NULL) OR (body IS NOT NULL AND metadata IS NOT NULL)));
