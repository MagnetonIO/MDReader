CREATE TABLE device_asset_windows_schema_v1 (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    version INTEGER NOT NULL CHECK(version=1)
);
CREATE TABLE device_asset_windows_bytes_v1 (
    id TEXT PRIMARY KEY REFERENCES device_asset_items_v1(id),
    body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<=4194304)
);
INSERT INTO device_asset_windows_schema_v1(singleton,version) VALUES(1,1);
