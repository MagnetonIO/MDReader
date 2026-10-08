CREATE TABLE device_incoming_schema_v1(
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    version INTEGER NOT NULL CHECK(version>0)
);
CREATE TABLE device_incoming_intents_v1(
    id TEXT PRIMARY KEY,
    origin TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    revision TEXT NOT NULL CHECK(length(revision)=64),
    bytes INTEGER NOT NULL CHECK(bytes BETWEEN 0 AND 1048576),
    input_digest TEXT NOT NULL CHECK(length(input_digest)=64),
    parent_identity TEXT NOT NULL,
    root_identity TEXT,
    assets_identity TEXT NOT NULL,
    assets TEXT NOT NULL CHECK(length(assets)<=16384),
    state TEXT NOT NULL CHECK(state IN('reserved','published','dropping','dropped')),
    source_id TEXT
);
INSERT INTO device_incoming_schema_v1(singleton,version) VALUES(1,1);
