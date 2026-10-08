CREATE TABLE mdreader_review_owner(
    id INTEGER PRIMARY KEY CHECK(id=1),
    version INTEGER NOT NULL,
    owner_uuid TEXT NOT NULL
);
