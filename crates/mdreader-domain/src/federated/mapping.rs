//! One real, validated schema mapping per source, plus the identity rule that
//! makes three observations one `Document`.
//!
//! # Built from the adapter's declaration, never alongside it
//!
//! A mapping's source side is [`DataSource::schema`] — the schema the adapter
//! *declared*, derived from the same `SourceAttr` list that produced its
//! capabilities. Nothing here restates what a source observes, so nothing here
//! can drift from it: a source that stops declaring `size` produces a mapping
//! without `size` on the next call, and a source that declares something the
//! `Document` concept cannot hold is a typed refusal
//! ([`FederationError::UnmappableAttribute`]) rather than a silently narrower
//! mapping.
//!
//! # Why the mappings declare no value transformations
//!
//! Not an omission — a consequence of reusing the adapters' declarations
//! instead of re-deriving them.
//!
//! The conversion this federation genuinely needs is the one the brief names:
//! a filesystem mtime (a count of milliseconds, observed as text) and a git
//! commit timestamp (seconds, observed as text) both becoming a typed
//! `Timestamp`. That conversion already exists, as data, in
//! `catdb-connectors`: `SourceAttr::parsed("modifiedAt", BaseType::Timestamp)`
//! attaches a [`ValueTransformation::Parse`] to the attribute, and
//! `SourceModel::type_row` applies it to every row. The adapter's declared
//! schema therefore already says `modifiedAt: Timestamp`.
//!
//! A mapping's transformation chain runs between the two *declared* types. So
//! restating that `Parse` here would be a second, contradictory claim — the
//! value does not arrive as `Text`, it arrives as `Timestamp` — and
//! [`Mapping::validate`] refuses it with
//! [`MappingError::TransformationChainBroken`](catdb_sdk::core::MappingError::TransformationChainBroken).
//! That refusal is pinned by a test, because "there are no transformations
//! here" and "the transformation is already declared upstream and duplicating
//! it is an error" look identical until you try it.
//!
//! What matters is that [`ValueTransformation`] stays the *only* conversion
//! mechanism in this module and in the assembler: values move from a source
//! row into a `Document` slot through
//! [`apply_chain`](catdb_sdk::core::apply_chain) over the mapping's own declared
//! chain, whatever that chain happens to be. There is no per-field code for a
//! chain to compete with, so a future source that does need one gets it by
//! declaring it.
//!
//! # Why write capability is declared, not defaulted
//!
//! [`MappingWriteCapability::ReadOnly`] is the enum's default, and it is still
//! written out explicitly on every mapping: "sources are never modified" is
//! the premise of the whole exercise, and a premise that holds only because
//! nobody changed a default is not a premise anyone can check.
//!
//! [`DataSource::schema`]: catdb_sdk::connectors::DataSource::schema
//! [`Mapping::validate`]: catdb_sdk::core::Mapping::validate
//! [`ValueTransformation`]: catdb_sdk::core::ValueTransformation
//! [`ValueTransformation::Parse`]: catdb_sdk::core::ValueTransformation::Parse

use catdb_sdk::connectors::DataSource;
use catdb_sdk::core::{
    AttrTarget, IdentityConfidence, IdentityKey, IdentityRule, Mapping, MappingWriteCapability,
    Path, PathProver, Schema, ValidMapping,
};

use super::schema::{document_schema, ENTITY};
use super::FederationError;

/// The attribute every adapter declares as its natural key, and the key the
/// identity rule compares.
pub const KEY: &str = "path";

/// The mapping from one source's declared schema into the federated
/// `Document`.
///
/// Total by construction over whatever the source declares: every declared
/// entity and attribute gets an image, or the whole mapping is refused.
pub fn source_mapping(source: &dyn DataSource) -> Result<Mapping, FederationError> {
    let src = source.schema();
    let dst = document_schema();
    let id = source.id().clone();

    // A source that describes something other than exactly `Document` is not
    // describing this concept, and pretending otherwise would merge two
    // entities into one image — coverage's `EntitiesMerged`, i.e. real loss.
    let entities: Vec<String> = src.entities.iter().map(|e| e.to_string()).collect();
    if entities != [ENTITY] {
        return Err(FederationError::UnexpectedEntities {
            source_id: id,
            found: entities,
        });
    }

    let mut mapping = Mapping::new(src.name.clone(), dst.name.clone());
    mapping.entity_map.insert(ENTITY.into(), ENTITY.into());
    // A source declaring an arrow would be declaring structure the `Document`
    // concept does not have; none of the three does, and the check is here so
    // that one which did could not be silently dropped (a missing arrow image
    // is not a validation error the source's own name would appear in).
    if let Some(arrow) = src.arrows.keys().next() {
        return Err(FederationError::UnmappableArrow {
            source_id: id,
            arrow: arrow.to_string(),
        });
    }

    for attr in src.attributes.values() {
        let Some(target) = dst.attributes.get(&attr.name) else {
            return Err(FederationError::UnmappableAttribute {
                source_id: id,
                attr: attr.name.to_string(),
                detail: "the `Document` concept declares no attribute of that name".to_string(),
            });
        };
        // Same-name is the rule here, but the *types* still have to agree, and
        // saying so up front turns a validation error into a message naming
        // the source that caused it.
        if src.typeside.resolve(&attr.ty) != dst.typeside.resolve(&target.ty) {
            return Err(FederationError::UnmappableAttribute {
                source_id: id,
                attr: attr.name.to_string(),
                detail: format!(
                    "the source declares it `{}` but the `Document` concept holds `{}`",
                    attr.ty, target.ty
                ),
            });
        }
        mapping.attr_map.insert(
            attr.name.clone(),
            AttrTarget {
                path: Path::identity(ENTITY),
                attr: attr.name.clone(),
            },
        );
    }

    // A source that cannot observe the key cannot have its rows identified
    // with anyone else's, and a rule naming an attribute the source does not
    // declare would not validate. An unavailable source is exactly this case:
    // it declares the entity so errors can name it, and observes nothing.
    if src
        .attributes
        .contains_key(&catdb_sdk::core::AttrName::from(KEY))
    {
        mapping.identities.push(path_identity_rule());
    }
    mapping.write_capability = MappingWriteCapability::ReadOnly;
    Ok(mapping)
}

/// [`source_mapping`], put through [`Mapping::validate`] with a real prover.
///
/// The [`ValidMapping`] is the witness the federation holds: totality, image
/// typing, transformation typing and identity-rule well-formedness were all
/// checked by the kernel, not asserted here.
pub fn validated_source_mapping(
    source: &dyn DataSource,
    prover: &dyn PathProver,
) -> Result<ValidMapping, FederationError> {
    let mapping = source_mapping(source)?;
    validate_against(source.id().clone(), &mapping, source.schema(), prover)
}

/// Validate an already-built mapping, reporting the source that owns it.
pub(super) fn validate_against(
    source: catdb_sdk::connectors::SourceId,
    mapping: &Mapping,
    src: &Schema,
    prover: &dyn PathProver,
) -> Result<ValidMapping, FederationError> {
    mapping
        .validate(src, &document_schema(), prover)
        .map_err(|errors| FederationError::MappingInvalid {
            source_id: source,
            detail: errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
            errors,
        })
}

/// The rule that says a row one adapter produced denotes the same `Document`
/// as a row another adapter produced.
///
/// # Why path equality is the right rule *here*
///
/// Not because paths are globally safe keys — they are not. Two files with the
/// same relative path under different roots are different documents, and a
/// path is not stable across a rename. The rule is sound in this federation
/// for a narrower, checkable reason:
///
/// * all three adapters are rooted at the **same** directory, and each one
///   derives its key from its own read of that directory (a walk, or
///   `git ls-files` run inside it), so the key is workspace-relative in the
///   same sense for all three;
/// * the workspace-relative path is each adapter's *declared* natural key —
///   `SourceRow::key`, the thing `catdb-connectors` already states two sources
///   describing the same document agree on;
/// * within one root, that key is a key **by construction**: a directory
///   cannot hold two distinct files at one relative path.
///
/// That is precisely [`IdentityConfidence::Known`] — "an exact match on
/// something that is a key by construction" — and the precondition it rests on
/// (one shared root) is written into the rule's `basis`, because a rule a human
/// has to rule on is worthless without its stated basis. Federating two
/// *different* roots would break the precondition, and the honest response
/// would be a different rule at a lower tier, not this one applied wider.
///
/// # What the rule does not do
///
/// It does not merge. [`IdentityRule::propose`] yields an
/// [`IdentityClaim`](catdb_sdk::core::IdentityClaim) about two ids; both records
/// keep their identity and both stay retrievable — in this crate, as the
/// [`Document::observations`](crate::federated::Document::observations) the
/// assembled document carries verbatim.
pub fn path_identity_rule() -> IdentityRule {
    IdentityRule::SharedKey {
        src_entity: ENTITY.into(),
        canonical_entity: ENTITY.into(),
        keys: vec![IdentityKey {
            src: KEY.into(),
            canonical: KEY.into(),
        }],
        confidence: IdentityConfidence::Known,
        basis: "every adapter declares the workspace-relative path as its natural key and all \
                sources in this federation read the SAME root, so within that root the path is a \
                key by construction; federating a second root would invalidate the precondition, \
                not merely weaken it"
            .to_string(),
    }
}
