use super::*;
use crate::{hash_file, hex, records, unixfs, Conversion, KnownRecord, Member, Role};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

const DATABASE: &str = "reviews.sqlite";
const FORMAT: &str = "mdreader-archived-reviews";
const MAX_REPLY: usize = 4 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageManifest {
    format: String,
    version: u8,
    archive: ArchiveIdentity,
    database: Member,
    records: u64,
    unavailable_artifacts: usize,
}
const SCHEMA: &str = "
CREATE TABLE legacy_review_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1));
INSERT INTO legacy_review_schema_v1 VALUES(1,1);
CREATE TABLE legacy_review_imports_v1(export_id TEXT PRIMARY KEY,manifest_sha256 TEXT NOT NULL UNIQUE,state TEXT NOT NULL CHECK(state='staged'),record_count INTEGER NOT NULL);
CREATE TABLE legacy_review_artifacts_v1(artifact_index INTEGER PRIMARY KEY,metadata_json BLOB NOT NULL,availability TEXT NOT NULL);
CREATE TABLE legacy_review_records_v1(record_id TEXT PRIMARY KEY,artifact_index INTEGER NOT NULL,legacy_rowid INTEGER NOT NULL,raw_json BLOB NOT NULL,row_sha256 TEXT NOT NULL,dto_json BLOB NOT NULL,UNIQUE(artifact_index,legacy_rowid),FOREIGN KEY(artifact_index) REFERENCES legacy_review_artifacts_v1(artifact_index));
CREATE INDEX legacy_review_order_v1 ON legacy_review_records_v1(artifact_index,legacy_rowid);
";

fn identity_valid(identity: &ArchiveIdentity) -> bool {
    identity.export_id.get_version_num() == 4
        && identity.manifest_sha256.len() == 64
        && identity
            .manifest_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
}
fn verify_input(
    path: &Path,
    expected: &ArchiveIdentity,
    limits: &Limits,
) -> Result<crate::Manifest> {
    if !identity_valid(expected) {
        return Err(Error::Invalid);
    }
    let mut descriptor = unixfs::Source::open(&path.join("manifest.json"), false)?;
    descriptor.validate(&expected.manifest_sha256, limits.max_manifest_bytes as u64)?;
    let manifest = crate::verify(path, limits)?;
    if manifest.export_id != expected.export_id {
        return Err(Error::Conflict);
    }
    descriptor.validate(&expected.manifest_sha256, limits.max_manifest_bytes as u64)?;
    Ok(manifest)
}
pub fn stage_reviews(
    archive: &Path,
    expected: &ArchiveIdentity,
    destination: &Path,
    limits: &Limits,
) -> Result<StageReceipt> {
    stage_inner(archive, expected, destination, limits, || Ok(()))
}
pub(super) fn stage_inner(
    archive: &Path,
    expected: &ArchiveIdentity,
    destination: &Path,
    limits: &Limits,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<StageReceipt> {
    limits.validate()?;
    unixfs::valid_destination(destination)?;
    let manifest = verify_input(archive, expected, limits)?;
    if destination.try_exists()? {
        let existing = ReviewArchive::open(destination, expected, limits)?;
        return Ok(StageReceipt {
            archive: expected.clone(),
            records: existing.manifest.records,
            unavailable_artifacts: existing.manifest.unavailable_artifacts,
            replayed: true,
        });
    }
    let stage = unixfs::Stage::create(destination)?;
    let mut db_file = stage.create_file(DATABASE)?;
    let mut connection = Connection::open_with_flags(
        stage.path.join(DATABASE),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let database_bound = limits
        .max_records_bytes
        .min(limits.max_artifact_bytes)
        .min(limits.max_total_bytes);
    if database_bound < 4096 {
        return Err(Error::Bounds);
    }
    connection.execute_batch(&format!("PRAGMA page_size=4096; PRAGMA max_page_count={}; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;",database_bound/4096))?;
    connection.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        (limits.max_row_bytes.saturating_mul(2)).min(i32::MAX as usize) as i32,
    )?;
    let tx = connection.transaction()?;
    tx.execute_batch(SCHEMA)?;
    let mut count = 0u64;
    let mut unavailable = 0usize;
    for (index, artifact) in manifest.artifacts.iter().enumerate() {
        let available = artifact.role == Role::Proposals
            && matches!(artifact.conversion, Conversion::Known { .. });
        if !available {
            unavailable += 1
        }
        tx.execute(
            "INSERT INTO legacy_review_artifacts_v1 VALUES(?1,?2,?3)",
            params![
                index as i64,
                serde_json::to_vec(artifact)?,
                if available { "imported" } else { "unavailable" }
            ],
        )?;
        if !available {
            continue;
        }
        let records_name = artifact.records.as_ref().ok_or(Error::Integrity)?;
        let records_member = manifest
            .members
            .iter()
            .find(|m| &m.name == records_name)
            .ok_or(Error::Integrity)?;
        let raw_member = manifest
            .members
            .iter()
            .find(|m| m.name == artifact.raw)
            .ok_or(Error::Integrity)?;
        let mut source = unixfs::Source::open(&archive.join(&artifact.raw), true)?;
        source.validate(&raw_member.sha256, limits.max_artifact_bytes)?;
        let check_name = format!("check-{index:04}.jsonl");
        let mut checked = stage.create_file(&check_name)?;
        let converted = records::convert(
            Role::Proposals,
            &archive.join(&artifact.raw),
            &mut checked,
            limits,
        )?;
        let expected_count = match (&converted, &artifact.conversion) {
            (
                records::Converted::Known { rows, .. },
                Conversion::Known {
                    rows: expected_rows,
                },
            ) if rows == expected_rows => *rows,
            _ => return Err(Error::Integrity),
        };
        let (bytes, hash) = hash_file(&mut checked, limits.max_records_bytes)?;
        if bytes != records_member.bytes || hash != records_member.sha256 {
            return Err(Error::Integrity);
        }
        source.validate(&raw_member.sha256, limits.max_artifact_bytes)?;
        checked.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(checked);
        let mut imported = 0;
        loop {
            let mut line = Vec::new();
            let n = Read::by_ref(&mut reader)
                .take(limits.max_row_bytes as u64 + 2)
                .read_until(b'\n', &mut line)?;
            if n == 0 {
                break;
            }
            if n > limits.max_row_bytes + 1 || line.pop() != Some(b'\n') {
                return Err(Error::Bounds);
            }
            let KnownRecord::Proposal { row_id, fields } = serde_json::from_slice(&line)? else {
                return Err(Error::Integrity);
            };
            count += 1;
            imported += 1;
            if count > limits.max_rows {
                return Err(Error::Bounds);
            }
            let row_hash = hex(&Sha256::digest(&line));
            let record_id = hex(&Sha256::digest(serde_json::to_vec(&(
                expected, index, row_id,
            ))?));
            let raw = RawReference {
                archive: expected.clone(),
                artifact_index: index,
                raw_member: artifact.raw.clone(),
                raw_sha256: raw_member.sha256.clone(),
                records_sha256: records_member.sha256.clone(),
                row_id: row_id.to_string(),
                row_sha256: row_hash.clone(),
            };
            let dto = interpret(&fields, raw, record_id.clone());
            let dto_bytes = serde_json::to_vec(&dto)?;
            if dto_bytes.len() > MAX_REPLY {
                return Err(Error::Bounds);
            }
            tx.execute(
                "INSERT INTO legacy_review_records_v1 VALUES(?1,?2,?3,?4,?5,?6)",
                params![record_id, index as i64, row_id, line, row_hash, dto_bytes],
            )?;
            checkpoint()?;
        }
        if imported != expected_count {
            return Err(Error::Integrity);
        }
        drop(reader);
        stage.remove_file(&check_name)?;
    }
    tx.execute(
        "INSERT INTO legacy_review_imports_v1 VALUES(?1,?2,'staged',?3)",
        params![
            expected.export_id.to_string(),
            expected.manifest_sha256,
            count
        ],
    )?;
    // Bind all admissions to the still-verified selected archive before commit.
    verify_input(archive, expected, limits)?;
    tx.commit()?;
    drop(connection);
    db_file.sync_all()?;
    let (bytes, sha256) = hash_file(&mut db_file, database_bound)?;
    let staged = StageManifest {
        format: FORMAT.into(),
        version: 1,
        archive: expected.clone(),
        database: Member {
            name: DATABASE.into(),
            bytes,
            sha256,
        },
        records: count,
        unavailable_artifacts: unavailable,
    };
    let encoded = serde_json::to_vec_pretty(&staged)?;
    if encoded.len() > limits.max_manifest_bytes
        || bytes + encoded.len() as u64 > limits.max_total_bytes
    {
        return Err(Error::Bounds);
    }
    let mut file = stage.create_file("manifest.json")?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    stage.sync()?;
    checkpoint()?;
    // Validate receipt, schema and read path before publishing; hidden stage name
    // is permitted only by this private helper, never by public open.
    drop(open_inner(&stage.path, expected, limits, false)?);
    stage.publish()?;
    Ok(StageReceipt {
        archive: expected.clone(),
        records: count,
        unavailable_artifacts: unavailable,
        replayed: false,
    })
}
pub struct ReviewArchive {
    connection: Connection,
    directory: File,
    database: unixfs::Source,
    path: PathBuf,
    manifest: StageManifest,
    limits: Limits,
}
fn open_inner(
    path: &Path,
    expected: &ArchiveIdentity,
    limits: &Limits,
    completed: bool,
) -> Result<ReviewArchive> {
    limits.validate()?;
    if !identity_valid(expected) {
        return Err(Error::Invalid);
    }
    if completed {
        unixfs::valid_destination(path)?
    }
    let directory = unixfs::private_directory(path)?;
    let mut file = unixfs::open_file(&directory, "manifest.json")?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(limits.max_manifest_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limits.max_manifest_bytes {
        return Err(Error::Bounds);
    }
    let manifest: StageManifest = serde_json::from_slice(&bytes)?;
    if manifest.format != FORMAT
        || manifest.version != 1
        || manifest.archive != *expected
        || manifest.database.name != DATABASE
        || manifest.records > limits.max_rows
        || manifest.unavailable_artifacts > limits.max_artifacts
    {
        return Err(Error::Conflict);
    }
    let mut db = unixfs::open_file(&directory, DATABASE)?;
    let (length, hash) = hash_file(
        &mut db,
        limits.max_records_bytes.min(limits.max_artifact_bytes),
    )?;
    if length != manifest.database.bytes || hash != manifest.database.sha256 {
        return Err(Error::Integrity);
    }
    if length + bytes.len() as u64 > limits.max_total_bytes {
        return Err(Error::Bounds);
    }
    let names = std::fs::read_dir(path)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<std::collections::BTreeSet<_>>>()?;
    if names
        != std::collections::BTreeSet::from([
            std::ffi::OsString::from("manifest.json"),
            std::ffi::OsString::from(DATABASE),
        ])
    {
        return Err(Error::Integrity);
    }
    let mut database = unixfs::Source::open(&path.join(DATABASE), true)?;
    let connection = records::open_snapshot(&path.join(DATABASE))?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    connection.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        limits
            .max_row_bytes
            .saturating_mul(2)
            .min(i32::MAX as usize) as i32,
    )?;
    let version: i64 = connection.query_row(
        "SELECT version FROM legacy_review_schema_v1 WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let imported: (String, String, String, u64) = connection.query_row(
        "SELECT export_id,manifest_sha256,state,record_count FROM legacy_review_imports_v1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let count: u64 =
        connection.query_row("SELECT count(*) FROM legacy_review_records_v1", [], |r| {
            r.get(0)
        })?;
    let artifacts: usize =
        connection.query_row("SELECT count(*) FROM legacy_review_artifacts_v1", [], |r| {
            r.get(0)
        })?;
    let unavailable: usize = connection.query_row(
        "SELECT count(*) FROM legacy_review_artifacts_v1 WHERE availability='unavailable'",
        [],
        |r| r.get(0),
    )?;
    if version != 1
        || imported
            != (
                expected.export_id.to_string(),
                expected.manifest_sha256.clone(),
                "staged".into(),
                manifest.records,
            )
        || count != manifest.records
        || artifacts > limits.max_artifacts
        || unavailable != manifest.unavailable_artifacts
    {
        return Err(Error::Integrity);
    }
    database.validate(
        &hash,
        limits.max_records_bytes.min(limits.max_artifact_bytes),
    )?;
    unixfs::validate_directory(&directory, path)?;
    Ok(ReviewArchive {
        connection,
        directory,
        database,
        path: path.into(),
        manifest,
        limits: limits.clone(),
    })
}
impl ReviewArchive {
    pub fn open(path: &Path, expected: &ArchiveIdentity, limits: &Limits) -> Result<Self> {
        open_inner(path, expected, limits, true)
    }
    fn guard(&self) -> Result<()> {
        unixfs::validate_directory(&self.directory, &self.path)?;
        self.database.check_path()
    }
    pub fn list(&self, offset: u64, limit: usize) -> Result<ReviewPage> {
        self.guard()?;
        if !(1..=100).contains(&limit) || offset > self.manifest.records {
            return Err(Error::Invalid);
        }
        let mut query = self.connection.prepare("SELECT dto_json FROM legacy_review_records_v1 ORDER BY artifact_index,legacy_rowid LIMIT ?1 OFFSET ?2")?;
        let mut rows = query.query(params![limit as i64, offset as i64])?;
        let mut records = Vec::new();
        let mut size = 64usize; // page keys, separators and next-offset allowance
        while let Some(row) = rows.next()? {
            let encoded: Vec<u8> = row.get(0)?;
            if encoded.len() > MAX_REPLY {
                return Err(Error::Bounds);
            }
            if size + encoded.len() + 1 > MAX_REPLY {
                if records.is_empty() {
                    return Err(Error::Bounds);
                }
                break;
            }
            size += encoded.len() + 1;
            records.push(serde_json::from_slice(&encoded)?);
        }
        let next = offset + records.len() as u64;
        self.guard()?;
        Ok(ReviewPage {
            records,
            next_offset: (next < self.manifest.records).then_some(next),
        })
    }
    pub fn get(&self, id: &str) -> Result<Option<ArchivedReview>> {
        let bytes = self.read_column(id, false)?;
        bytes
            .map(|b| serde_json::from_slice(&b).map_err(Error::from))
            .transpose()
    }
    pub fn raw_record(&self, id: &str) -> Result<Option<Vec<u8>>> {
        self.read_column(id, true)
    }
    fn read_column(&self, id: &str, raw: bool) -> Result<Option<Vec<u8>>> {
        self.guard()?;
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid);
        }
        let sql = if raw {
            "SELECT raw_json FROM legacy_review_records_v1 WHERE record_id=?1"
        } else {
            "SELECT dto_json FROM legacy_review_records_v1 WHERE record_id=?1"
        };
        let result: Option<Vec<u8>> = self
            .connection
            .query_row(sql, [id], |r| r.get(0))
            .optional()?;
        if result.as_ref().is_some_and(|v| {
            v.len()
                > if raw {
                    self.limits.max_row_bytes
                } else {
                    MAX_REPLY
                }
        }) {
            return Err(Error::Bounds);
        }
        self.guard()?;
        Ok(result)
    }
}
