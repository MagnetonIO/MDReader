//! The filesystem source: path, size, mtime.
//!
//! # Its honest declaration
//!
//! | attribute | type | pushdown | note |
//! |---|---|---|---|
//! | `path` | `Text` | `Eq`, `Prefix` | a walk really can prune by prefix, and really can stat one path |
//! | `size` | `Int` | *none* | a walk cannot narrow by size; it has to stat every candidate anyway |
//! | `modifiedAt` | `Timestamp` | *none* | likewise |
//!
//! And it does **not** observe `title`, `author` or `body` at all. Asked for
//! one, it returns [`SourceError::UnobservableAttribute`] — it will not
//! post-filter on an attribute its rows never carry and hand back an empty
//! answer.
//!
//! # Paging consistency
//!
//! A filesystem cursor is a bounded lexical resume key, not an operating
//! system snapshot. Each page observes the tree when that page runs, so
//! concurrent file edits may be visible on later pages. The federated query
//! service retains its completed result for client-facing continuation, but
//! the connector does not claim an immutable filesystem view it cannot prove.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use catdb_core::{BaseType, Schema};
use catdb_query::EntityQuery;
use catdb_storage::ObjectFilterOp;

use crate::{
    Availability, DataSource, ReadContext, SourceAttr, SourceCapabilities, SourceCursor,
    SourceError, SourceFreshness, SourceId, SourceModel, SourcePage, SourcePlan,
};

/// Reads documents out of a directory tree.
#[derive(Debug, Clone)]
pub struct FilesystemSource {
    model: SourceModel,
    root: PathBuf,
    contained_root: Option<crate::walk::ContainedRoot>,
    cursor_key: [u8; 32],
}

impl FilesystemSource {
    /// The source's name, as it appears on every row it produces.
    pub const ID: &'static str = "filesystem";

    fn attributes() -> Vec<SourceAttr> {
        vec![
            // A walk really can prune by prefix and really can look at one
            // path, so these are honest.
            SourceAttr::text("path").pushes([ObjectFilterOp::Eq, ObjectFilterOp::Prefix]),
            // Size and mtime come from the stat this source has to do for
            // every candidate anyway: there is nothing to narrow with.
            SourceAttr::parsed("size", BaseType::Int),
            SourceAttr::parsed("modifiedAt", BaseType::Timestamp),
        ]
    }

    /// Open a directory tree. A root that does not exist (or is not a
    /// directory) is declared unavailable, not a panic.
    pub fn new(root: impl AsRef<Path>) -> Self {
        let requested_root = root.as_ref().to_path_buf();
        let contained_root = crate::walk::ContainedRoot::open(&requested_root);
        Self::from_open_attempt(requested_root, contained_root)
    }

    /// Preserve the connector's declared schema while explicitly refusing
    /// access to a registered physical source that is not currently usable.
    pub fn unavailable(root: impl AsRef<Path>, reason: impl Into<String>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            model: SourceModel::unavailable_with_attrs(
                SourceId::new(Self::ID),
                "doc",
                "Document",
                Self::attributes(),
                reason,
            ),
            root,
            contained_root: None,
            cursor_key: crate::walk::new_cursor_key(),
        }
    }

    /// Read through an already-admitted directory authority without reopening
    /// its pathname.
    pub fn from_directory_authority(authority: &crate::DirectoryAuthority) -> Self {
        Self::from_open_attempt(
            authority.display_root().to_path_buf(),
            Ok(authority.contained_root().clone()),
        )
    }

    fn from_open_attempt(
        requested_root: PathBuf,
        contained_root: Result<crate::walk::ContainedRoot, crate::walk::WalkError>,
    ) -> Self {
        let root = contained_root
            .as_ref()
            .map(|root| root.canonical().to_path_buf())
            .unwrap_or(requested_root);
        let id = SourceId::new(Self::ID);
        let model = match &contained_root {
            Ok(_) => SourceModel::new(id, "doc", "Document", Self::attributes()),
            Err(error) => SourceModel::unavailable(
                id,
                "doc",
                "Document",
                format!(
                    "`{}` is not a directory or has no secure contained-open root: {error}",
                    root.display()
                ),
            ),
        };
        Self {
            model,
            root,
            contained_root: contained_root.ok(),
            cursor_key: crate::walk::new_cursor_key(),
        }
    }

    /// Reopen a source from its exact V1 catalog body.
    ///
    /// The catalog pins a lossless, platform-tagged absolute canonical path
    /// and the physical directory identity observed when the version was
    /// authored. Reopening a relative path, a different platform's path, or a
    /// replaced directory fails closed. A root must therefore exist while its
    /// catalog version is authored; an unavailable instance has no physical
    /// identity and cannot be reopened as catalog authority.
    pub fn from_source_spec(
        spec: &serde_json::Value,
    ) -> Result<Self, crate::CatalogSourceSpecError> {
        let object = crate::source_spec_object(spec, "filesystem", &["format", "kind", "root"])?;
        let contained = crate::contained_root_from_spec(
            object.get("root").ok_or(crate::CatalogSourceSpecError)?,
        )?;
        Ok(Self::from_open_attempt(
            contained.canonical().to_path_buf(),
            Ok(contained),
        ))
    }

    /// The root this source reads, for provenance and tests.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl DataSource for FilesystemSource {
    fn id(&self) -> &SourceId {
        self.model.id()
    }

    fn schema(&self) -> &Schema {
        self.model.schema()
    }

    fn capabilities(&self) -> &SourceCapabilities {
        self.model.capabilities()
    }

    fn source_spec(&self) -> serde_json::Value {
        serde_json::json!({
            "format": crate::LOCAL_SOURCE_FORMAT,
            "kind": "filesystem",
            "root": crate::root_authority_spec(
                &self.root,
                self.contained_root.as_ref().map(crate::walk::ContainedRoot::identity),
            ),
        })
    }

    async fn freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
        context.check()?;
        let source = self.clone();
        let owned_context = context.clone();
        tokio::task::spawn_blocking(move || source.measure_freshness(&owned_context))
            .await
            .map_err(|_| SourceError::WorkerFailed)?
    }

    fn compile(
        &self,
        query: &EntityQuery,
        authorized_fields: &[String],
    ) -> Result<SourcePlan, SourceError> {
        self.model.compile(query, authorized_fields)
    }

    async fn execute_page(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        context.check()?;
        let source = self.clone();
        let owned_plan = plan.clone();
        let owned_cursor = cursor.cloned();
        let owned_context = context.clone();
        tokio::task::spawn_blocking(move || {
            source.execute_bounded(&owned_plan, owned_cursor.as_ref(), &owned_context)
        })
        .await
        .map_err(|_| SourceError::WorkerFailed)?
    }
}

impl FilesystemSource {
    fn measure_freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
        let started_at_ms = crate::observation_now_ms();
        if matches!(
            self.model.capabilities().availability(),
            Availability::Available
        ) {
            self.verify_root_authority()?;
            self.contained_root
                .as_ref()
                .ok_or_else(|| SourceError::Containment {
                    source_id: self.model.id().clone(),
                    path: self.root.display().to_string(),
                    detail: "configured root has no physical identity".to_string(),
                })?
                .inspect()
                .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))?;
        }
        context.check()?;
        Ok(crate::freshness_since(started_at_ms))
    }

    fn execute_bounded(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        let started_at_ms = crate::observation_now_ms();
        self.model.admit(plan)?;
        self.verify_root_authority()?;
        let cursor = crate::walk::decode_cursor(cursor, &self.cursor_key)?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.snapshot.is_some() || cursor.after.is_some())
        {
            return Err(SourceError::InvalidCursor);
        }
        let prefix = crate::plan::pushed_prefix(plan, "path");
        let quantum = context.page_size();
        let contained_root =
            self.contained_root
                .as_ref()
                .ok_or_else(|| SourceError::Containment {
                    source_id: self.model.id().clone(),
                    path: self.root.display().to_string(),
                    detail: "configured root has no physical identity".to_string(),
                })?;
        let walked = crate::walk::walk_page(
            contained_root,
            &prefix,
            None,
            cursor.and_then(|cursor| cursor.walk),
            quantum,
            context,
        )
        .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))?;

        let mut scanned = 0usize;
        let mut rows = Vec::new();
        for file in &walked.files {
            context.check()?;
            let row = self.model.row(&file.rel, self.observe(file)?)?;
            // The source executes what it declared it can, and only then is a
            // candidate counted as examined.
            // Counted before the source's own filters run: `scanned` is what
            // this source physically examined, so a pushdown that never
            // reached the physical read cannot hide behind it.
            scanned += 1;
            if !plan
                .pushed
                .filters
                .iter()
                .all(|filter| crate::plan::filter_matches(filter, &row.value))
            {
                continue;
            }
            if crate::plan::passes_all(&plan.residual, &row.value) {
                let mut row = row;
                if let Some(fields) = row.value.as_object_mut() {
                    fields.retain(|field, _| plan.projected.contains(field));
                }
                rows.push(row);
            }
        }
        let has_more = walked.has_more;
        let next_cursor = if has_more {
            Some(crate::walk::encode_cursor(
                &crate::walk::LocalCursor {
                    after: None,
                    walk: Some(walked.cursor),
                    snapshot: None,
                },
                &self.cursor_key,
            )?)
        } else {
            None
        };
        context.check()?;
        let freshness = crate::freshness_since(started_at_ms);
        Ok(SourcePage {
            source: self.model.id().clone(),
            scanned,
            rows,
            next_cursor,
            observed_at_ms: freshness.observed_at_ms,
            freshness_ms: freshness.freshness_ms,
        })
    }
}

impl FilesystemSource {
    fn verify_root_authority(&self) -> Result<(), SourceError> {
        self.contained_root
            .as_ref()
            .ok_or_else(|| SourceError::Containment {
                source_id: self.model.id().clone(),
                path: self.root.display().to_string(),
                detail: "configured root has no physical identity".to_string(),
            })?
            .verify_current()
            .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))
    }

    /// Stat one file into the lexical forms the declared transformations type.
    ///
    /// A stat that fails is a [`SourceError::Read`], not a row with the
    /// attribute missing: the file is there and the source declared it can
    /// observe these attributes, so failing to is missing data, not absence.
    fn observe(
        &self,
        file: &crate::walk::Walked,
    ) -> Result<std::collections::BTreeMap<String, String>, SourceError> {
        let fail = |detail: String| SourceError::Read {
            source_id: self.model.id().clone(),
            path: file.rel.clone(),
            detail,
        };
        let contained_root =
            self.contained_root
                .as_ref()
                .ok_or_else(|| SourceError::Unavailable {
                    source_id: self.model.id().clone(),
                    reason: "secure contained-open root is unavailable".to_string(),
                })?;
        let opened = contained_root
            .open_file(Path::new(&file.rel))
            .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))?;
        let meta = opened.metadata().map_err(|e| fail(e.to_string()))?;
        let modified = meta
            .modified()
            .map_err(|e| fail(e.to_string()))?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| fail(e.to_string()))?
            .as_millis();
        let modified = i64::try_from(modified)
            .map_err(|_| fail("modification time does not fit in an i64".to_string()))?;
        let size = i64::try_from(meta.len())
            .map_err(|_| fail("size does not fit in an i64".to_string()))?;
        Ok([
            ("path".to_string(), file.rel.clone()),
            ("size".to_string(), size.to_string()),
            ("modifiedAt".to_string(), modified.to_string()),
        ]
        .into_iter()
        .collect())
    }
}
