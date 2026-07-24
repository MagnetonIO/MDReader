//! Rules-based, non-destructive overlay extraction.
//!
//! # What an overlay is
//!
//! Markdown is the source of truth. A schema is a **non-destructive
//! overlay**: a typed interpretation laid *over* the corpus, never written
//! back into it. One file can participate in several overlays at once — the
//! same ADR is a `document_overlay.Document`, a `decision_overlay.Decision`,
//! and (if it declares a component) an `architecture_overlay.Component` —
//! without the file being rewritten, moved, or annotated. Nothing in this
//! module opens a file for writing; extraction consumes an already-parsed,
//! in-memory [`SourceDocument`] view and returns values.
//!
//! # Evidence vs. assertion
//!
//! A markdown link is *syntactic evidence*: `[[0007-storage]]` says two files
//! mention each other and nothing more. A CatDB morphism is a *typed semantic
//! assertion*: `supersedes : Decision → Decision` says one decision replaces
//! another. Promoting the former to the latter is exactly where extraction
//! can lie, so the rules here only do it when the markdown says so
//! explicitly (a link under a `## Supersedes` heading, a component's own
//! `## Depends on` section), and each rule records why it fired.
//!
//! Where a rule would have to guess author intent it is **left out**. See
//! [`decision`] and [`architecture`] for the per-extractor lists of what is
//! deliberately not extracted. Under-extracting is the correct failure mode:
//! a missing object costs a user one manual entry, a wrong one costs them
//! their trust in every other object.
//!
//! # Nothing is authoritative by fiat
//!
//! Every extracted fact starts [`ApprovalState::Proposed`] and carries full
//! derivation provenance — source path, line range, content hash of that
//! span, the schema and version it is typed against, the extractor id and
//! version, and a confidence. A rejected proposal is never materialized into
//! the graph; an approved one carries its reviewer in the kernel's existing
//! [`Provenance::approved_by`].
//!
//! [`Provenance::approved_by`]: catdb_core::Provenance::approved_by

pub mod architecture;
pub mod decision;
mod resolve;
mod source;

use catdb_core::{MorphismInstance, Object, Provenance};
use catdb_ir::CatOperation;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use resolve::{resolve_document, slug, split_fragment, ResolvedTarget};
pub use source::{SourceDocument, SourceLink, SourceSection};

/// Version of the overlay schemas this module extracts against, recorded on
/// every fact's [`Provenance::schema_version`]. Bump it whenever an overlay
/// schema's entities, arrows, or attributes change, so previously stored
/// facts can be told apart from ones typed against the new shape.
pub const OVERLAY_SCHEMA_VERSION: &str = "1";

/// Whether a human has accepted an extracted fact.
///
/// Extraction proposes; a human disposes. This is a real state machine, not
/// a boolean, because "nobody has looked at this yet" and "somebody looked
/// and said no" are different facts about the corpus, and a re-run must not
/// silently re-propose what was already rejected.
///
/// ```text
///            approve                  approve
///   Rejected ────────► Approved ◄──────────── Proposed
///          ◄────────           ────────────►
///            reject                   reject
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    /// Extracted but not yet reviewed. The default for every new fact.
    #[default]
    Proposed,
    /// A human accepted it. Only approved facts are authoritative.
    Approved,
    /// A human rejected it. Never materialized into the graph.
    Rejected,
}

impl ApprovalState {
    /// Whether a fact in this state may be asserted into the graph at all.
    /// Rejected facts never are.
    pub fn is_materializable(self) -> bool {
        !matches!(self, Self::Rejected)
    }
}

/// Why an approval transition was refused.
///
/// Re-approving what is already approved (or re-rejecting a rejection) is an
/// error rather than a silent no-op: it would overwrite the recorded
/// reviewer with a second one and lose who actually made the call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalError {
    /// The fact is already approved (by the named reviewer).
    #[error("already approved by `{0}`")]
    AlreadyApproved(String),
    /// The fact is already rejected.
    #[error("already rejected")]
    AlreadyRejected,
}

/// A review record: the approval state plus who set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Review {
    /// Current state.
    pub state: ApprovalState,
    /// The reviewer who moved it out of [`ApprovalState::Proposed`].
    pub reviewer: Option<String>,
}

impl Review {
    /// Accept the fact. Legal from `Proposed` and from `Rejected` (a
    /// reviewer may reverse their own rejection); refused when already
    /// approved.
    pub fn approve(&mut self, reviewer: impl Into<String>) -> Result<(), ApprovalError> {
        if self.state == ApprovalState::Approved {
            return Err(ApprovalError::AlreadyApproved(
                self.reviewer.clone().unwrap_or_default(),
            ));
        }
        self.state = ApprovalState::Approved;
        self.reviewer = Some(reviewer.into());
        Ok(())
    }

    /// Reject the fact. Legal from `Proposed` and from `Approved` (revoking
    /// an approval); refused when already rejected.
    pub fn reject(&mut self, reviewer: impl Into<String>) -> Result<(), ApprovalError> {
        if self.state == ApprovalState::Rejected {
            return Err(ApprovalError::AlreadyRejected);
        }
        self.state = ApprovalState::Rejected;
        self.reviewer = Some(reviewer.into());
        Ok(())
    }

    /// The reviewer to stamp on [`Provenance::approved_by`]: `Some` only when
    /// the fact is actually approved, so provenance never claims review for
    /// a rejected or unreviewed fact.
    fn approved_by(&self) -> Option<String> {
        match self.state {
            ApprovalState::Approved => self.reviewer.clone(),
            _ => None,
        }
    }
}

/// An extracted object: the kernel object, the rule that produced it, and
/// its review state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedObject {
    /// The kernel object, entity-qualified and carrying full provenance.
    pub object: Object,
    /// Id of the rule that fired (e.g. `D1`), for auditing and for showing a
    /// reviewer *why* this was proposed.
    pub rule: String,
    /// Review state.
    pub review: Review,
}

/// An extracted morphism: the kernel morphism instance, the rule that
/// produced it, and its review state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedMorphism {
    /// The kernel morphism instance, arrow-qualified and carrying full
    /// provenance.
    pub morphism: MorphismInstance,
    /// Id of the rule that fired.
    pub rule: String,
    /// Review state.
    pub review: Review,
}

/// The result of running one extractor over a corpus.
///
/// Ordering is deterministic: objects and morphisms come out in corpus order,
/// then document order, so golden tests can pin exact output.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Extraction {
    /// The extractor that produced this, e.g. `mdreader.decision@1`.
    pub extractor: String,
    /// The overlay schema the facts are typed against.
    pub schema: String,
    /// Proposed objects, in deterministic order.
    pub objects: Vec<ExtractedObject>,
    /// Proposed morphisms, in deterministic order.
    pub morphisms: Vec<ExtractedMorphism>,
}

impl Extraction {
    /// Every materializable fact (everything not rejected) as engine
    /// operations, objects before morphisms so a morphism never references
    /// an object the same batch has not asserted yet.
    ///
    /// Proposals are included deliberately: a review UI needs the proposed
    /// graph in order to show what approving would mean — including which
    /// schema laws it would violate. Their provenance carries no
    /// `approved_by`, so an unreviewed fact is always distinguishable from
    /// an approved one.
    pub fn operations(&self) -> Vec<CatOperation> {
        self.operations_matching(ApprovalState::is_materializable)
    }

    /// Only the facts a human has approved — the authoritative subset.
    pub fn approved_operations(&self) -> Vec<CatOperation> {
        self.operations_matching(|state| state == ApprovalState::Approved)
    }

    fn operations_matching(&self, keep: fn(ApprovalState) -> bool) -> Vec<CatOperation> {
        let mut ops = Vec::new();
        for extracted in self.objects.iter().filter(|o| keep(o.review.state)) {
            ops.push(CatOperation::AssertObject {
                entity: extracted.object.entity.to_string(),
                object_id: *extracted.object.id.as_uuid(),
                value: extracted.object.value.clone(),
                provenance: to_ir_provenance(
                    &extracted.object.provenance,
                    extracted.review.approved_by(),
                ),
            });
        }
        for extracted in self.morphisms.iter().filter(|m| keep(m.review.state)) {
            ops.push(CatOperation::AssertMorphism {
                arrow: extracted.morphism.arrow.to_string(),
                morphism_id: *extracted.morphism.id.as_uuid(),
                source: *extracted.morphism.source.as_uuid(),
                target: *extracted.morphism.target.as_uuid(),
                provenance: to_ir_provenance(
                    &extracted.morphism.provenance,
                    extracted.review.approved_by(),
                ),
            });
        }
        ops
    }
}

/// Translate kernel provenance to the wire form the operation log stores,
/// stamping the reviewer at materialization time (approval happens after
/// extraction, so the kernel object built by a rule never carries one).
fn to_ir_provenance(provenance: &Provenance, approved_by: Option<String>) -> catdb_ir::Provenance {
    catdb_ir::Provenance {
        source: provenance.source.clone(),
        range: provenance.range.map(|(start, end)| [start, end]),
        content_hash: provenance.content_hash.clone(),
        schema_version: provenance.schema_version.clone(),
        extractor: provenance.extractor.clone(),
        confidence: provenance.confidence,
        approved_by,
    }
}

/// The full derivation record every extracted fact carries.
///
/// Constructing provenance through this (rather than filling a struct at
/// each call site) is what makes "every object is locatable and auditable" a
/// property of the module instead of a habit: there is no way to build a
/// fact here without naming its file, its line range, the hash of that span,
/// the schema version, the extractor, and a confidence.
pub(crate) struct Derivation<'a> {
    /// Extractor id and version, e.g. `mdreader.decision@1`.
    pub extractor: &'a str,
    /// Source file path, repo-relative.
    pub file: &'a str,
    /// Inclusive 1-based line range of the span the fact was read from.
    pub lines: (u32, u32),
    /// blake3 hash of that span, as recorded by the parser.
    pub content_hash: &'a str,
    /// Extractor confidence in `[0, 1]`.
    pub confidence: f32,
}

impl Derivation<'_> {
    fn provenance(&self) -> Provenance {
        Provenance {
            source: Some(self.file.to_string()),
            range: Some(self.lines),
            content_hash: Some(self.content_hash.to_string()),
            schema_version: Some(OVERLAY_SCHEMA_VERSION.to_string()),
            extractor: Some(self.extractor.to_string()),
            confidence: Some(self.confidence),
            // Never set at extraction time: approval is a later, human act.
            approved_by: None,
        }
    }
}

/// Deterministic UUID from labelled parts (blake3-derived), so re-extracting
/// an unchanged corpus produces byte-identical ids and the engine's
/// idempotent upserts are genuine no-ops.
pub(crate) fn stable_uuid(parts: &[&str]) -> Uuid {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part.as_bytes());
        // Length-delimit so ["ab", "c"] and ["a", "bc"] cannot collide.
        hasher.update(&[0]);
    }
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    Uuid::from_bytes(bytes)
}

/// Build an extracted object under a rule.
pub(crate) fn object(
    schema: &str,
    entity: &str,
    id: Uuid,
    value: serde_json::Value,
    rule: &str,
    derivation: &Derivation<'_>,
) -> ExtractedObject {
    ExtractedObject {
        object: Object {
            id: catdb_core::ObjectId::from_uuid(id),
            entity: format!("{schema}.{entity}").into(),
            value,
            provenance: derivation.provenance(),
        },
        rule: rule.to_string(),
        review: Review::default(),
    }
}

/// Build an extracted morphism under a rule.
pub(crate) fn morphism(
    schema: &str,
    arrow: &str,
    source: Uuid,
    target: Uuid,
    rule: &str,
    derivation: &Derivation<'_>,
) -> ExtractedMorphism {
    ExtractedMorphism {
        morphism: MorphismInstance {
            id: catdb_core::MorphismId::from_uuid(stable_uuid(&[
                schema,
                arrow,
                &source.to_string(),
                &target.to_string(),
            ])),
            arrow: format!("{schema}.{arrow}").into(),
            source: catdb_core::ObjectId::from_uuid(source),
            target: catdb_core::ObjectId::from_uuid(target),
            provenance: derivation.provenance(),
        },
        rule: rule.to_string(),
        review: Review::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_fact_is_proposed_and_unreviewed() {
        let review = Review::default();
        assert_eq!(review.state, ApprovalState::Proposed);
        assert_eq!(review.approved_by(), None);
    }

    #[test]
    fn approval_transitions_are_explicit_and_reversible() {
        let mut review = Review::default();
        review.approve("alice").unwrap();
        assert_eq!(review.state, ApprovalState::Approved);
        assert_eq!(review.approved_by().as_deref(), Some("alice"));

        // Revoking an approval is legal and clears `approved_by`.
        review.reject("bob").unwrap();
        assert_eq!(review.state, ApprovalState::Rejected);
        assert_eq!(review.approved_by(), None);

        // Reversing a rejection is legal too.
        review.approve("carol").unwrap();
        assert_eq!(review.approved_by().as_deref(), Some("carol"));
    }

    #[test]
    fn redundant_transitions_are_refused_rather_than_overwriting_the_reviewer() {
        let mut review = Review::default();
        review.approve("alice").unwrap();
        assert_eq!(
            review.approve("mallory"),
            Err(ApprovalError::AlreadyApproved("alice".into()))
        );
        assert_eq!(review.reviewer.as_deref(), Some("alice"));

        let mut review = Review::default();
        review.reject("alice").unwrap();
        assert_eq!(
            review.reject("mallory"),
            Err(ApprovalError::AlreadyRejected)
        );
        assert_eq!(review.reviewer.as_deref(), Some("alice"));
    }

    #[test]
    fn rejected_facts_are_never_materializable() {
        assert!(ApprovalState::Proposed.is_materializable());
        assert!(ApprovalState::Approved.is_materializable());
        assert!(!ApprovalState::Rejected.is_materializable());
    }

    #[test]
    fn stable_uuid_is_deterministic_and_unambiguously_delimited() {
        assert_eq!(stable_uuid(&["a", "b"]), stable_uuid(&["a", "b"]));
        assert_ne!(stable_uuid(&["ab", "c"]), stable_uuid(&["a", "bc"]));
    }

    #[test]
    fn derivation_fills_every_provenance_field_except_approval() {
        let derivation = Derivation {
            extractor: "mdreader.test@1",
            file: "docs/a.md",
            lines: (3, 9),
            content_hash: "blake3:abc",
            confidence: 0.75,
        };
        let provenance = derivation.provenance();
        assert_eq!(provenance.source.as_deref(), Some("docs/a.md"));
        assert_eq!(provenance.range, Some((3, 9)));
        assert_eq!(provenance.content_hash.as_deref(), Some("blake3:abc"));
        assert_eq!(provenance.schema_version.as_deref(), Some("1"));
        assert_eq!(provenance.extractor.as_deref(), Some("mdreader.test@1"));
        assert_eq!(provenance.confidence, Some(0.75));
        assert_eq!(provenance.approved_by, None);
    }
}
