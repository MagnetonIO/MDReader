CREATE TABLE device_windows_body_schema_v1(
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    version INTEGER NOT NULL CHECK(version=1));
CREATE TABLE device_windows_bodies_v1(
    id TEXT PRIMARY KEY REFERENCES device_import_intents_v1(id),
    revision TEXT NOT NULL CHECK(length(revision)=64),
    body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<=1048576));
INSERT INTO device_windows_body_schema_v1 VALUES(1,1);
