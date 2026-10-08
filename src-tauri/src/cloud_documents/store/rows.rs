//! Raw SQLite row decoding; validation remains with the transaction owner.
use rusqlite::Row;

pub(super) struct SnapshotPins {
    pub generation: Vec<u8>,
    pub schema_version_id: Vec<u8>,
    pub mapping_version_id: Vec<u8>,
    pub source_version_id: Vec<u8>,
}

impl SnapshotPins {
    pub fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            generation: row.get(0)?,
            schema_version_id: row.get(1)?,
            mapping_version_id: row.get(2)?,
            source_version_id: row.get(3)?,
        })
    }
}

pub(super) struct CachedBodyKey {
    pub source_id: Vec<u8>,
    pub source_version_id: Vec<u8>,
    pub record_key: Vec<u8>,
    pub revision: Vec<u8>,
    pub size_bytes: i64,
}

impl CachedBodyKey {
    pub fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            source_id: row.get(0)?,
            source_version_id: row.get(1)?,
            record_key: row.get(2)?,
            revision: row.get(3)?,
            size_bytes: row.get(4)?,
        })
    }
}
