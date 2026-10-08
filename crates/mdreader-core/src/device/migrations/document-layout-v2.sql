-- Validation specification for the layout produced by ordered migration 003.
CREATE TABLE device_documents_v1(
 id TEXT PRIMARY KEY, name TEXT NOT NULL, source_id TEXT NOT NULL UNIQUE REFERENCES sources(id),
 revision TEXT NOT NULL, bytes INTEGER NOT NULL CHECK(bytes>=0), pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN(0,1)),
 line INTEGER NOT NULL DEFAULT 1 CHECK(line>0), indexed INTEGER NOT NULL DEFAULT 0 CHECK(indexed IN(0,1)),
 state_generation INTEGER NOT NULL DEFAULT 0 CHECK(typeof(state_generation)='integer' AND state_generation BETWEEN 0 AND 9007199254740991),
 retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN(0,1)),
 retirement TEXT CHECK(retirement IS NULL OR length(CAST(retirement AS BLOB))<=2048)
);
