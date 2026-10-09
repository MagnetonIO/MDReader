CREATE TABLE provider_reading_schema(version INTEGER NOT NULL CHECK(version=1));
INSERT INTO provider_reading_schema VALUES(1);
CREATE TABLE provider_reading_states(
 source_id TEXT NOT NULL REFERENCES sources(id), path TEXT NOT NULL, revision TEXT NOT NULL,
 pinned INTEGER NOT NULL CHECK(pinned IN(0,1)), line INTEGER NOT NULL CHECK(line>=1),
 generation INTEGER NOT NULL CHECK(generation>=0), PRIMARY KEY(source_id,path,revision)
);
