//! Which source answers which field, declared in order, with the reason.
//!
//! # Declared, not implied
//!
//! Two sources can answer `modifiedAt` and two can answer `author`. Something
//! has to decide, and the two dishonest ways to decide are (a) whichever
//! source happened to be read first and (b) whichever value looks better. Both
//! are invisible at the call site, so both are unarguable after the fact.
//!
//! Instead the order is a value: [`precedence`] returns, per `Document`
//! attribute, the [`Provider`]s that may answer it, in the order they are
//! consulted, each with a written [`FieldPrecedence::rationale`]. A reader can
//! print it. A test can assert it. Changing which source wins means editing a
//! declaration, not a branch.
//!
//! # Precedence is over SOURCES, not over an adapter's internal rules
//!
//! `title` has two candidates in the concept — a YAML `title:` field and the
//! document's first H1 — but they are not two *sources*. They are the two
//! halves of one rule the frontmatter adapter declares and applies, and the
//! adapter reports the winning title without saying which half fired. This
//! layer does not re-parse the file to find out: re-deriving the adapter's
//! parse in order to attribute it would replace a single declaration with two
//! that can disagree, which is exactly the failure this design exists to
//! prevent.
//!
//! So the adapter-internal chain is *recorded* rather than re-run:
//! [`Provider::declared_fallback`] names it, sourced from the adapter's own
//! documentation, and the attribution says `frontmatter` answered `title`. The
//! order within that chain is still observable where it matters — in the
//! values. A document with both a YAML title and a different first H1 gets the
//! YAML one, and a document with only an H1 gets the H1, and both are pinned
//! by tests against the real corpus.

use catdb_sdk::connectors::{FilesystemSource, FrontmatterSource, GitSource, SourceId};
use catdb_sdk::core::AttrName;
use serde::{Deserialize, Serialize};

/// One source's ability to answer one `Document` attribute.
///
/// A provider is a pair, not a source: `git` answers `author` and `modifiedAt`
/// but says nothing about `title`, and precedence is per field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// The adapter that would answer.
    pub source: SourceId,
    /// The attribute **as that adapter declares it**. The `Document` field it
    /// answers is whatever the source's validated mapping sends it to; the
    /// federation checks that the two agree rather than assuming they do.
    pub attr: AttrName,
    /// The fallback chain the adapter declares *inside* this provider, in its
    /// own declared order, or empty when the adapter observes the attribute
    /// exactly one way.
    ///
    /// Recorded from the adapter's declaration, never re-derived here — see the
    /// [module docs](self).
    pub declared_fallback: Vec<String>,
}

impl Provider {
    /// A provider with no adapter-internal fallback.
    fn new(source: &str, attr: &str) -> Self {
        Self {
            source: SourceId::new(source),
            attr: attr.into(),
            declared_fallback: Vec::new(),
        }
    }

    /// A provider whose adapter declares an internal fallback chain.
    fn with_fallback(source: &str, attr: &str, fallback: &[&str]) -> Self {
        Self {
            declared_fallback: fallback.iter().map(|step| (*step).to_string()).collect(),
            ..Self::new(source, attr)
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.source, self.attr)
    }
}

/// The declared order for one `Document` attribute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldPrecedence {
    /// The `Document` attribute being answered.
    pub field: AttrName,
    /// The providers that may answer it, in the order they are consulted.
    /// The first one that observed a value wins.
    pub order: Vec<Provider>,
    /// Why this order and not another. Required, and required to be about
    /// *this* field: an order without a reason is an implied precedence with
    /// extra steps.
    pub rationale: String,
}

/// The whole precedence declaration: one entry per `Document` attribute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precedence(Vec<FieldPrecedence>);

impl Precedence {
    /// Every field's declaration, in `Document` attribute-name order.
    pub fn fields(&self) -> &[FieldPrecedence] {
        &self.0
    }

    /// The declaration for one field.
    pub fn of(&self, field: &AttrName) -> Option<&FieldPrecedence> {
        self.0.iter().find(|entry| &entry.field == field)
    }
}

/// The declared precedence for the federated `Document`.
///
/// | field | order | why |
/// |---|---|---|
/// | `path` | filesystem, git, frontmatter | all three agree by construction — that agreement *is* the identity rule; the filesystem leads because it is the authority on what is on disk |
/// | `title` | frontmatter | the only source that observes one |
/// | `author` | frontmatter, git | a declared `author:` is the document's own claim; `%an` is who last committed |
/// | `modifiedAt` | filesystem, git, Drive, Dropbox | local byte time wins locally; provider time is authoritative for provider-owned bytes |
/// | `size` | filesystem, Drive, Dropbox | each storage authority observes the bytes it owns |
/// | `body` | frontmatter, Drive, Dropbox | the content reader for the owning storage authority answers |
pub fn precedence() -> Precedence {
    let fs = FilesystemSource::ID;
    let git = GitSource::ID;
    let front = FrontmatterSource::ID;
    let drive = "google-drive";
    let dropbox = "dropbox";
    Precedence(vec![
        FieldPrecedence {
            field: "author".into(),
            order: vec![Provider::new(front, "author"), Provider::new(git, "author")],
            rationale: "a YAML `author:` is the document's own statement of who wrote it. Git's \
                        `%an` is who last committed a change to the file, which is evidence of \
                        editing rather than a claim of authorship — a reviewer's typo fix would \
                        otherwise rename the author. So the document's statement wins, and the \
                        commit author answers only where the document made no statement. \
                        Frontmatter declares no fallback for `author` at all, so where both are \
                        silent the field is absent."
                .to_string(),
        },
        FieldPrecedence {
            field: "body".into(),
            order: vec![
                Provider::new(front, "body"),
                Provider::new(drive, "body"),
                Provider::new(dropbox, "body"),
            ],
            rationale: "the content reader for the owning storage authority answers the body. \
                        Frontmatter reads local Markdown, while Drive and Dropbox hydrate \
                        provider-owned content; filesystem metadata and git history cannot \
                        produce document text."
                .to_string(),
        },
        FieldPrecedence {
            field: "checksum".into(),
            order: vec![
                Provider::new(drive, "checksum"),
                Provider::new(dropbox, "contentHash"),
            ],
            rationale: "Drive and Dropbox expose provider-computed content checksums under \
                        different physical names; both map into the same read-only semantic \
                        checksum slot."
                .to_string(),
        },
        FieldPrecedence {
            field: "mediaType".into(),
            order: vec![Provider::new(drive, "mimeType")],
            rationale: "Google Drive declares a MIME type while the reviewed Dropbox connector \
                        does not expose an equivalent physical attribute."
                .to_string(),
        },
        FieldPrecedence {
            field: "modifiedAt".into(),
            order: vec![
                Provider::new(fs, "modifiedAt"),
                Provider::new(git, "modifiedAt"),
                Provider::new(drive, "modifiedAt"),
                Provider::new(dropbox, "modifiedAt"),
            ],
            rationale: "the filesystem mtime is when the bytes this `Document` was actually read \
                        from last changed — uncommitted edits included. A git commit timestamp \
                        describes the last COMMITTED state, and is absent entirely for anything \
                        untracked or staged-only. So the filesystem answers first, and git \
                        answers wherever the filesystem is not in the federation or did not see \
                        the file. The known cost of this order: a fresh clone rewrites every \
                        mtime to the checkout time, and there this field reports the checkout \
                        rather than the edit — which is a true statement about these bytes, and \
                        is why the git observation is kept on the row rather than discarded. For \
                        a provider-owned document, Drive or Dropbox answers with that storage \
                        authority's own modification timestamp."
                .to_string(),
        },
        FieldPrecedence {
            field: "path".into(),
            order: vec![
                Provider::new(fs, "path"),
                Provider::new(git, "path"),
                Provider::new(front, "path"),
                Provider::new(drive, "id"),
                Provider::new(dropbox, "path"),
            ],
            rationale: "local sources derive the same workspace-relative path from one root, so \
                        they cannot disagree; that agreement is the local identity rule. Drive's \
                        id and Dropbox's path are their respective provider-local record keys, \
                        mapped into the same required semantic identity field."
                .to_string(),
        },
        FieldPrecedence {
            field: "providerId".into(),
            order: vec![Provider::new(dropbox, "id")],
            rationale: "Dropbox exposes a stable provider-local id in addition to its path; Drive's id is already the canonical path identity for its pathless records."
                .to_string(),
        },
        FieldPrecedence {
            field: "revision".into(),
            order: vec![Provider::new(dropbox, "rev")],
            rationale: "Dropbox exposes an immutable revision token while the reviewed Drive connector does not declare an equivalent field."
                .to_string(),
        },
        FieldPrecedence {
            field: "size".into(),
            order: vec![
                Provider::new(fs, "size"),
                Provider::new(drive, "size"),
                Provider::new(dropbox, "size"),
            ],
            rationale: "the owning storage authority answers byte size: filesystem for local \
                        documents, Drive or Dropbox for provider-owned documents. Git could \
                        compute a blob size but does not declare one, and this layer does not \
                        invent capabilities its sources did not claim."
                .to_string(),
        },
        FieldPrecedence {
            field: "title".into(),
            order: vec![
                Provider::with_fallback(
                    front,
                    "title",
                    &["YAML `title:`", "the first H1, skipping code fences"],
                ),
                Provider::new(drive, "name"),
                Provider::new(dropbox, "name"),
            ],
            rationale: "frontmatter resolves a local title through its declared YAML-then-H1 \
                        fallback without re-parsing that decision here. For provider-owned \
                        documents, Drive or Dropbox supplies its declared name field. Each \
                        storage path therefore reports one observable title source."
                .to_string(),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federated::schema::ATTRIBUTES;

    /// Every attribute the concept declares has a declared order, and nothing
    /// else does. An attribute with no entry would be resolved by nothing; an
    /// entry with no attribute would order providers for a field that does not
    /// exist.
    #[test]
    fn precedence_covers_exactly_the_document_attributes() {
        let declared: Vec<String> = precedence()
            .fields()
            .iter()
            .map(|entry| entry.field.to_string())
            .collect();
        let mut expected: Vec<String> = ATTRIBUTES
            .iter()
            .map(|(name, _, _)| (*name).to_string())
            .collect();
        expected.sort();
        assert_eq!(declared, expected);
    }

    #[test]
    fn every_field_states_why_its_order_is_what_it_is() {
        for entry in precedence().fields() {
            assert!(
                entry.rationale.len() > 40,
                "`{}` has no real rationale",
                entry.field
            );
            assert!(!entry.order.is_empty(), "`{}` has no provider", entry.field);
        }
    }

    /// A field cannot list the same source twice: the second entry could never
    /// be reached, and a declaration with an unreachable line is a declaration
    /// a reader will misread.
    #[test]
    fn no_field_lists_a_source_twice() {
        for entry in precedence().fields() {
            let mut seen = std::collections::BTreeSet::new();
            for provider in &entry.order {
                assert!(
                    seen.insert(provider.source.clone()),
                    "`{}` lists `{}` twice",
                    entry.field,
                    provider.source
                );
            }
        }
    }
}
