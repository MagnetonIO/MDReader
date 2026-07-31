//! The frontmatter source: YAML frontmatter fields, and the first H1 as a
//! title fallback.
//!
//! # Its honest declaration
//!
//! | attribute | type | pushdown | note |
//! |---|---|---|---|
//! | `path` | `Text` | `Eq`, `Prefix` | it walks the tree, so it can prune |
//! | `title` | `Text` | *none* | it has to open and parse the file to know |
//! | `author` | `Text` | *none* | likewise |
//! | `body` | `Text` | *none* | likewise |
//!
//! It does not observe `size` or `modifiedAt`.
//!
//! # The fallback chain, and where it stops
//!
//! `title` is the YAML `title:` when there is one, otherwise the document's
//! first H1. When there is neither, `title` is **absent from the row** — not
//! the filename, not the empty string. A filename is a plausible guess and
//! plausible guesses are exactly what a semantic layer must not make.
//!
//! `author` has no fallback at all: it is the YAML `author:` or nothing.
//!
//! # Malformed frontmatter fails the read
//!
//! A frontmatter block that is not valid YAML is
//! [`SourceError::Malformed`], not a document quietly treated as having no
//! metadata. The corpus contains such a file on purpose. Skipping it would
//! silently shrink every answer that should have included it, and a silently
//! shorter answer is indistinguishable from a correct one.
//!
//! # Paging consistency
//!
//! A frontmatter cursor is a bounded lexical resume key, not an operating
//! system snapshot. Each page reads current file bytes, so concurrent edits
//! may be visible on later pages. The connector makes this weaker guarantee
//! explicit instead of relying on asynchronous watcher timing.

use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use catdb_core::Schema;
use catdb_query::EntityQuery;

use crate::{
    Availability, DataSource, ReadContext, SourceAttr, SourceCapabilities, SourceCursor,
    SourceError, SourceFreshness, SourceId, SourceModel, SourcePage, SourcePlan,
};

const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

/// Reads YAML frontmatter (and the first H1) out of the markdown files under a
/// directory.
#[derive(Debug, Clone)]
pub struct FrontmatterSource {
    model: SourceModel,
    root: PathBuf,
    contained_root: Option<crate::walk::ContainedRoot>,
    cursor_key: [u8; 32],
}

impl FrontmatterSource {
    /// The source's name, as it appears on every row it produces.
    pub const ID: &'static str = "frontmatter";

    /// The file extensions this source considers markdown documents.
    pub const EXTENSIONS: &'static [&'static str] = &["md", "markdown"];

    fn attributes() -> Vec<SourceAttr> {
        vec![
            SourceAttr::text("path").pushes([
                catdb_storage::ObjectFilterOp::Eq,
                catdb_storage::ObjectFilterOp::Prefix,
            ]),
            // Everything else needs the file open and parsed, so there is
            // nothing this source can narrow with.
            SourceAttr::text("title"),
            SourceAttr::text("author"),
            SourceAttr::text("body"),
        ]
    }

    /// Open a directory tree of markdown documents.
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
        let object = crate::source_spec_object(
            spec,
            "frontmatter",
            &["extensions", "format", "kind", "root"],
        )?;
        let extensions = object
            .get("extensions")
            .and_then(serde_json::Value::as_array)
            .ok_or(crate::CatalogSourceSpecError)?;
        if extensions
            != &Self::EXTENSIONS
                .iter()
                .map(|extension| serde_json::Value::String((*extension).to_string()))
                .collect::<Vec<_>>()
        {
            return Err(crate::CatalogSourceSpecError);
        }
        let contained = crate::contained_root_from_spec(
            object.get("root").ok_or(crate::CatalogSourceSpecError)?,
        )?;
        Ok(Self::from_open_attempt(
            contained.canonical().to_path_buf(),
            Ok(contained),
        ))
    }

    /// The root this source reads.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl DataSource for FrontmatterSource {
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
            "extensions": Self::EXTENSIONS,
            "format": crate::LOCAL_SOURCE_FORMAT,
            "kind": "frontmatter",
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

impl FrontmatterSource {
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
            Some(Self::EXTENSIONS),
            cursor.and_then(|cursor| cursor.walk),
            quantum,
            context,
        )
        .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))?;

        let mut scanned = 0usize;
        let mut rows = Vec::new();
        let mut reserved_document_bytes = 0usize;
        for file in &walked.files {
            context.check()?;
            let text = self.read_checked(file, context, &mut reserved_document_bytes)?;
            let observed = self.observe(&file.rel, &text)?;
            let mut row = self.model.row(&file.rel, observed.raw)?;
            if let Some(fields) = row.value.as_object_mut() {
                for field in observed.explicit_nulls {
                    fields.insert(field, serde_json::Value::Null);
                }
            }
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

    fn read_checked(
        &self,
        file: &crate::walk::Walked,
        context: &ReadContext,
        reserved_document_bytes: &mut usize,
    ) -> Result<String, SourceError> {
        let fail = |error: std::io::Error| SourceError::Read {
            source_id: self.model.id().clone(),
            path: file.rel.clone(),
            detail: error.to_string(),
        };
        let contained_root =
            self.contained_root
                .as_ref()
                .ok_or_else(|| SourceError::Unavailable {
                    source_id: self.model.id().clone(),
                    reason: "secure contained-open root is unavailable".to_string(),
                })?;
        let mut input = contained_root
            .open_file(Path::new(&file.rel))
            .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))?;
        let declared_len = input.metadata().map_err(&fail)?.len();
        if declared_len > u64::try_from(MAX_DOCUMENT_BYTES).expect("fixed bound fits u64") {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "frontmatter document bytes",
            });
        }
        let declared_len = usize::try_from(declared_len).unwrap_or(usize::MAX);
        let starting_bytes = *reserved_document_bytes;
        let reserved_after = starting_bytes
            .checked_add(declared_len)
            .ok_or(SourceError::ByteBudgetExceeded)?;
        if reserved_after > context.remaining_bytes() {
            return Err(SourceError::ByteBudgetExceeded);
        }
        *reserved_document_bytes = reserved_after;
        let mut bytes = Vec::with_capacity(declared_len);
        let mut chunk = [0u8; 64 * 1024];
        loop {
            context.check()?;
            let read = input.read(&mut chunk).map_err(&fail)?;
            if read == 0 {
                break;
            }
            if bytes.len().saturating_add(read) > MAX_DOCUMENT_BYTES {
                return Err(SourceError::ResourceLimit {
                    source_id: self.model.id().clone(),
                    resource: "frontmatter document bytes",
                });
            }
            let actual_after = starting_bytes
                .checked_add(bytes.len())
                .and_then(|used| used.checked_add(read))
                .ok_or(SourceError::ByteBudgetExceeded)?;
            if actual_after > context.remaining_bytes() {
                return Err(SourceError::ByteBudgetExceeded);
            }
            *reserved_document_bytes = (*reserved_document_bytes).max(actual_after);
            bytes.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8(bytes).map_err(|error| SourceError::Read {
            source_id: self.model.id().clone(),
            path: file.rel.clone(),
            detail: error.to_string(),
        })
    }
}

impl FrontmatterSource {
    /// Read one document into the lexical forms the declared transformations
    /// type. An attribute that is not there is simply not in the map.
    fn observe(&self, rel: &str, text: &str) -> Result<ObservedFrontmatter, SourceError> {
        let malformed = |attr: &str, detail: String| SourceError::Malformed {
            source_id: self.model.id().clone(),
            path: rel.to_string(),
            attr: attr.to_string(),
            detail,
        };
        let (yaml, body) = match split_frontmatter(text) {
            Some(split) => split,
            None => (String::new(), text.to_string()),
        };
        let document: serde_yaml::Value = serde_yaml::from_str(&yaml)
            .map_err(|error| malformed("frontmatter", error.to_string()))?;

        let mut raw = std::collections::BTreeMap::new();
        let mut explicit_nulls = Vec::new();
        raw.insert("path".to_string(), rel.to_string());
        raw.insert("body".to_string(), body.clone());
        // `title:` if the YAML declares one, otherwise the first H1 — and
        // nothing at all if neither exists.
        if let Some(title) = string_field(&document, "title")
            .map_err(|detail| malformed("title", detail))?
            .or_else(|| first_h1(&body))
        {
            raw.insert("title".to_string(), title);
        }
        // No fallback: an author is declared or it is absent.
        if yaml_field(&document, "author").is_some_and(serde_yaml::Value::is_null) {
            explicit_nulls.push("author".to_string());
        } else if let Some(author) =
            string_field(&document, "author").map_err(|detail| malformed("author", detail))?
        {
            raw.insert("author".to_string(), author);
        }
        Ok(ObservedFrontmatter {
            raw,
            explicit_nulls,
        })
    }
}

struct ObservedFrontmatter {
    raw: std::collections::BTreeMap<String, String>,
    explicit_nulls: Vec<String>,
}

fn yaml_field<'a>(document: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    document
        .as_mapping()?
        .iter()
        .find_map(|(found, value)| (found.as_str() == Some(key)).then_some(value))
}

/// Split a leading `---` fenced YAML block off the document, returning the
/// block and the body after it. `None` when there is no frontmatter at all.
fn split_frontmatter(text: &str) -> Option<(String, String)> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            return Some((
                rest[..offset].to_string(),
                rest[offset + line.len()..].to_string(),
            ));
        }
        offset += line.len();
    }
    // An unterminated block is not a frontmatter block; the document is all
    // body, which is what a markdown renderer would do with it too.
    None
}

/// One YAML field, when it is a string.
///
/// A field of another shape is an error rather than a rendered guess: this
/// source declares `title` and `author` as `Text`, and `[a, b]` is not text
/// however it is spelled.
fn string_field(document: &serde_yaml::Value, key: &str) -> Result<Option<String>, String> {
    let Some(mapping) = document.as_mapping() else {
        return Ok(None);
    };
    let Some((_, value)) = mapping.iter().find(|(k, _)| k.as_str() == Some(key)) else {
        return Ok(None);
    };
    match value {
        serde_yaml::Value::String(text) => Ok(Some(text.clone())),
        serde_yaml::Value::Null => Ok(None),
        other => Err(format!(
            "`{key}:` is {}, not text",
            match other {
                serde_yaml::Value::Bool(_) => "a boolean",
                serde_yaml::Value::Number(_) => "a number",
                serde_yaml::Value::Sequence(_) => "a sequence",
                serde_yaml::Value::Mapping(_) => "a mapping",
                _ => "not a scalar",
            }
        )),
    }
}

/// The document's first level-one heading, skipping fenced code blocks — a
/// `#` inside a fence is code, not a heading, and a title taken from one would
/// be an observation of something that is not there.
fn first_h1(body: &str) -> Option<String> {
    let mut fenced = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("# ") {
            let heading = heading.trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frontmatter_block_splits_off_the_body() {
        let (yaml, body) = split_frontmatter("---\ntitle: X\n---\n\nbody\n").expect("split");
        assert_eq!(yaml, "title: X\n");
        assert_eq!(body, "\nbody\n");
        // Delimiters with nothing between them are "no metadata", not an error.
        let (yaml, body) = split_frontmatter("---\n---\n\n# H\n").expect("split");
        assert_eq!(yaml, "");
        assert_eq!(body, "\n# H\n");
        // A document with a frontmatter block and no body at all.
        let (_, body) = split_frontmatter("---\ntitle: X\n---\n").expect("split");
        assert_eq!(body, "");
        // No opening delimiter, and an unterminated one, are both "no block".
        assert!(split_frontmatter("# H\n").is_none());
        assert!(split_frontmatter("---\ntitle: X\n").is_none());
    }

    #[test]
    fn the_first_h1_skips_fenced_code_and_deeper_headings() {
        assert_eq!(first_h1("## Deep\n\n# Real\n"), Some("Real".to_string()));
        assert_eq!(
            first_h1("```\n# Fake\n```\n# Real\n"),
            Some("Real".to_string())
        );
        assert_eq!(
            first_h1("~~~\n# Fake\n~~~\n# Real\n"),
            Some("Real".to_string())
        );
        assert_eq!(first_h1("no headings here\n"), None);
        assert_eq!(first_h1("#NoSpace\n"), None);
    }
}
