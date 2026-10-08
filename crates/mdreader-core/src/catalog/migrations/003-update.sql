CREATE TRIGGER reader_catalog_update_v1 AFTER UPDATE OF name,enabled,path,identity ON sources BEGIN
INSERT INTO reader_catalog_authorities_v1(source_id,epoch) VALUES(new.id,lower(hex(randomblob(16)))) ON CONFLICT(source_id) DO UPDATE SET epoch=excluded.epoch;
END;
