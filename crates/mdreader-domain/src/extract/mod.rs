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
//! derivation provenance — source path, line range, content hash of exactly
//! that span, the schema and version it is typed against, the extractor id
//! and version, and a confidence. An approved fact carries its reviewer in
//! the kernel's existing [`Provenance::approved_by`].
//!
//! # Where the approval guarantee actually lives
//!
//! **In the engine, at the write boundary — not here.**
//!
//! [`CatOperation`] is a public IR type with public constructors, and a
//! review UI must be able to *see* proposed facts or it cannot render them.
//! So no amount of type-hiding in this module can make an unapproved write
//! impossible; anything that can read a proposal can rebuild the operation by
//! hand. Chasing that would trade real UI capability for a guarantee that
//! still would not hold.
//!
//! Instead, [`engine_registration`](crate::engine_registration) declares the
//! overlay schemas in
//! [`DomainRegistration::approval_required_schemas`](catdb_engine::DomainRegistration::approval_required_schemas),
//! and [`EmbeddedCatDb::apply_operations`] refuses any assertion into one of
//! them whose provenance names no reviewer — rejecting the whole batch, with
//! [`EngineError::UnapprovedFact`](catdb_engine::EngineError::UnapprovedFact).
//! That holds however the operation was constructed.
//!
//! What this module provides is **defence in depth**, and is described as
//! such: there is no convenience path to an unapproved write.
//! [`Extraction`] has exactly one method yielding [`CatOperation`]s —
//! [`Extraction::approved_operations`] — and it emits only what a human
//! accepted. Proposals are reachable through [`Extraction::preview`], whose
//! [`ProposalPreview`] produces no operations at all, and which can still be
//! turned into an in-memory instance and checked against the schema's laws,
//! so "what would approving this do?" is answerable without writing anything.
//!
//! [`Provenance::approved_by`]: catdb_core::Provenance::approved_by
//! [`EmbeddedCatDb::apply_operations`]: catdb_engine::EmbeddedCatDb::apply_operations

pub mod architecture;
pub mod decision;
mod resolve;
mod source;

use catdb_core::{MemInstance, MorphismInstance, Object, Provenance, RowId, Schema};
use catdb_engine::{LocatedViolation, SourceSpan, ViolatingObject};
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
    /// The approved facts, as engine operations — **the only way an
    /// extraction can reach durable storage.**
    ///
    /// There is deliberately no sibling that turns proposals into
    /// [`CatOperation`]s. A `CatOperation` is the engine's durable write unit
    /// ([`EmbeddedCatDb::apply_operations`] takes exactly a
    /// `Vec<CatOperation>`), so a method that produced them for unreviewed
    /// facts would put the whole approval model one function call away from
    /// being bypassed — by a caller who never read this doc comment. Use
    /// [`Self::preview`] to inspect what approving *would* mean; it cannot be
    /// applied, because it yields no operations.
    ///
    /// Objects come before morphisms so a morphism never references an object
    /// the same batch has not asserted yet, and a morphism is emitted only
    /// when **both of its endpoints are also approved**: approving an edge
    /// whose endpoints are still proposals would write a dangling edge into
    /// the authoritative graph. Under-emitting is the safe direction — the
    /// edge appears as soon as its endpoints are approved too.
    ///
    /// [`EmbeddedCatDb::apply_operations`]: catdb_engine::EmbeddedCatDb::apply_operations
    pub fn approved_operations(&self) -> Vec<CatOperation> {
        let mut ops = Vec::new();
        let mut approved_objects = std::collections::HashSet::new();
        for extracted in &self.objects {
            if extracted.review.state != ApprovalState::Approved {
                continue;
            }
            approved_objects.insert(*extracted.object.id.as_uuid());
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
        for extracted in &self.morphisms {
            if extracted.review.state != ApprovalState::Approved {
                continue;
            }
            let (source, target) = (
                *extracted.morphism.source.as_uuid(),
                *extracted.morphism.target.as_uuid(),
            );
            if !approved_objects.contains(&source) || !approved_objects.contains(&target) {
                continue;
            }
            ops.push(CatOperation::AssertMorphism {
                arrow: extracted.morphism.arrow.to_string(),
                morphism_id: *extracted.morphism.id.as_uuid(),
                source,
                target,
                provenance: to_ir_provenance(
                    &extracted.morphism.provenance,
                    extracted.review.approved_by(),
                ),
            });
        }
        ops
    }

    /// A read-only preview of everything this extraction proposes — including
    /// facts nobody has approved.
    ///
    /// This is how a review UI answers "what would approving this do?"
    /// without any of it becoming durable. See [`ProposalPreview`].
    pub fn preview(&self) -> ProposalPreview<'_> {
        ProposalPreview { extraction: self }
    }
}

/// A read-only view of an [`Extraction`]'s proposed graph.
///
/// **It yields no [`CatOperation`]s**, so there is no convenient way to
/// persist a proposal: only [`Extraction::approved_operations`] produces
/// something a durable apply accepts. This is a guardrail, not the guarantee
/// — the objects and morphisms below are public kernel values, and a caller
/// determined to write one anyway can construct the operation by hand. That
/// attempt is refused by the engine, which is where the guarantee lives; see
/// the [module docs](self#where-the-approval-guarantee-actually-lives).
///
/// A reviewer can see every proposal, build the graph they would get, and
/// find out which schema laws that graph would break — all in memory.
///
/// The approved path yields operations, which is what a durable apply takes:
///
/// ```
/// use mdreader_domain::extract::{decision, SourceDocument};
/// let corpus: Vec<SourceDocument> = Vec::new();
/// let extraction = decision::extract(&corpus);
/// let ops: Vec<catdb_ir::CatOperation> = extraction.approved_operations();
/// assert!(ops.is_empty(), "nothing approved yet");
/// ```
///
/// The preview path does not. The snippet below fails to compile, which is
/// what "no convenience path" means concretely; it differs from the one above
/// by exactly one thing — which method it reaches for.
///
/// ```compile_fail
/// use mdreader_domain::extract::{decision, SourceDocument};
/// let corpus: Vec<SourceDocument> = Vec::new();
/// let extraction = decision::extract(&corpus);
/// // `ProposalPreview` has no method that produces operations.
/// let ops: Vec<catdb_ir::CatOperation> = extraction.preview().operations();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ProposalPreview<'a> {
    extraction: &'a Extraction,
}

impl<'a> ProposalPreview<'a> {
    /// Every proposed object, rejected ones included, in extraction order.
    pub fn objects(&self) -> &'a [ExtractedObject] {
        &self.extraction.objects
    }

    /// Every proposed morphism, rejected ones included, in extraction order.
    pub fn morphisms(&self) -> &'a [ExtractedMorphism] {
        &self.extraction.morphisms
    }

    /// The graph that approving every non-rejected proposal would produce, as
    /// a kernel instance keyed by object id.
    ///
    /// Entity and arrow names are schema-local (the qualifying `schema.`
    /// prefix stripped) so the schema's own equations apply directly.
    pub fn instance(&self) -> MemInstance {
        let prefix = format!("{}.", self.extraction.schema);
        let mut instance = MemInstance::new(self.extraction.schema.clone());
        for extracted in self.objects() {
            if !extracted.review.state.is_materializable() {
                continue;
            }
            if let Some(local) = extracted.object.entity.as_str().strip_prefix(&prefix) {
                instance.add_row(local, extracted.object.id.to_string());
            }
        }
        for extracted in self.morphisms() {
            if !extracted.review.state.is_materializable() {
                continue;
            }
            if let Some(local) = extracted.morphism.arrow.as_str().strip_prefix(&prefix) {
                instance.set_fk(
                    local,
                    extracted.morphism.source.to_string(),
                    extracted.morphism.target.to_string(),
                );
            }
        }
        instance
    }

    /// Which of `schema`'s laws the proposed graph would violate, reported in
    /// the same [`LocatedViolation`] shape the engine produces for the
    /// committed graph — so a UI renders a preview violation and a real one
    /// with one code path.
    ///
    /// Nothing is written: this runs entirely over [`Self::instance`].
    pub fn check_equations(&self, schema: &Schema) -> Vec<LocatedViolation> {
        let instance = self.instance();
        let located = |row: Option<&RowId>| -> Option<ViolatingObject> {
            let row = row?.as_str();
            self.objects()
                .iter()
                .find(|extracted| extracted.object.id.to_string() == row)
                .map(|extracted| ViolatingObject {
                    id: *extracted.object.id.as_uuid(),
                    entity: extracted.object.entity.to_string(),
                    source: source_span(&extracted.object.provenance),
                })
        };
        instance
            .check_equations(schema)
            .into_iter()
            .filter_map(|violation| {
                Some(LocatedViolation {
                    schema: schema.name.to_string(),
                    law: violation.equation.to_string(),
                    lhs_path: violation
                        .equation
                        .lhs
                        .arrows
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    rhs_path: violation
                        .equation
                        .rhs
                        .arrows
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    subject: located(Some(&violation.row))?,
                    lhs: located(violation.lhs.as_ref()),
                    rhs: located(violation.rhs.as_ref()),
                })
            })
            .collect()
    }
}

/// The engine's [`SourceSpan`] shape, read out of kernel provenance.
fn source_span(provenance: &Provenance) -> Option<SourceSpan> {
    let (start, end) = provenance.range?;
    Some(SourceSpan {
        file: provenance.source.clone()?,
        start_line: Some(start),
        end_line: Some(end),
        content_hash: provenance.content_hash.clone(),
    })
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
    /// blake3 hash of **exactly** the lines `lines` names.
    ///
    /// The two fields are a pair: a range that named more (or fewer) lines
    /// than the hash covers could never be checked against it, so the hash
    /// would silently stop detecting that the cited text had drifted.
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

/// blake3 hash of a source span's text, in the `blake3:<hex>` form the
/// parser uses.
///
/// A fact's `content_hash` must always be the hash of *exactly* the lines its
/// `range` names — see [`Derivation`]. Facts cited to a whole section or a
/// whole document reuse the hash the parser already computed for that span;
/// facts cited to a single line (a link) hash that line here.
pub(crate) fn blake3_hex(text: &str) -> String {
    format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex())
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
