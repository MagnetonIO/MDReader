//! The declaration side of the federated `Document`: the schema, one real
//! validated mapping per source, what each mapping carries and leaves
//! unreached, and the identity rule.
//!
//! Every assertion names things — attributes, sources, fields — rather than
//! counting them, because a count that is still right after a rename is not a
//! test of anything.

mod federation_support;

use catdb_connectors::{
    Availability, DataSource, FilesystemSource, FrontmatterSource, GitSource, SourceId,
};
use catdb_core::{
    AttrName, AttrTarget, BaseType, Concept, IdentityConfidence, IdentityRule, InformationLoss,
    MappingError, ObjectId, Path, ValueTransformation,
};
use catdb_validation::EggProver;
use federation_support::corpus;
use mdreader_domain::federated::{
    self, document_schema, path_identity_rule, source_mapping, validated_source_mapping,
    Federation, FederationError,
};

fn prover() -> EggProver {
    EggProver::default()
}

/// The three adapters over the committed corpus.
fn adapters() -> (FilesystemSource, GitSource, FrontmatterSource) {
    let root = corpus();
    (
        FilesystemSource::new(&root),
        GitSource::open(&root),
        FrontmatterSource::new(&root),
    )
}

fn names(concepts: &[Concept]) -> Vec<&str> {
    concepts.iter().map(Concept::name).collect()
}

// --- the schema ------------------------------------------------------------

/// The concept is kernel-validated, exactly like the builtin overlays.
#[test]
fn the_document_schema_validates_and_declares_the_concept_by_name() {
    let schema = document_schema();
    assert_eq!(schema.validate(), Ok(()));
    assert_eq!(schema.name.as_str(), "document");

    let mut attrs: Vec<(&str, &str, bool)> = schema
        .attributes
        .values()
        .map(|attr| (attr.name.as_str(), attr.ty.as_str(), attr.nullable))
        .collect();
    attrs.sort();
    assert_eq!(
        attrs,
        [
            ("author", "Text", true),
            ("body", "Text", true),
            ("modifiedAt", "Timestamp", true),
            // The natural key is the one thing a Document always has.
            ("path", "Text", false),
            ("size", "Int", true),
            ("title", "Text", true),
        ]
    );
}

/// The concept is one entity. If it were several, "one concept from many
/// sources" would be trivially true and would mean nothing.
#[test]
fn the_document_concept_is_exactly_one_entity() {
    let schema = document_schema();
    let entities: Vec<&str> = schema
        .entities
        .iter()
        .map(|entity| entity.as_str())
        .collect();
    assert_eq!(entities, [federated::ENTITY]);
    assert_eq!(federated::qualified_entity(), "document.Document");
}

/// The whole domain — the four pipeline stages, their cores, the overlays and
/// the federated `Document` — validates with the real prover.
#[test]
fn the_federated_document_validates_with_the_rest_of_the_domain() {
    let report = mdreader_domain::validate_all(&prover());
    assert!(
        report.is_clean(),
        "domain must validate cleanly:\n{}",
        report.error_messages().join("\n")
    );
    assert!(report
        .schemas
        .iter()
        .any(|recorded| recorded.schema.as_str() == federated::SCHEMA));
}

// --- one real mapping per source -------------------------------------------

/// Every source's mapping is a real, kernel-validated functor into
/// `document` — not a lookup table, and not waived.
#[test]
fn every_source_mapping_validates_against_the_real_prover() {
    let (fs, git, front) = adapters();
    for source in [&fs as &dyn DataSource, &git, &front] {
        let valid = validated_source_mapping(source, &prover())
            .unwrap_or_else(|error| panic!("`{}` mapping must validate: {error}", source.id()));
        assert!(
            valid.waivers().is_empty(),
            "`{}` needed a waiver to validate",
            source.id()
        );
        assert_eq!(valid.mapping().dst.as_str(), federated::SCHEMA);
        assert_eq!(
            valid.mapping().write_capability,
            catdb_core::MappingWriteCapability::ReadOnly,
            "`{}` must declare itself read-only",
            source.id()
        );
    }
}

/// Each mapping carries exactly what its adapter declared it observes —
/// derived from the adapter's schema, never restated here.
#[test]
fn each_mapping_carries_exactly_the_attributes_its_source_declares() {
    let (fs, git, front) = adapters();
    assert!(
        matches!(git.capabilities().availability(), Availability::Available),
        "the corpus lives inside this repository, so the git source must be available"
    );
    let carried = |source: &dyn DataSource| -> Vec<String> {
        let mapping = source_mapping(source).expect("mapping builds");
        let mut attrs: Vec<String> = mapping
            .attr_map
            .keys()
            .map(std::string::ToString::to_string)
            .collect();
        attrs.sort();
        // Every attribute lands in the same slot on the one entity, so the
        // destination side is worth asserting once, here.
        for (attr, target) in &mapping.attr_map {
            assert_eq!(target.path, Path::identity(federated::ENTITY));
            assert_eq!(&target.attr, attr);
        }
        attrs
    };
    assert_eq!(carried(&fs), ["modifiedAt", "path", "size"]);
    assert_eq!(carried(&git), ["author", "modifiedAt", "path"]);
    assert_eq!(carried(&front), ["author", "body", "path", "title"]);
}

/// The mappings declare no value transformations, and that is a consequence of
/// reuse rather than an omission: the `Parse` that types a raw mtime is
/// declared on the adapter's `SourceAttr`, so the mapping's source type is
/// ALREADY `Timestamp`, and restating the parse is a typed error.
#[test]
fn duplicating_the_adapters_declared_parse_in_a_mapping_is_refused() {
    let (fs, _, _) = adapters();
    let mut mapping = source_mapping(&fs).expect("mapping builds");
    assert!(
        mapping.transformations.is_empty(),
        "the source's declared types already match the concept's"
    );

    mapping.transformations.push(ValueTransformation::Parse {
        attr: "modifiedAt".into(),
        to: BaseType::Timestamp,
    });
    let errors = mapping
        .validate(fs.schema(), &document_schema(), &prover())
        .expect_err("a second, contradictory parse must not validate");
    assert!(
        errors.iter().any(|error| matches!(
            error,
            MappingError::TransformationChainBroken {
                attr,
                expected: BaseType::Timestamp,
                found: BaseType::Text,
                ..
            } if attr.as_str() == "modifiedAt"
        )),
        "expected a broken chain on `modifiedAt`, got {errors:?}"
    );
}

/// A source that declares something the concept cannot hold is refused by
/// name, rather than producing a quietly narrower mapping.
#[test]
fn an_attribute_the_concept_cannot_hold_is_a_typed_refusal() {
    let tree = federation_support::tree(&[("note.md", "# Note\n")]);
    let front = FrontmatterSource::new(tree.path());
    let mut mapping = source_mapping(&front).expect("mapping builds");
    // Simulate an adapter that grew an attribute the concept has no slot for,
    // by asking the concept to receive one it does not declare.
    mapping.attr_map.insert(
        "wordCount".into(),
        AttrTarget {
            path: Path::identity(federated::ENTITY),
            attr: "wordCount".into(),
        },
    );
    let errors = mapping
        .validate(front.schema(), &document_schema(), &prover())
        .expect_err("an attribute the schemas do not declare must not validate");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, MappingError::UnknownSourceAttr { attr } if attr.as_str() == "wordCount")),
        "expected `wordCount` to be refused, got {errors:?}"
    );
}

/// A source that is not here declares itself unavailable. Its mapping is still
/// real and still valid — it simply carries nothing, and claims no identity it
/// could not compare.
#[test]
fn an_unavailable_source_yields_a_valid_mapping_that_carries_nothing() {
    let tree = federation_support::tree(&[("note.md", "# Note\n")]);
    let git = GitSource::open(tree.path());
    assert!(
        matches!(
            git.capabilities().availability(),
            Availability::Unavailable { .. }
        ),
        "a temp dir is not a git work tree"
    );
    let mapping = source_mapping(&git).expect("an unavailable source still maps");
    assert!(mapping.attr_map.is_empty());
    assert!(
        mapping.identities.is_empty(),
        "a source that cannot observe the key cannot claim identity on it"
    );
    validated_source_mapping(&git, &prover()).expect("and it still validates");
}

// --- coverage: what is carried, what is lost, by name -----------------------

/// The kernel's own coverage analysis, per source, named attribute by named
/// attribute. `unreached_destination` is reported and is explicitly NOT loss.
#[test]
fn coverage_reports_what_each_mapping_carries_and_leaves_unreached_by_name() {
    let (fs, git, front) = adapters();
    let federation =
        Federation::new(vec![&fs, &git, &front], &prover()).expect("the federation declares");

    let expected: [(&str, &[&str], &[&str]); 3] = [
        (
            FilesystemSource::ID,
            &["modifiedAt", "path", "size"],
            &["author", "body", "title"],
        ),
        (
            GitSource::ID,
            &["author", "modifiedAt", "path"],
            &["body", "size", "title"],
        ),
        (
            FrontmatterSource::ID,
            &["author", "body", "path", "title"],
            &["modifiedAt", "size"],
        ),
    ];

    for (id, carried, unreached) in expected {
        let (_, report) = federation
            .coverage()
            .into_iter()
            .find(|(source, _)| source.as_str() == id)
            .unwrap_or_else(|| panic!("no coverage for `{id}`"));

        let mut mapped: Vec<&str> = report
            .mapped
            .iter()
            .filter(|concept| matches!(concept.concept, Concept::Attribute(_)))
            .map(|concept| concept.concept.name())
            .collect();
        mapped.sort();
        assert_eq!(mapped, carried, "`{id}` carries");

        assert!(report.is_total(), "`{id}` must map its whole source schema");
        assert!(
            report.is_lossless(),
            "`{id}` must lose nothing: {:?}",
            report.loss
        );
        assert!(
            report.loss.is_empty(),
            "`{id}` reported loss: {:?}",
            report.loss
        );
        assert!(
            !report.collapses_values(),
            "`{id}` must not collapse distinct values"
        );

        let mut unreached_found: Vec<&str> = report
            .unreached_destination
            .iter()
            .filter(|concept| matches!(concept, Concept::Attribute(_)))
            .map(Concept::name)
            .collect();
        unreached_found.sort();
        assert_eq!(unreached_found, unreached, "`{id}` leaves unreached");
        // The load-bearing distinction: destination structure this source does
        // not fill is NOT information loss, and the report keeps them apart.
        assert!(
            report.is_lossless() && !report.unreached_destination.is_empty(),
            "`{id}` should be lossless while leaving part of the concept unreached"
        );
    }
}

/// Between them the three sources reach every attribute of the concept: the
/// federation is not quietly leaving a declared field with no supplier.
#[test]
fn the_three_mappings_together_reach_every_document_attribute() {
    let (fs, git, front) = adapters();
    let federation = Federation::new(vec![&fs, &git, &front], &prover()).expect("declares");
    let mut reached: Vec<String> = federation
        .coverage()
        .iter()
        .flat_map(|(_, report)| {
            report
                .mapped
                .iter()
                .filter(|concept| matches!(concept.concept, Concept::Attribute(_)))
                .map(|concept| concept.concept.name().to_string())
        })
        .collect();
    reached.sort();
    reached.dedup();
    let mut declared: Vec<String> = federated::ATTRIBUTES
        .iter()
        .map(|(name, _, _)| (*name).to_string())
        .collect();
    declared.sort();
    assert_eq!(reached, declared);
}

/// What loss actually looks like, so that "no loss" above means something: a
/// draft that drops an attribute is reported as `Dropped` **by name**, and the
/// kernel refuses to validate it.
#[test]
fn a_mapping_that_drops_an_attribute_reports_it_lost_by_name_and_fails_to_validate() {
    let (fs, _, _) = adapters();
    let mut draft = source_mapping(&fs).expect("mapping builds");
    draft.attr_map.remove(&AttrName::from("size"));

    let report = catdb_core::coverage(&draft, fs.schema(), &document_schema());
    assert!(!report.is_lossless(), "dropping an attribute is loss");
    assert_eq!(
        report.loss,
        vec![InformationLoss::Dropped {
            concept: Concept::Attribute("size".into())
        }]
    );
    assert_eq!(names(&report.unmapped), ["size"]);

    let errors = draft
        .validate(fs.schema(), &document_schema(), &prover())
        .expect_err("a partial mapping must not validate");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, MappingError::MissingAttrImage { attr } if attr.as_str() == "size")),
        "expected `size` to be reported missing, got {errors:?}"
    );
}

/// Two source attributes landing in one slot is the other kind of loss, and it
/// is reported by naming both of them.
#[test]
fn two_attributes_sharing_one_slot_are_reported_as_overwriting_by_name() {
    let (fs, _, _) = adapters();
    let mut draft = source_mapping(&fs).expect("mapping builds");
    draft.attr_map.insert(
        "size".into(),
        AttrTarget {
            path: Path::identity(federated::ENTITY),
            attr: "path".into(),
        },
    );
    let report = catdb_core::coverage(&draft, fs.schema(), &document_schema());
    assert!(
        matches!(
            report.loss.as_slice(),
            [InformationLoss::AttributesOverwritten { image, sources }]
                if image.attr.as_str() == "path"
                    && sources.iter().map(|name| name.as_str()).collect::<Vec<_>>() == ["path", "size"]
        ),
        "expected `path` and `size` reported as competing for one slot, got {:?}",
        report.loss
    );
}

// --- identity ---------------------------------------------------------------

/// The rule that makes three observations one document: an exact match on the
/// natural key, at the `Known` tier, with its basis written down.
#[test]
fn the_identity_rule_matches_on_the_natural_key_at_the_known_tier() {
    let rule = path_identity_rule();
    assert_eq!(rule.src_entity().as_str(), federated::ENTITY);
    assert_eq!(rule.canonical_entity().as_str(), federated::ENTITY);
    let keys: Vec<(&str, &str)> = rule
        .keys()
        .iter()
        .map(|key| (key.src.as_str(), key.canonical.as_str()))
        .collect();
    assert_eq!(keys, [("path", "path")]);
    assert_eq!(rule.confidence(), IdentityConfidence::Known);
    assert!(
        rule.basis().to_lowercase().contains("same root"),
        "the basis must state the precondition path equality rests on: {}",
        rule.basis()
    );
    assert!(rule.confidence().is_linkable());
}

/// A claim relates two ids. It does not merge them, and it cannot: both come
/// out of the proposal untouched and distinct.
#[test]
fn proposing_an_identity_links_two_records_without_touching_either() {
    let source = ObjectId::from_uuid(uuid::Uuid::from_u128(1));
    let canonical = ObjectId::from_uuid(uuid::Uuid::from_u128(2));
    let claim = path_identity_rule()
        .propose(source, canonical)
        .expect("a `Known` rule links");
    assert_eq!(claim.source(), source);
    assert_eq!(claim.canonical(), canonical);
    assert_ne!(claim.source(), claim.canonical());
    assert_eq!(claim.confidence(), IdentityConfidence::Known);
}

/// The rule rides on every mapping whose source can observe the key, so
/// `Mapping::validate` type-checks it against both schemas rather than this
/// crate asserting it is well formed.
#[test]
fn the_identity_rule_is_carried_and_type_checked_by_every_available_mapping() {
    let (fs, git, front) = adapters();
    for source in [&fs as &dyn DataSource, &git, &front] {
        let mapping = source_mapping(source).expect("mapping builds");
        assert_eq!(
            mapping.identities,
            vec![path_identity_rule()],
            "`{}` must carry the identity rule",
            source.id()
        );
    }
}

/// The rule is refused where it would be wrong: a `Document` schema without
/// `path` cannot be identified on `path`, and the kernel says so rather than
/// this layer trusting itself.
#[test]
fn an_identity_rule_on_an_undeclared_key_is_refused_by_the_kernel() {
    let (fs, _, _) = adapters();
    let mut mapping = source_mapping(&fs).expect("mapping builds");
    mapping.identities = vec![match path_identity_rule() {
        IdentityRule::SharedKey {
            src_entity,
            canonical_entity,
            confidence,
            basis,
            ..
        } => IdentityRule::SharedKey {
            src_entity,
            canonical_entity,
            keys: vec![catdb_core::IdentityKey {
                src: "inode".into(),
                canonical: "inode".into(),
            }],
            confidence,
            basis,
        },
    }];
    let errors = mapping
        .validate(fs.schema(), &document_schema(), &prover())
        .expect_err("an identity on an attribute neither schema declares must not validate");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, MappingError::IdentityKeyUnknown { attr, .. } if attr.as_str() == "inode")),
        "expected `inode` to be refused as a key, got {errors:?}"
    );
}

// --- the declaration hangs together -----------------------------------------

/// The precedence declaration and the validated mappings describe the same
/// federation, in both directions. Neither is allowed to drift.
#[test]
fn declaring_a_federation_pins_precedence_and_mappings_against_each_other() {
    let (fs, git, front) = adapters();
    let federation = Federation::new(vec![&fs, &git, &front], &prover()).expect("declares");

    for entry in federation.precedence().fields() {
        for provider in &entry.order {
            let binding = federation
                .binding(&provider.source)
                .unwrap_or_else(|| panic!("`{}` is not bound", provider.source));
            let target = binding
                .mapping()
                .mapping()
                .attr_map
                .get(&provider.attr)
                .unwrap_or_else(|| panic!("`{provider}` is not mapped"));
            assert_eq!(
                target.attr, entry.field,
                "`{provider}` must land on `{}`",
                entry.field
            );
        }
    }
}

/// The declared order, by name — the thing a reader consults to know why a
/// value won.
#[test]
fn the_declared_precedence_names_its_sources_in_order() {
    let order = |field: &str| -> Vec<String> {
        federated::precedence()
            .of(&field.into())
            .unwrap_or_else(|| panic!("no precedence for `{field}`"))
            .order
            .iter()
            .map(|provider| provider.source.to_string())
            .collect()
    };
    assert_eq!(order("path"), ["filesystem", "git", "frontmatter"]);
    assert_eq!(order("title"), ["frontmatter"]);
    assert_eq!(order("author"), ["frontmatter", "git"]);
    assert_eq!(order("modifiedAt"), ["filesystem", "git"]);
    assert_eq!(order("size"), ["filesystem"]);
    assert_eq!(order("body"), ["frontmatter"]);
}

/// `title`'s two candidates are one adapter's declared chain, recorded here
/// rather than re-derived — so the record has to say what that chain is.
#[test]
fn the_title_provider_records_the_adapters_own_fallback_chain() {
    let precedence = federated::precedence();
    let title = precedence.of(&"title".into()).expect("title is declared");
    let provider = &title.order[0];
    assert_eq!(provider.source.as_str(), FrontmatterSource::ID);
    assert_eq!(
        provider.declared_fallback,
        ["YAML `title:`", "the first H1, skipping code fences"]
    );
    // Everything else is observed exactly one way, and says so by saying
    // nothing.
    for entry in precedence.fields() {
        for other in &entry.order {
            if entry.field.as_str() != "title" {
                assert!(
                    other.declared_fallback.is_empty(),
                    "`{other}` claims a fallback chain it does not have"
                );
            }
        }
    }
}

/// A federation of nothing is a refusal, not an empty answer.
#[test]
fn a_federation_needs_at_least_one_source() {
    let error = Federation::new(Vec::new(), &prover()).expect_err("no sources is a refusal");
    assert!(matches!(error, FederationError::NoSources));
}

/// Two sources under one name would make provenance ambiguous, which is the
/// one thing this concept cannot tolerate.
#[test]
fn two_sources_with_one_name_are_refused() {
    let (fs, _, _) = adapters();
    let other = FilesystemSource::new(corpus());
    let error = Federation::new(vec![&fs, &other], &prover()).expect_err("duplicate is a refusal");
    assert!(
        matches!(&error, FederationError::DuplicateSource(id) if id == &SourceId::new(FilesystemSource::ID)),
        "got {error}"
    );
}
