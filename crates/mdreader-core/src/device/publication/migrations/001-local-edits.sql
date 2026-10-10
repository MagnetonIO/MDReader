CREATE TABLE device_edit_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);
CREATE TABLE device_edits_v1(operation TEXT PRIMARY KEY CHECK(length(operation)=36),previous TEXT NOT NULL CHECK(length(previous)<=512),revision TEXT NOT NULL CHECK(length(revision)=64),document TEXT NOT NULL CHECK(length(document)=36));
INSERT INTO device_edit_schema_v1(singleton,version) VALUES(1,1);
