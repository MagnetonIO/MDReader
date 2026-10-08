CREATE TABLE reader_catalog_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);
CREATE TABLE reader_catalog_authorities_v1(source_id TEXT PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,epoch TEXT NOT NULL CHECK(length(epoch)=32));
