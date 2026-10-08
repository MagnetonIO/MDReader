CREATE TABLE profile_import_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1));
INSERT INTO profile_import_schema_v1 VALUES(1,1);
CREATE TABLE profile_import_receipts_v1(operation TEXT PRIMARY KEY CHECK(length(operation)=36),receipt TEXT NOT NULL CHECK(length(CAST(receipt AS BLOB))<=262144),digest TEXT NOT NULL CHECK(length(digest)=64));
