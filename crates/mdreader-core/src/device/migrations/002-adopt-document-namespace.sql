CREATE TABLE device_document_schema_v1 (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    version INTEGER NOT NULL CHECK(version>0)
);
INSERT INTO device_document_schema_v1(singleton,version) VALUES(1,1);
