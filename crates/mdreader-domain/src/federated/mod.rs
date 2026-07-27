//! The federated `Document`: one semantic concept, assembled from sources
//! CatDB does not own, with provenance naming which source answered each
//! field.
//!
//! ```text
//! Document.title       <- YAML frontmatter | first H1        (frontmatter)
//! Document.modifiedAt  <- filesystem mtime | commit timestamp (filesystem, git)
//! Document.author      <- YAML frontmatter | commit author    (frontmatter, git)
//! Document.path        <- the natural key all three agree on
//! ```
//!
//! # Where the parts come from
//!
//! Almost nothing here is new. The three adapters, their honest capability
//! declarations, the value-transformation language, mapping validation,
//! coverage analysis and identity rules are all the platform's, and this module
//! is the *domain-side* statement that binds them into one concept:
//!
//! | piece | who owns it | what this module adds |
//! |---|---|---|
//! | reading a source | `catdb-connectors` | nothing — it calls `read` |
//! | executing one query across all of them, and reporting how | `catdb-federation` | nothing — [`Federation::assemble_from`] attributes the answer it produced |
//! | what a source observes | the adapter's `SourceAttr` declaration | nothing — [`mapping::source_mapping`] reads it |
//! | typing a raw observation | `ValueTransformation` on the `SourceAttr` | nothing — see [`mapping`] for why the mappings restate none of it |
//! | is this mapping sound | `Mapping::validate` | the mapping, and the schema it lands in |
//! | what a mapping loses | `coverage` | [`Federation::coverage`], per source |
//! | are these rows one thing | `IdentityRule` | the rule, and its justification |
//! | which source won | — | [`precedence`] and [`attribution`] |
//!
//! # Read-only, structurally
//!
//! Every mapping declares [`MappingWriteCapability::ReadOnly`](catdb_core::MappingWriteCapability::ReadOnly),
//! every adapter opens files for reading, and the tests hash the fixture tree
//! before and after every assembly. Nothing in this module has a path to a
//! writer.
//!
//! # What this module deliberately does not do
//!
//! It does not federate *filtering*. [`Federation::scope`] narrows by `path`
//! and refuses anything else, because the natural key is the one thing every
//! source both observes and can push down, and splitting a richer query into a
//! pushed half and a residual half is
//! [`catdb_connectors::SourcePlan`]'s job — already built, and not worth a
//! second implementation here that could disagree with it.

pub mod attribution;
pub mod mapping;
pub mod precedence;
pub mod schema;

pub use attribution::{Document, FieldAttribution, ProviderVerdict};
pub use mapping::{path_identity_rule, source_mapping, validated_source_mapping};
pub use precedence::{precedence, FieldPrecedence, Precedence, Provider};
pub use schema::{document_schema, qualified_entity, ATTRIBUTES, ENTITY, SCHEMA};

use std::collections::{BTreeMap, BTreeSet};

use catdb_connectors::{
    federate, read, Availability, DataSource, ReadContext, SourceError, SourceId, SourceRow,
};
use catdb_core::{
    apply_chain, coverage, AttrName, IdentityRefusal, IdentityRule, MappingCoverage, MappingError,
    ObjectId, PathProver, Schema, ValidMapping,
};
use catdb_federation::{Federated, Outcome};
use catdb_query::{parse_query, validate_entity, EntityQuery, EntityTarget, EntityVocabulary};
use uuid::Uuid;

/// Why a federated `Document` could not be declared or assembled.
///
/// Every variant means the answer would have been *wrong*, not merely empty.
/// As in `catdb-connectors`, there is no variant meaning "nothing matched": an
/// empty corpus assembles into zero documents, successfully.
#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    /// No sources were given, so there is nothing to federate.
    #[error("a federation needs at least one source")]
    NoSources,
    /// The same source id appears twice, so provenance would be ambiguous.
    #[error("source `{0}` appears twice in the federation")]
    DuplicateSource(SourceId),
    /// A source describes entities other than the one `Document` concept.
    #[error(
        "source `{source_id}` describes {found:?}, but the federated concept is exactly one entity `{}`",
        schema::ENTITY
    )]
    UnexpectedEntities {
        /// The source that disagrees.
        source_id: SourceId,
        /// What it declares instead.
        found: Vec<String>,
    },
    /// A source declares an arrow, which the `Document` concept has no
    /// structure to receive.
    #[error(
        "source `{source_id}` declares arrow `{arrow}`, which the `Document` concept cannot hold"
    )]
    UnmappableArrow {
        /// The source that declares it.
        source_id: SourceId,
        /// The arrow.
        arrow: String,
    },
    /// A source declares an attribute the `Document` concept cannot hold.
    #[error("source `{source_id}` declares `{attr}`: {detail}")]
    UnmappableAttribute {
        /// The source that declares it.
        source_id: SourceId,
        /// The attribute.
        attr: String,
        /// Why it cannot land.
        detail: String,
    },
    /// The kernel refused a source's mapping.
    #[error("the mapping from `{source_id}` into the federated `Document` is invalid: {detail}")]
    MappingInvalid {
        /// The source whose mapping failed.
        source_id: SourceId,
        /// The joined error messages.
        detail: String,
        /// The kernel's own errors.
        errors: Vec<MappingError>,
    },
    /// A source's mapping carries an attribute no precedence entry consults,
    /// so the data would arrive and never be read.
    #[error(
        "source `{source_id}` maps `{attr}` onto `{field}`, but no declared precedence for `{field}` \
         consults it: the value would be fetched and silently ignored"
    )]
    MappedButNotDeclared {
        /// The source.
        source_id: SourceId,
        /// Its attribute.
        attr: String,
        /// The `Document` field it lands on.
        field: String,
    },
    /// A precedence entry names a provider that source's mapping does not
    /// support, so the order describes a federation that was not built.
    #[error(
        "precedence for `{field}` consults `{source_id}.{attr}`, but that source's mapping does not \
         send `{attr}` there"
    )]
    PrecedenceNotMapped {
        /// The `Document` field.
        field: String,
        /// The source named.
        source_id: SourceId,
        /// The attribute named.
        attr: String,
    },
    /// A source refused, or could not complete, a read.
    #[error(transparent)]
    Source(#[from] SourceError),
    /// The scope query is not valid against the federated vocabulary.
    #[error(transparent)]
    Query(#[from] catdb_query::QueryError),
    /// The scope filters on something other than the natural key.
    #[error(
        "a federation scope may only narrow `{}` — the natural key every source observes and can \
         push down; `{attr}` is not it, and applying it here would re-implement the connector's \
         pushed/residual split",
        mapping::KEY
    )]
    ScopeNotPath {
        /// The attribute the scope tried to filter on.
        attr: String,
    },
    /// A row arrived without the natural key, so it cannot be identified with
    /// any other source's view of the same document.
    #[error(
        "source `{source_id}` returned a row (`{key}`) with no usable `{}`",
        mapping::KEY
    )]
    KeylessRow {
        /// The source.
        source_id: SourceId,
        /// The row's own key, for locating it.
        key: String,
    },
    /// One source produced two rows for one key, so there is no single
    /// observation to attribute.
    #[error("source `{source_id}` returned two rows for `{key}`")]
    DuplicateKey {
        /// The source.
        source_id: SourceId,
        /// The contested key.
        key: String,
    },
    /// The federated answer being attributed does not describe this
    /// federation's sources, so attributing it would drop observations or
    /// credit rows to a source that was never bound.
    #[error(
        "this federation binds {federated:?}, but the answer reports on {answered:?}: attributing \
         it would silently drop observations, or credit rows to a source with no declared mapping"
    )]
    SourceSetMismatch {
        /// The sources this federation binds, in name order.
        federated: Vec<String>,
        /// The sources the answer accounts for, in name order.
        answered: Vec<String>,
    },
    /// The answer says a source refused, and carries rows from it anyway.
    ///
    /// Attributing them would put a value on a `Document` field whose provider
    /// the same answer records as having never run.
    #[error(
        "source `{source_id}` is reported as having refused ({reason}), but the answer carries \
         {rows} row(s) from it"
    )]
    RefusedButAnswered {
        /// The source.
        source_id: SourceId,
        /// The refusal, as the answer reports it.
        reason: String,
        /// How many rows it nevertheless produced.
        rows: usize,
    },
    /// A declared transformation chain could not be applied to an observed
    /// value.
    #[error("source `{source_id}` observed `{attr}` for `{key}`, but the mapping's chain refused it: {detail}")]
    Transformation {
        /// The source.
        source_id: SourceId,
        /// The document.
        key: String,
        /// The attribute.
        attr: String,
        /// The transformation's own message.
        detail: String,
    },
    /// The identity rule refused to link an observation to its document.
    #[error("the identity rule refused `{key}`: {refusal}")]
    IdentityRefused {
        /// The document.
        key: String,
        /// The kernel's refusal.
        refusal: IdentityRefusal,
    },
}

/// One source bound into the federation, with the validated mapping that puts
/// its attributes into the `Document`.
pub struct Binding<'a> {
    source: &'a dyn DataSource,
    mapping: ValidMapping,
}

impl<'a> Binding<'a> {
    /// The source.
    pub fn source(&self) -> &'a dyn DataSource {
        self.source
    }

    /// Its name.
    pub fn id(&self) -> &SourceId {
        self.source.id()
    }

    /// The validated mapping from its declared schema into `document`.
    pub fn mapping(&self) -> &ValidMapping {
        &self.mapping
    }

    /// Whether the source can be read here, and why not when it cannot.
    pub fn availability(&self) -> &Availability {
        self.source.capabilities().availability()
    }
}

/// The federated `Document`: a declared schema, one validated mapping per
/// source, a declared precedence, and one identity rule.
///
/// Constructing one is the check. Every mapping is validated by the kernel,
/// the precedence and the mappings are pinned against each other in both
/// directions, and the sources are federated (so two of them declaring the
/// same attribute with different types is a refusal, not a silent winner).
pub struct Federation<'a> {
    bindings: Vec<Binding<'a>>,
    document: Schema,
    source_vocabulary: Schema,
    precedence: Precedence,
    identity: IdentityRule,
}

impl<'a> Federation<'a> {
    /// Declare a federation over these sources, validating everything.
    pub fn new(
        sources: Vec<&'a dyn DataSource>,
        prover: &dyn PathProver,
    ) -> Result<Self, FederationError> {
        if sources.is_empty() {
            return Err(FederationError::NoSources);
        }
        let mut seen = BTreeSet::new();
        for source in &sources {
            if !seen.insert(source.id().clone()) {
                return Err(FederationError::DuplicateSource(source.id().clone()));
            }
        }
        // The sources must agree about what each attribute *means* before any
        // mapping is worth validating; `federate` is where a type disagreement
        // becomes a typed refusal instead of a silent winner.
        let source_vocabulary = federate(&sources)?;

        let mut bindings = Vec::new();
        for source in sources {
            let mapping = validated_source_mapping(source, prover)?;
            bindings.push(Binding { source, mapping });
        }

        let federation = Self {
            bindings,
            document: document_schema(),
            source_vocabulary,
            precedence: precedence(),
            identity: path_identity_rule(),
        };
        federation.check_precedence_against_mappings()?;
        Ok(federation)
    }

    /// The precedence declaration and the validated mappings must describe the
    /// same federation, in both directions: nothing mapped that no order
    /// consults, and nothing consulted that no mapping carries.
    ///
    /// Without the first check a source could contribute data that is fetched
    /// and silently dropped. Without the second, an order could name a source
    /// that never had the attribute, and a reader of the declaration would
    /// believe a fallback exists that cannot fire.
    fn check_precedence_against_mappings(&self) -> Result<(), FederationError> {
        for binding in &self.bindings {
            // An unavailable source declares no attributes at all; it is
            // exempt from both directions, which is exactly how "git is not a
            // repository here" stays a declaration rather than a failure.
            if !matches!(binding.availability(), Availability::Available) {
                continue;
            }
            for (attr, target) in &binding.mapping.mapping().attr_map {
                let field = target.attr.to_string();
                let consulted = self.precedence.of(&target.attr).is_some_and(|entry| {
                    entry
                        .order
                        .iter()
                        .any(|p| &p.source == binding.id() && &p.attr == attr)
                });
                if !consulted {
                    return Err(FederationError::MappedButNotDeclared {
                        source_id: binding.id().clone(),
                        attr: attr.to_string(),
                        field,
                    });
                }
            }
        }
        for entry in self.precedence.fields() {
            for provider in &entry.order {
                let Some(binding) = self.binding(&provider.source) else {
                    continue;
                };
                if !matches!(binding.availability(), Availability::Available) {
                    continue;
                }
                let mapped = binding
                    .mapping
                    .mapping()
                    .attr_map
                    .get(&provider.attr)
                    .is_some_and(|target| target.attr == entry.field);
                if !mapped {
                    return Err(FederationError::PrecedenceNotMapped {
                        field: entry.field.to_string(),
                        source_id: provider.source.clone(),
                        attr: provider.attr.to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// The federated `Document` schema.
    pub fn schema(&self) -> &Schema {
        &self.document
    }

    /// The union schema the sources agree on — the vocabulary a scope query is
    /// stated against.
    pub fn source_vocabulary(&self) -> &Schema {
        &self.source_vocabulary
    }

    /// Every bound source, in federation order.
    pub fn bindings(&self) -> &[Binding<'a>] {
        &self.bindings
    }

    /// One bound source, by name.
    pub fn binding(&self, id: &SourceId) -> Option<&Binding<'a>> {
        self.bindings.iter().find(|binding| binding.id() == id)
    }

    /// The declared precedence.
    pub fn precedence(&self) -> &Precedence {
        &self.precedence
    }

    /// The identity rule that makes several observations one document.
    pub fn identity_rule(&self) -> &IdentityRule {
        &self.identity
    }

    /// What each source's mapping carries and what it leaves unreached, from
    /// the kernel's own coverage analysis.
    pub fn coverage(&self) -> Vec<(SourceId, MappingCoverage)> {
        self.bindings
            .iter()
            .map(|binding| {
                (
                    binding.id().clone(),
                    coverage(
                        binding.mapping.mapping(),
                        binding.source.schema(),
                        &self.document,
                    ),
                )
            })
            .collect()
    }

    /// The sources that declared themselves unavailable here, with their
    /// reasons. They are never read and never stand in an empty answer.
    pub fn unavailable(&self) -> Vec<(SourceId, String)> {
        self.bindings
            .iter()
            .filter_map(|binding| match binding.availability() {
                Availability::Unavailable { reason } => {
                    Some((binding.id().clone(), reason.clone()))
                }
                Availability::Available => None,
            })
            .collect()
    }

    /// Validate a scope: a filter over the natural key, stated against the
    /// federated vocabulary, that every source can compile.
    ///
    /// An empty string means the whole root.
    pub fn scope(&self, filters: &str) -> Result<EntityQuery, FederationError> {
        let entity = self
            .source_vocabulary
            .entities
            .first()
            .map(|entity| format!("{}.{}", self.source_vocabulary.name, entity))
            .unwrap_or_default();
        let vocab = EntityVocabulary::derive(std::slice::from_ref(&self.source_vocabulary));
        let query = validate_entity(EntityTarget::parse(&entity), &parse_query(filters)?, &vocab)?;
        for filter in &query.filters {
            if filter.attr != mapping::KEY {
                return Err(FederationError::ScopeNotPath {
                    attr: filter.attr.clone(),
                });
            }
        }
        Ok(query)
    }

    /// Read every available source over `scope` and assemble the documents,
    /// in path order.
    ///
    /// Fails closed. A source that refuses (a malformed frontmatter block, a
    /// file that cannot be stat'd) fails the whole assembly rather than
    /// shrinking it: a silently shorter answer is indistinguishable from a
    /// correct one.
    ///
    /// `context` carries the caller's single cancellation signal, deadline,
    /// and work and response-byte budgets across every source page.
    pub async fn assemble(
        &self,
        scope: &EntityQuery,
        context: &ReadContext,
    ) -> Result<Vec<Document>, FederationError> {
        let readings = self.read_all(scope, context).await?;
        self.documents(&readings)
    }

    /// Assemble the documents from an answer a federating
    /// [`Executor`](catdb_federation::Executor) has already produced.
    ///
    /// [`assemble`](Self::assemble) reads the sources itself, which is enough
    /// when nobody needs to know *how* each source ran. When somebody does —
    /// which mode executed, how much was scanned, where the pushdown line fell,
    /// whether a source was left out — the report and the attribution have to
    /// come from **one** execution. Two reads that agree today are still two
    /// reads, and a report describing the read the attribution did not use is
    /// exactly the kind of seam this project keeps losing guarantees at.
    ///
    /// So this attributes the rows it is given and reads nothing. It fails
    /// closed on every way an answer could fail to describe this federation:
    ///
    /// * a bound source the answer does not account for, or an accounted-for
    ///   source this federation does not bind
    ///   ([`FederationError::SourceSetMismatch`]);
    /// * a source the answer reports as having refused, carrying rows anyway
    ///   ([`FederationError::RefusedButAnswered`]).
    ///
    /// A source the answer reports as [`Outcome::Refused`] is recorded as
    /// [`ProviderVerdict::NotAsked`] with the executor's own reason — the same
    /// verdict [`assemble`](Self::assemble) records for a source that declared
    /// itself unavailable, and for the same reason: "it was never consulted"
    /// and "it had nothing to say" are different facts.
    ///
    /// What this cannot check is that the answer came from the scope the caller
    /// means: a [`Federated`] carries rows and reports, not the query that
    /// produced them. Pass the answer to the scope you validated.
    pub fn assemble_from(&self, answer: &Federated) -> Result<Vec<Document>, FederationError> {
        let readings = self.attribute_all(answer)?;
        self.documents(&readings)
    }

    /// The documents every reading between them saw, in path order.
    fn documents(&self, readings: &[Reading<'_>]) -> Result<Vec<Document>, FederationError> {
        let mut keys: BTreeSet<&str> = BTreeSet::new();
        for reading in readings {
            keys.extend(reading.rows.keys().map(String::as_str));
        }

        keys.into_iter()
            .map(|key| self.assemble_one(key, readings))
            .collect()
    }

    /// Read each source once, keeping its rows by key.
    async fn read_all(
        &self,
        scope: &EntityQuery,
        context: &ReadContext,
    ) -> Result<Vec<Reading<'_>>, FederationError> {
        let mut readings = Vec::new();
        for binding in &self.bindings {
            let mut reading = Reading {
                binding,
                asked: true,
                reason: String::new(),
                rows: BTreeMap::new(),
            };
            match binding.availability() {
                Availability::Unavailable { reason } => {
                    // Never executed: an unavailable source's refusal must not
                    // be caught and turned into "it returned nothing".
                    reading.asked = false;
                    reading.reason = reason.clone();
                }
                Availability::Available => {
                    let authorized_fields = binding.source.capabilities().observed();
                    for row in read(binding.source, scope, &authorized_fields, context)
                        .await?
                        .rows
                    {
                        reading.take(row)?;
                    }
                }
            }
            readings.push(reading);
        }
        Ok(readings)
    }

    /// Split an already-executed answer back into one reading per bound source.
    fn attribute_all(&self, answer: &Federated) -> Result<Vec<Reading<'_>>, FederationError> {
        let bound: BTreeSet<String> = self
            .bindings
            .iter()
            .map(|binding| binding.id().to_string())
            .collect();
        let accounted: BTreeSet<String> = answer
            .reports
            .iter()
            .map(|report| report.source.to_string())
            .collect();
        if bound != accounted {
            return Err(FederationError::SourceSetMismatch {
                federated: bound.into_iter().collect(),
                answered: accounted.into_iter().collect(),
            });
        }

        let mut readings = Vec::new();
        for binding in &self.bindings {
            let mut reading = Reading {
                binding,
                asked: true,
                reason: String::new(),
                rows: BTreeMap::new(),
            };
            let mine = answer.rows.iter().filter(|row| &row.source == binding.id());
            // The set check above guarantees this, but an `expect` here would
            // be a panic on a caller's data; the typed refusal is the same
            // fact and cannot take the process down.
            let Some(report) = answer.report(binding.id()) else {
                return Err(FederationError::SourceSetMismatch {
                    federated: bound.into_iter().collect(),
                    answered: accounted.into_iter().collect(),
                });
            };
            match &report.outcome {
                Outcome::Refused { reason } => {
                    let rows = mine.count();
                    if rows > 0 {
                        return Err(FederationError::RefusedButAnswered {
                            source_id: binding.id().clone(),
                            reason: reason.to_string(),
                            rows,
                        });
                    }
                    reading.asked = false;
                    reading.reason = reason.to_string();
                }
                Outcome::Ran(_) => {
                    for row in mine {
                        reading.take(row.clone())?;
                    }
                }
            }
            readings.push(reading);
        }
        Ok(readings)
    }

    /// Resolve one document's fields against the declared precedence.
    fn assemble_one(
        &self,
        key: &str,
        readings: &[Reading<'_>],
    ) -> Result<Document, FederationError> {
        let canonical = object_id(&format!("catdb:document:{key}"));
        let mut observations = Vec::new();
        let mut claims = Vec::new();
        for reading in readings {
            let Some(row) = reading.rows.get(key) else {
                continue;
            };
            let observed = object_id(&format!("catdb:observation:{}:{key}", row.source));
            claims.push(
                self.identity
                    .propose(observed, canonical)
                    .map_err(|refusal| FederationError::IdentityRefused {
                        key: key.to_string(),
                        refusal,
                    })?,
            );
            observations.push(row.clone());
        }

        let mut fields = BTreeMap::new();
        for entry in self.precedence.fields() {
            let mut verdicts = Vec::new();
            for provider in &entry.order {
                verdicts.push((
                    provider.clone(),
                    self.verdict(key, provider, readings, &entry.field)?,
                ));
            }
            fields.insert(
                entry.field.clone(),
                FieldAttribution {
                    field: entry.field.clone(),
                    verdicts,
                },
            );
        }

        Ok(Document::new(key.to_string(), fields, observations, claims))
    }

    /// What one provider had to say about one field of one document.
    fn verdict(
        &self,
        key: &str,
        provider: &Provider,
        readings: &[Reading<'_>],
        field: &AttrName,
    ) -> Result<ProviderVerdict, FederationError> {
        let Some(reading) = readings
            .iter()
            .find(|reading| reading.binding.id() == &provider.source)
        else {
            return Ok(ProviderVerdict::NotAsked {
                reason: "not part of this federation".to_string(),
            });
        };
        if !reading.asked {
            return Ok(ProviderVerdict::NotAsked {
                reason: reading.reason.clone(),
            });
        }
        let Some(row) = reading.rows.get(key) else {
            return Ok(ProviderVerdict::DidNotSeeDocument);
        };
        // Absent is absent: an attribute the source did not observe for this
        // row is not a key of its value at all.
        let Some(raw) = row.value.get(provider.attr.as_str()) else {
            return Ok(ProviderVerdict::ObservedNothing);
        };
        if raw.is_null() {
            return Ok(ProviderVerdict::ObservedNothing);
        }
        // The ONLY conversion mechanism: whatever chain the validated mapping
        // declares for this attribute, applied in declared order. It is empty
        // for all three adapters today (see `mapping`), and this call is what
        // makes that a fact about the declaration rather than about the code.
        let chain: Vec<_> = reading
            .binding
            .mapping
            .mapping()
            .transformation_chain(&provider.attr)
            .into_iter()
            .cloned()
            .collect();
        let value = apply_chain(&chain, raw).map_err(|error| FederationError::Transformation {
            source_id: provider.source.clone(),
            key: key.to_string(),
            attr: field.to_string(),
            detail: error.to_string(),
        })?;
        Ok(ProviderVerdict::Observed(value))
    }
}

impl std::fmt::Debug for Federation<'_> {
    /// Names the bound sources and their availability. A `&dyn DataSource`
    /// cannot be printed, and the useful thing about a federation is which
    /// sources are in it anyway.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Federation")
            .field("document", &self.document.name)
            .field(
                "sources",
                &self
                    .bindings
                    .iter()
                    .map(|binding| (binding.id().to_string(), binding.availability().clone()))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod product_scope_tests {
    use catdb_connectors::{DataSource, FilesystemSource, FrontmatterSource, GitSource};
    use catdb_validation::EggProver;

    use super::Federation;

    #[test]
    fn exact_product_path_is_typed_without_round_tripping_through_query_text() {
        let root = tempfile::tempdir().expect("temporary source root");
        std::fs::write(root.path().join("a file.md"), "# A file\n").expect("write markdown");
        let filesystem = FilesystemSource::new(root.path());
        let git = GitSource::open(root.path());
        let frontmatter = FrontmatterSource::new(root.path());
        let sources: Vec<&dyn DataSource> = vec![&filesystem, &git, &frontmatter];
        let federation =
            Federation::new(sources, &EggProver::default()).expect("valid federation");

        let query = federation
            .scope_exact_path("a file.md")
            .expect("spaces are data, not query syntax");

        assert_eq!(query.filters.len(), 1);
        assert_eq!(query.filters[0].attr, "path");
        assert_eq!(query.filters[0].op, catdb_query::AttrOp::Eq);
        assert_eq!(
            query.filters[0].value,
            catdb_query::AttrValue::Text("a file.md".to_string())
        );
    }
}

/// One source's rows for one scope, keyed by the natural key.
struct Reading<'a> {
    binding: &'a Binding<'a>,
    /// False when the source declared itself unavailable and was never read.
    asked: bool,
    /// Why it was not asked; empty when it was.
    reason: String,
    rows: BTreeMap<String, SourceRow>,
}

impl Reading<'_> {
    /// File one row under the natural key.
    ///
    /// Shared by [`Federation::read_all`] and [`Federation::attribute_all`] so
    /// that a row this federation cannot identify is refused the same way
    /// whoever performed the read: a keyless row cannot be linked to any other
    /// source's view of the same document, and two rows for one key leave no
    /// single observation to attribute.
    fn take(&mut self, row: SourceRow) -> Result<(), FederationError> {
        let key = row
            .value
            .get(mapping::KEY)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| FederationError::KeylessRow {
                source_id: self.binding.id().clone(),
                key: row.key.clone(),
            })?
            .to_string();
        if self.rows.insert(key.clone(), row).is_some() {
            return Err(FederationError::DuplicateKey {
                source_id: self.binding.id().clone(),
                key,
            });
        }
        Ok(())
    }
}

/// A deterministic id for a record, so two runs over an unchanged corpus
/// produce the same claims.
///
/// Derived from the record's name rather than randomly, so an
/// [`IdentityClaim`] is reproducible: the claim about `docs/a.md` seen by the
/// filesystem is the same claim on every run, which is what makes it something
/// a human can be asked to rule on twice.
fn object_id(name: &str) -> ObjectId {
    ObjectId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()))
}
