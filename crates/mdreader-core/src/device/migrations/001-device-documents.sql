CREATE TABLE IF NOT EXISTS device_documents_v1(
        id TEXT PRIMARY KEY, name TEXT NOT NULL, source_id TEXT NOT NULL UNIQUE REFERENCES sources(id),
        revision TEXT NOT NULL, bytes INTEGER NOT NULL CHECK(bytes>=0), pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN(0,1)),
        line INTEGER NOT NULL DEFAULT 1 CHECK(line>0), indexed INTEGER NOT NULL DEFAULT 0 CHECK(indexed IN(0,1)));
        CREATE TABLE IF NOT EXISTS device_import_intents_v1(
        id TEXT PRIMARY KEY, name TEXT NOT NULL, revision TEXT NOT NULL CHECK(length(revision)=64),
        bytes INTEGER NOT NULL CHECK(bytes BETWEEN 0 AND 1048576), parent_identity TEXT NOT NULL,
        root_identity TEXT, state TEXT NOT NULL CHECK(state IN('reserved','published')));
