//! Bounded observations of indexed local Markdown, never folder admission or
//! a claim that external filesystem contents still match a cached index.
mod model;
pub use model::*;
pub(crate) mod schema;
use super::{ReaderError, ReaderStore, Result, Source, SourceKind};
use rusqlite::{OptionalExtension, params};

#[derive(PartialEq, Eq)]
struct Observation {
    authority: String,
    name: String,
    index: Option<(String, Option<i64>)>,
}

impl ReaderStore {
    /// Bounded source header; neither crawls nor changes the retained grant.
    pub fn catalog_source(&self, source: &str) -> Result<Source> {
        schema::validate(&self.library, false)?;
        if self.source_kind(source)? != SourceKind::Local {
            return Err(ReaderError::Invalid("Local catalog source required".into()));
        }
        Ok(self.library.query_row("SELECT id,CASE WHEN length(CAST(name AS BLOB))<=256 THEN name END,enabled FROM sources WHERE id=?1",[source],|r|Ok(Source{id:r.get(0)?,name:r.get(1)?,enabled:r.get(2)?}))?)
    }
    fn catalog_snapshot(&self, source: &str) -> Result<Observation> {
        schema::validate(&self.library, false)?;
        schema::validate(&self.index, true)?;
        if self.source_kind(source)? != SourceKind::Local {
            return Err(ReaderError::Invalid("Local catalog source required".into()));
        }
        self.stored_root(source, true)?.verify()?;
        let (authority,name): (String,String) = self.library.query_row(
            "SELECT a.epoch,CASE WHEN length(CAST(s.name AS BLOB))<=256 THEN s.name END FROM reader_catalog_authorities_v1 a JOIN sources s ON s.id=a.source_id WHERE a.source_id=?1",
            [source],
            |r| Ok((r.get(0)?,r.get(1)?)),
        )?;
        let snapshot = self
            .index
            .query_row(
                "SELECT snapshot,indexed_at_ms FROM reader_catalog_snapshots_v1 WHERE source_id=?1",
                [source],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?;
        if authority.len() != 32
            || !authority.bytes().all(|b| b.is_ascii_hexdigit())
            || snapshot
                .as_ref()
                .is_some_and(|(s, _)| s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(ReaderError::Invalid("Invalid catalog snapshot".into()));
        }
        Ok(Observation {
            authority,
            name,
            index: snapshot,
        })
    }
    /// The cursor snapshot fences every committed reindex and grant transition,
    /// including revoke/re-enable. Exact document reads must still verify bytes.
    pub fn catalog_page(
        &self,
        source: &str,
        after: Option<&str>,
        expected: Option<&Snapshot>,
        limit: usize,
    ) -> Result<Page> {
        self.catalog_page_checked(source, after, expected, limit, || {})
    }
    fn catalog_page_checked(
        &self,
        source: &str,
        after: Option<&str>,
        expected: Option<&Snapshot>,
        limit: usize,
        observed: impl FnOnce(),
    ) -> Result<Page> {
        if !(1..=100).contains(&limit)
            || after.is_some_and(|p| {
                p.is_empty()
                    || p.len() > 4096
                    || std::path::Path::new(p)
                        .components()
                        .any(|c| !matches!(c, std::path::Component::Normal(_)))
            })
        {
            return Err(ReaderError::Invalid("Invalid catalog page".into()));
        }
        let before = self.catalog_snapshot(source)?;
        let snapshot = before.index.as_ref().map(|(index, _)| Snapshot {
            authority: before.authority.clone(),
            index: index.clone(),
        });
        if expected.is_some() && expected != snapshot.as_ref()
            || after.is_some() && expected.is_none()
        {
            return Err(ReaderError::Conflict);
        }
        if snapshot.is_none() {
            if self.catalog_snapshot(source)? != before {
                return Err(ReaderError::Conflict);
            }
            return Ok(Page {
                source_name: before.name,
                snapshot: None,
                indexed_at_ms: None,
                documents: vec![],
                next_after: None,
            });
        }
        let mut documents=self.index.prepare_cached("SELECT path,revision,title,modified_ms,pending FROM (
 SELECT d.path,d.revision,d.title,d.modified_ms,0 AS pending FROM available_documents d WHERE d.source_id=?1 AND NOT EXISTS(SELECT 1 FROM reader_folder_files_v1 f WHERE f.source_id=d.source_id AND f.path=d.path AND f.scan<>d.scan)
 UNION ALL SELECT f.path,'' AS revision,f.path AS title,f.modified_ms,1 AS pending FROM reader_folder_files_v1 f WHERE f.source_id=?1 AND NOT EXISTS(SELECT 1 FROM available_documents d WHERE d.source_id=f.source_id AND d.path=f.path AND d.scan=f.scan) AND NOT EXISTS(SELECT 1 FROM reader_authority.reader_local_denied_v1 x WHERE x.source_id=f.source_id AND (x.prefix='' OR f.path=x.prefix COLLATE NOCASE OR substr(f.path,1,length(x.prefix)+1)=x.prefix||'/' COLLATE NOCASE))
 ) WHERE path>?2 ORDER BY path LIMIT ?3")?.query_map(params![source,after.unwrap_or(""),limit+1],|r|Ok(Document {path:r.get(0)?,revision:r.get(1)?,title:r.get(2)?,modified_ms:r.get(3)?,pending:r.get(4)?}))?.collect::<std::result::Result<Vec<_>,_>>()?;
        if documents.iter().any(|d| {
            d.path.is_empty()
                || d.path.chars().any(char::is_control)
                || std::path::Path::new(&d.path)
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                || (!d.pending && (d.revision.len() != 64 || !d.revision.bytes().all(|b| b.is_ascii_hexdigit())))
                || (d.pending && !d.revision.is_empty())
        }) {
            return Err(ReaderError::Invalid("Invalid catalog evidence".into()));
        }
        observed();
        let next_after = if documents.len() > limit {
            documents.truncate(limit);
            documents.last().map(|d| d.path.clone())
        } else {
            None
        };
        if self.catalog_snapshot(source)? != before {
            return Err(ReaderError::Conflict);
        }
        Ok(Page {
            snapshot,
            source_name: before.name,
            indexed_at_ms: before.index.and_then(|(_, at)| at),
            documents,
            next_after,
        })
    }
}

#[cfg(test)]
mod tests;
