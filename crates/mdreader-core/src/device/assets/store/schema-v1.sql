CREATE TABLE IF NOT EXISTS device_asset_generations_v1 (
    document_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(typeof(generation)='integer' AND generation>=0),
    PRIMARY KEY(document_id,revision)
);
CREATE TABLE IF NOT EXISTS device_asset_items_v1 (
    id TEXT PRIMARY KEY,
    document_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    target TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes BETWEEN 1 AND 4194304),
    received INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL CHECK(state IN('reserved','ready','retired')),
    expected_manifest TEXT NOT NULL,
    hash TEXT,
    mime TEXT,
    width INTEGER,
    height INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS device_asset_active_target_v1
    ON device_asset_items_v1(document_id,revision,target) WHERE state='ready';
CREATE UNIQUE INDEX IF NOT EXISTS device_asset_pending_target_v1
    ON device_asset_items_v1(document_id,revision,target) WHERE state='reserved';
CREATE INDEX IF NOT EXISTS device_asset_document_v1
    ON device_asset_items_v1(document_id,revision,state);
