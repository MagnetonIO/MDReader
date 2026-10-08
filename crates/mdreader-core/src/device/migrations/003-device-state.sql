ALTER TABLE device_documents_v1 ADD COLUMN state_generation INTEGER NOT NULL DEFAULT 0 CHECK(typeof(state_generation)='integer' AND state_generation BETWEEN 0 AND 9007199254740991);
ALTER TABLE device_documents_v1 ADD COLUMN retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN(0,1));
ALTER TABLE device_documents_v1 ADD COLUMN retirement TEXT CHECK(retirement IS NULL OR length(CAST(retirement AS BLOB))<=2048);
UPDATE device_document_schema_v1 SET version=2 WHERE singleton=1 AND version=1;
