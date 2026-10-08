CREATE TABLE native_incoming_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),remote_library TEXT NOT NULL CHECK(length(remote_library)=36));
CREATE TABLE native_incoming_items_v1(
 scope TEXT NOT NULL CHECK(length(scope)=64),
 wire_revision TEXT NOT NULL CHECK(length(wire_revision)=36),
 wire_item TEXT NOT NULL CHECK(length(wire_item)=36),
 sequence INTEGER NOT NULL CHECK(sequence>0),
 identity TEXT NOT NULL CHECK(length(identity)<=16384),
 origin TEXT NOT NULL CHECK(length(origin)<=1024),
 document_id TEXT CHECK(length(document_id)=36),
 state TEXT NOT NULL CHECK(state IN('reserved','applied')),
 receipt TEXT CHECK(length(receipt)<=32768),
 PRIMARY KEY(scope,wire_revision), UNIQUE(scope,wire_item),
 CHECK((state='reserved' AND receipt IS NULL) OR (state='applied' AND receipt IS NOT NULL AND document_id IS NOT NULL))
);
