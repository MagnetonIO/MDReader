//! Resumable owner turns for metadata discovery and lexical publication.
//! Aborted/incomplete scans preserve unseen rows; only a successful final gate
//! may retire missing files. Watch preferences are durable, not active-tab state.
mod preferences;
mod schema;
mod worker;
use super::{invalid, ReaderError, ReaderStore, Result, MAX_DOCUMENT_BYTES};
use crate::{
    source_root::file_stamp::{single_link_stamp, SourceFileStamp},
    SourceRoot,
};
use rusqlite::{params, OptionalExtension};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
pub use worker::FolderScan;
use worker::ScanEvent;
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FolderIndexStatus {
    pub source_id: String,
    pub monitor_enabled: bool,
    pub phase: String,
    pub known_documents: u64,
    pub indexed_documents: u64,
    pub observed_at_ms: Option<i64>,
    pub complete: bool,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderScanProgress {
    Waiting,
    Published,
    Complete,
}
impl ReaderStore {
    pub fn begin_folder_scan(&mut self, source: &str, generation: u64) -> Result<FolderScan> {
        let root = self.stored_root(source, true)?;
        self.folder_generation(source, generation)?;
        let known=self.index.prepare("SELECT f.path,f.stamp,d.revision FROM reader_folder_files_v1 f JOIN available_documents d ON d.source_id=f.source_id AND d.path=f.path WHERE f.source_id=?1 AND f.scan=d.scan AND d.parser_version=?2 LIMIT 100001")?.query_map(params![source,crate::parse::PARSER_VERSION],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut stamps = HashMap::new();
        if known.len() > 100000 {
            return Err(invalid("Folder observation exceeds metadata budget"));
        }
        for (path, stamp, revision) in known {
            stamps.insert(
                path,
                (
                    serde_json::from_str::<SourceFileStamp>(&stamp)
                        .map_err(|_| ReaderError::Unavailable)?,
                    revision,
                ),
            );
        }
        let paths=self.library.prepare("SELECT path FROM reader_folder_events_v1 WHERE source_id=?1 ORDER BY path LIMIT 257")?.query_map([source],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let has_complete: bool = self.index.query_row(
            "SELECT EXISTS(SELECT 1 FROM reader_folder_scans_v1 WHERE source_id=?1 AND complete=1)",
            [source],
            |r| r.get(0),
        )?;
        let targets = if has_complete && !paths.is_empty() && !paths.iter().any(|p| p.is_empty()) {
            Some(paths.into_iter().map(PathBuf::from).collect())
        } else {
            None
        };
        // Consume only the captured event batch. New events during this walk
        // stay durable for the following generation rather than restarting an
        // indefinitely busy broad-root census.
        self.library.execute(
            "DELETE FROM reader_folder_events_v1 WHERE source_id=?1",
            [source],
        )?;
        let scan = FolderScan::start(source.into(), generation, root, stamps, targets);
        self.index.execute("INSERT INTO reader_folder_scans_v1 VALUES(?1,?2,?3,0,NULL) ON CONFLICT(source_id) DO UPDATE SET scan=excluded.scan,generation=excluded.generation,complete=0",params![source,scan.token,generation])?;
        self.record_index_job_if_current(source, generation, "indexing", None)?;
        Ok(scan)
    }
    fn folder_generation(&self, source: &str, generation: u64) -> Result<()> {
        if !self
            .index_job_record(source)?
            .is_some_and(|j| j.generation == generation)
        {
            return Err(ReaderError::Cancelled);
        }
        self.stored_root(source, true)?.verify()?;
        Ok(())
    }
    /// At most eight metadata/body receipts per owner turn, without scanning,
    /// hashing, parsing or waiting on workers under the owner lock.
    pub fn publish_folder_scan(&mut self, scan: &mut FolderScan) -> Result<FolderScanProgress> {
        self.folder_generation(&scan.source, scan.generation)?;
        scan.root.verify()?;
        let mut published = false;
        for _ in 0..8 {
            let Some(event) = scan.next()? else {
                return Ok(if published {
                    FolderScanProgress::Published
                } else {
                    FolderScanProgress::Waiting
                });
            };
            let complete = matches!(event, ScanEvent::Complete);
            if !complete {
                let (path, stamp) = match &event {
                    ScanEvent::Metadata { path, stamp, .. }
                    | ScanEvent::Body { path, stamp, .. } => (path, stamp),
                    _ => unreachable!(),
                };
                if single_link_stamp(&scan.root.open_file_relative(Path::new(path))?)? != *stamp {
                    return Err(ReaderError::Conflict);
                }
            }
            self.folder_generation(&scan.source, scan.generation)?;
            let tx = self.index.transaction()?;
            let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM reader_folder_scans_v1 WHERE source_id=?1 AND scan=?2 AND generation=?3)",params![scan.source,scan.token,scan.generation],|r|r.get(0))?;
            if !active {
                return Err(ReaderError::Cancelled);
            }
            match event {
                ScanEvent::Metadata {
                    path,
                    stamp,
                    size,
                    modified,
                } => {
                    let stamp =
                        serde_json::to_string(&stamp).map_err(|_| ReaderError::Unavailable)?;
                    tx.execute("INSERT INTO reader_folder_files_v1 VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(source_id,path) DO UPDATE SET stamp=excluded.stamp,size=excluded.size,modified_ms=excluded.modified_ms,scan=excluded.scan",params![scan.source,path,stamp,size,modified,scan.token])?;
                }
                ScanEvent::Body {
                    path,
                    revision,
                    modified,
                    parsed,
                    ..
                } => {
                    if let Some(parsed) = parsed {
                        tx.execute(
                            "DELETE FROM documents WHERE source_id=?1 AND path=?2",
                            params![scan.source, path],
                        )?;
                        super::install_parsed(
                            &tx,
                            &scan.source,
                            &path,
                            &revision,
                            modified,
                            &scan.token,
                            parsed,
                        )?;
                    } else {
                        let count=tx.execute("UPDATE documents SET scan=?3,modified_ms=?4 WHERE source_id=?1 AND path=?2 AND revision=?5 AND parser_version=?6",params![scan.source,path,scan.token,modified,revision,crate::parse::PARSER_VERSION])?;
                        if count != 1 {
                            return Err(ReaderError::Conflict);
                        }
                    }
                }
                ScanEvent::Complete => {
                    if let Some(targets) = &scan.targets {
                        for path in targets {
                            let path = path.to_str().ok_or(ReaderError::Unavailable)?;
                            for table in ["documents", "reader_folder_files_v1"] {
                                tx.execute(&format!("DELETE FROM {table} WHERE source_id=?1 AND scan<>?2 AND (path=?3 OR substr(path,1,length(?3)+1)=?3||'/')"),params![scan.source,scan.token,path])?;
                            }
                        }
                    } else {
                        for table in ["documents", "reader_folder_files_v1"] {
                            tx.execute(
                                &format!("DELETE FROM {table} WHERE source_id=?1 AND scan<>?2"),
                                params![scan.source, scan.token],
                            )?;
                        }
                    }
                    super::rebuild_resolved_links(
                        &tx,
                        &scan.source,
                        &super::Cancellation::default(),
                    )?;
                    tx.execute("UPDATE reader_folder_scans_v1 SET complete=1,observed_at_ms=?2 WHERE source_id=?1",params![scan.source,now()?])?;
                }
            }
            scan.root.verify()?;
            crate::store::catalog::schema::publish(&tx, &scan.source)?;
            tx.commit()?;
            published = true;
            if complete {
                let pending: bool = self.library.query_row(
                    "SELECT EXISTS(SELECT 1 FROM reader_folder_events_v1 WHERE source_id=?1)",
                    [&scan.source],
                    |r| r.get(0),
                )?;
                if pending {
                    self.library.execute("UPDATE index_jobs SET phase='queued',generation=generation+1,error=NULL WHERE source_id=?1",[&scan.source])?;
                    return Err(ReaderError::Cancelled);
                }
                return Ok(FolderScanProgress::Complete);
            }
        }
        Ok(FolderScanProgress::Published)
    }
}
fn now() -> Result<i64> {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| ReaderError::Unavailable)?
            .as_millis(),
    )
    .map_err(|_| ReaderError::Unavailable)
}
pub(crate) fn initialize(
    library: &rusqlite::Connection,
    index: &rusqlite::Connection,
) -> Result<()> {
    schema::initialize(library, index)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod performance;
