//! Persistent watch preference, bounded event queue and truthful status.
use super::*;
impl ReaderStore {
    pub fn folder_index_status(&self, source: &str) -> Result<FolderIndexStatus> {
        let enabled:bool=self.library.query_row("SELECT COALESCE(p.monitor_enabled,1) FROM sources s LEFT JOIN reader_folder_preferences_v1 p ON p.source_id=s.id WHERE s.id=?1",[source],|r|r.get(0)).optional()?.ok_or(ReaderError::Unavailable)?;
        let available = self.stored_root(source, true).is_ok();
        let job = self.index_job_record(source)?;
        let scan: Option<(bool, Option<i64>)> = self
            .index
            .query_row(
                "SELECT complete,observed_at_ms FROM reader_folder_scans_v1 WHERE source_id=?1",
                [source],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let phase = if !enabled {
            "paused"
        } else if !available {
            "unavailable"
        } else {
            match job.as_ref().map(|j| j.phase.as_str()) {
                Some("queued") => "discovering",
                Some("indexing") => "indexing",
                Some("error") => "error",
                _ if scan.is_none() => "discovering",
                _ => "ready",
            }
        };
        let known:u64=self.index.query_row("SELECT count(*) FROM reader_folder_files_v1 f WHERE f.source_id=?1 AND NOT EXISTS(SELECT 1 FROM reader_authority.reader_local_denied_v1 x WHERE x.source_id=f.source_id AND mdreader_private_prefix_v1(f.path,x.prefix))",[source],|r|r.get(0))?;
        let indexed: u64 = self.index.query_row(
            "SELECT count(*) FROM available_documents WHERE source_id=?1",
            [source],
            |r| r.get(0),
        )?;
        Ok(FolderIndexStatus {
            source_id: source.into(),
            monitor_enabled: enabled,
            phase: phase.into(),
            known_documents: known.max(indexed),
            indexed_documents: indexed,
            observed_at_ms: scan.as_ref().and_then(|s| s.1),
            complete: scan.as_ref().is_some_and(|s| s.0)
                && !matches!(phase, "discovering" | "indexing" | "error" | "unavailable"),
            error: job.and_then(|j| j.error),
        })
    }
    pub fn set_folder_monitoring(&mut self, source: &str, enabled: bool) -> Result<()> {
        if enabled {
            self.stored_root(source, true)?;
        } else {
            let exists: bool = self.library.query_row(
                "SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1)",
                [source],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(ReaderError::Unavailable);
            }
        }
        if source.starts_with("provider:")
            || crate::local_authority::is_owned(&self.library, source)?
            || crate::device::is_device_source(&self.library, source)?
        {
            return Err(ReaderError::Unavailable);
        }
        self.library.execute("INSERT INTO reader_folder_preferences_v1 VALUES(?1,?2) ON CONFLICT(source_id) DO UPDATE SET monitor_enabled=excluded.monitor_enabled",params![source,enabled])?;
        if enabled {
            self.queue_index(source)?;
        } else {
            self.library.execute("UPDATE index_jobs SET phase='done',generation=generation+1,error=NULL WHERE source_id=?1",[source])?;
        }
        Ok(())
    }
    /// Coalesced native events; overflow/ignore-rule changes require a full
    /// census. Queueing during work invalidates that generation, never purges.
    pub fn queue_folder_changes(&mut self, source: &str, paths: &[String]) -> Result<()> {
        self.stored_root(source, true)?;
        if !self.folder_index_status(source)?.monitor_enabled {
            return Ok(());
        }
        let tx = self.library.transaction()?;
        if paths.is_empty()
            || paths.len() > 256
            || paths.iter().any(|p| {
                p.ends_with(".gitignore")
                    || p.ends_with(".ignore")
                    || p.ends_with(".contextignore")
                    || p.ends_with(".git/info/exclude")
            })
        {
            tx.execute(
                "DELETE FROM reader_folder_events_v1 WHERE source_id=?1",
                [source],
            )?;
            tx.execute(
                "INSERT INTO reader_folder_events_v1 VALUES(?1,'')",
                [source],
            )?;
        } else {
            for path in paths {
                if path.len() > 4096
                    || path.is_empty()
                    || Path::new(path)
                        .components()
                        .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    return Err(invalid("Invalid folder event"));
                }
                tx.execute(
                    "INSERT OR IGNORE INTO reader_folder_events_v1 VALUES(?1,?2)",
                    params![source, path],
                )?;
            }
            let count: usize = tx.query_row(
                "SELECT count(*) FROM reader_folder_events_v1 WHERE source_id=?1",
                [source],
                |r| r.get(0),
            )?;
            if count > 256 {
                tx.execute(
                    "DELETE FROM reader_folder_events_v1 WHERE source_id=?1",
                    [source],
                )?;
                tx.execute(
                    "INSERT INTO reader_folder_events_v1 VALUES(?1,'')",
                    [source],
                )?;
            }
        }
        tx.execute("INSERT INTO index_jobs(source_id,phase) VALUES(?1,'queued') ON CONFLICT(source_id) DO UPDATE SET phase=CASE WHEN phase='indexing' THEN phase ELSE 'queued' END,generation=CASE WHEN phase='indexing' THEN generation ELSE generation+1 END,error=NULL",[source])?;
        tx.commit()?;
        Ok(())
    }
}
