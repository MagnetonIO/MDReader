ALTER TABLE native_incoming_items_v1 RENAME TO native_incoming_items_upgrade_v1;
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
 PRIMARY KEY(scope,wire_revision), UNIQUE(scope,sequence),
 CHECK((state='reserved' AND receipt IS NULL) OR (state='applied' AND receipt IS NOT NULL AND document_id IS NOT NULL))
);
INSERT INTO native_incoming_items_v1 SELECT * FROM native_incoming_items_upgrade_v1;
DROP TABLE native_incoming_items_upgrade_v1;
ALTER TABLE native_incoming_schema_v1 RENAME TO native_incoming_schema_upgrade_v1;
CREATE TABLE native_incoming_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=2),remote_library TEXT NOT NULL CHECK(length(remote_library)=36));
INSERT INTO native_incoming_schema_v1 SELECT singleton,2,remote_library FROM native_incoming_schema_upgrade_v1;
DROP TABLE native_incoming_schema_upgrade_v1;
CREATE TABLE native_incoming_heads_v2(
 scope TEXT NOT NULL CHECK(length(scope)=64), wire_item TEXT NOT NULL CHECK(length(wire_item)=36),
 kind TEXT NOT NULL CHECK(kind IN('content','user_state')), head_revision TEXT NOT NULL CHECK(length(head_revision)=36),
 document_id TEXT NOT NULL CHECK(length(document_id)=36), target_item TEXT CHECK(length(target_item)=36),
 baseline_generation INTEGER CHECK(baseline_generation BETWEEN 0 AND 9007199254740991),
 tombstone_revision TEXT CHECK(length(tombstone_revision)=36),
 PRIMARY KEY(scope,wire_item),
 CHECK((kind='content' AND target_item IS NULL) OR (kind='user_state' AND target_item IS NOT NULL))
);
