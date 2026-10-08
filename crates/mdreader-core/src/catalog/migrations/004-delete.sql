CREATE TRIGGER reader_catalog_delete_v1 AFTER DELETE ON sources BEGIN
DELETE FROM reader_catalog_authorities_v1 WHERE source_id=old.id;
END;
