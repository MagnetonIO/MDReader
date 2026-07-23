//! The Markdown domain package: CatDB schemas for the FS -> MD -> KN -> RX pipeline.
//!
//! This crate re-exports the kernel's builtin pipeline as MDReader's domain
//! model: the four pipeline schemas, the three (contravariant) pipeline
//! mappings defined on their provenance-core sub-schemas, and the overlay
//! demo schemas. See [`catdb_core::builtin`] for the direction and
//! sub-schema design rationale.
//!
//! [`validate_all`] runs the whole domain — every schema, every equation,
//! every mapping — through a prover and returns the aggregated
//! [`ValidationReport`]; tests (and later, app startup) assert it is clean.

#![deny(missing_docs)]

pub use catdb_core::builtin;
use catdb_core::{Mapping, PathProver, Schema};
use catdb_validation::ValidationReport;

/// Returns this crate's package name.
pub fn crate_name() -> &'static str {
    "mdreader-domain"
}

/// Returns the categorical kernel crate this domain package is defined against.
pub fn kernel_crate() -> &'static str {
    catdb_core::crate_name()
}

/// The four pipeline schemas, in data-flow order: `fs`, `md`, `kn`, `rx`.
pub fn pipeline_schemas() -> [Schema; 4] {
    [
        builtin::fs_schema(),
        builtin::md_schema(),
        builtin::kn_schema(),
        builtin::rx_schema(),
    ]
}

/// The provenance-core sub-schemas the pipeline mappings are defined on, in
/// pipeline order: `md_core`, `kn_core`, `rx_core`.
pub fn pipeline_core_schemas() -> [Schema; 3] {
    [
        builtin::md_core_schema(),
        builtin::kn_core_schema(),
        builtin::rx_core_schema(),
    ]
}

/// The three pipeline mappings, in pipeline order: `F_parse : md_core → fs`,
/// `F_extract : kn_core → md`, `F_index : rx_core → kn`.
pub fn pipeline_mappings() -> Vec<Mapping> {
    vec![
        builtin::parse_mapping(),
        builtin::extract_mapping(),
        builtin::index_mapping(),
    ]
}

/// The overlay demo schemas: `document_overlay` and `decision_overlay`.
pub fn overlay_schemas() -> [Schema; 2] {
    [
        builtin::document_overlay_schema(),
        builtin::decision_overlay_schema(),
    ]
}

/// Validates the entire domain with the given prover: all pipeline schemas,
/// their provenance cores, the overlays, every schema equation, and all
/// three pipeline mappings.
pub fn validate_all(prover: &dyn PathProver) -> ValidationReport {
    let mut report = ValidationReport::default();
    for schema in pipeline_schemas()
        .into_iter()
        .chain(pipeline_core_schemas())
        .chain(overlay_schemas())
    {
        report.record_schema(&schema);
        report.record_equations(&schema, prover);
    }
    report.record_mapping(
        "F_parse",
        &builtin::parse_mapping(),
        &builtin::md_core_schema(),
        &builtin::fs_schema(),
        prover,
    );
    report.record_mapping(
        "F_extract",
        &builtin::extract_mapping(),
        &builtin::kn_core_schema(),
        &builtin::md_schema(),
        prover,
    );
    report.record_mapping(
        "F_index",
        &builtin::index_mapping(),
        &builtin::rx_core_schema(),
        &builtin::kn_schema(),
        prover,
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use catdb_core::SchemaName;
    use catdb_validation::EggProver;

    #[test]
    fn builds_on_catdb_core() {
        assert_eq!(crate_name(), env!("CARGO_PKG_NAME"));
        assert_eq!(kernel_crate(), "catdb-core");
    }

    #[test]
    fn pipeline_schemas_are_in_data_flow_order() {
        let names: Vec<SchemaName> = pipeline_schemas()
            .iter()
            .map(|schema| schema.name.clone())
            .collect();
        assert_eq!(
            names,
            vec!["fs".into(), "md".into(), "kn".into(), "rx".into()]
        );
    }

    #[test]
    fn pipeline_mappings_cover_the_three_stages() {
        let endpoints: Vec<(SchemaName, SchemaName)> = pipeline_mappings()
            .iter()
            .map(|mapping| (mapping.src.clone(), mapping.dst.clone()))
            .collect();
        assert_eq!(
            endpoints,
            vec![
                ("md_core".into(), "fs".into()),
                ("kn_core".into(), "md".into()),
                ("rx_core".into(), "kn".into()),
            ]
        );
    }

    #[test]
    fn the_whole_domain_validates_cleanly_with_the_real_prover() {
        let report = validate_all(&EggProver::default());
        assert!(
            report.is_clean(),
            "domain must validate cleanly:\n{}",
            report.error_messages().join("\n")
        );
        // Nine schemas, three mappings, and every builtin equation verdict
        // recorded (fs has 1 equation, md and md_core have 2 each).
        assert_eq!(report.schemas.len(), 9);
        assert_eq!(report.mappings.len(), 3);
        assert_eq!(report.equations.len(), 5);
        assert!(report
            .mappings
            .iter()
            .all(|mapping| mapping.waivers.is_empty()));
    }
}
