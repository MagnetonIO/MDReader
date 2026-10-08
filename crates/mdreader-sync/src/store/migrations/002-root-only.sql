ALTER TABLE mdreader_sync_graph_roots RENAME TO mdreader_previous_graph_roots;
CREATE TABLE mdreader_sync_graph_roots(account TEXT NOT NULL,library TEXT NOT NULL,device TEXT NOT NULL,operation_id TEXT NOT NULL,root_blob TEXT NOT NULL,dependency_count INTEGER NOT NULL CHECK(dependency_count BETWEEN 0 AND 72),root_receipt TEXT,PRIMARY KEY(account,library,device,operation_id));
INSERT INTO mdreader_sync_graph_roots SELECT account,library,device,operation_id,root_blob,dependency_count,root_receipt FROM mdreader_previous_graph_roots;
DROP TABLE mdreader_previous_graph_roots;
UPDATE mdreader_sync_graph_schema SET version=2 WHERE singleton=1 AND version=1;
