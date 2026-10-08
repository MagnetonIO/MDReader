CREATE TABLE reader_catalog_index_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);
CREATE TABLE reader_catalog_snapshots_v1(source_id TEXT PRIMARY KEY,snapshot TEXT NOT NULL CHECK(length(snapshot)=32),indexed_at_ms INTEGER);
