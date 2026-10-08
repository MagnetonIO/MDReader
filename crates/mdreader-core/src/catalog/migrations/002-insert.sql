CREATE TRIGGER reader_catalog_insert_v1 AFTER INSERT ON sources BEGIN
INSERT INTO reader_catalog_authorities_v1(source_id,epoch) VALUES(new.id,lower(hex(randomblob(16))));
END;
