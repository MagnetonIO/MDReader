CREATE TABLE reader_folder_index_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1));
CREATE TABLE reader_folder_files_v1(source_id TEXT NOT NULL,path TEXT NOT NULL,stamp TEXT NOT NULL,size INTEGER NOT NULL,modified_ms INTEGER NOT NULL,scan TEXT NOT NULL,PRIMARY KEY(source_id,path));
CREATE TABLE reader_folder_scans_v1(source_id TEXT PRIMARY KEY NOT NULL,scan TEXT NOT NULL,generation INTEGER NOT NULL,complete INTEGER NOT NULL CHECK(complete IN(0,1)),observed_at_ms INTEGER);
