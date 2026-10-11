CREATE TABLE reader_local_policy_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1));
CREATE TABLE reader_local_private_v1(path BLOB PRIMARY KEY NOT NULL);
CREATE TABLE reader_local_owned_v1(source_id TEXT PRIMARY KEY NOT NULL REFERENCES sources(id));
CREATE TABLE reader_local_denied_v1(source_id TEXT NOT NULL REFERENCES sources(id),prefix TEXT NOT NULL,PRIMARY KEY(source_id,prefix));
