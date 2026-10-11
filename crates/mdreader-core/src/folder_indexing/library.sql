CREATE TABLE reader_folder_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1));
CREATE TABLE reader_folder_preferences_v1(source_id TEXT PRIMARY KEY NOT NULL REFERENCES sources(id),monitor_enabled INTEGER NOT NULL DEFAULT 1 CHECK(monitor_enabled IN(0,1)));
CREATE TABLE reader_folder_events_v1(source_id TEXT NOT NULL REFERENCES sources(id),path TEXT NOT NULL,PRIMARY KEY(source_id,path));
