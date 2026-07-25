//! Seed a workspace whose overlay claims have already been REVIEWED, so the
//! GUI harness can drive the semantic surfaces without first performing every
//! approval through the Inspector.
//!
//! ```text
//! seed_reviewed_workspace <corpus-root> <app-data-dir> <reviewer> [schema ...]
//! ```
//!
//! # Why this exists, and what it is NOT
//!
//! `apps/mdreader/e2e-native` already seeds the app's *session* so the corpus
//! is granted on launch without driving NSOpenPanel (see `seed.ts`). The
//! Constraint Explorer needs one thing more: a law can only break over facts
//! that are actually **in the graph**, and a fact only reaches the graph when a
//! human approves it. Reproducing ~30 approval clicks through the Inspector
//! before every GUI run would make the interesting steps hostage to the
//! longest, most brittle one.
//!
//! So this seeds the review state the same way the app does — and *only* that
//! way. It opens the very database the app will open, through the very same
//! [`registration_with_proposals`] the app registers, runs a real import so
//! extraction proposes, and then calls the real
//! [`approve_and_promote`] with a real, non-blank reviewer name for each
//! proposal. The engine's approval check is the same gate it always is; there
//! is no bypass here, no direct write, and no auto-approval inside the product.
//! Point it at a schema and it approves that schema's proposals; point it at
//! nothing and it approves nothing.
//!
//! It is an `examples/` target on purpose: it is a test fixture generator, not
//! a product surface, so it is never part of a shipped binary.

use std::path::PathBuf;
use std::sync::Arc;

use catdb_embed::HashEmbedder;
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions};
use mdreader_ingest::proposals::{approve_and_promote, ProposalFilter, ProposalStore, StoredFact};
use mdreader_ingest::registration_with_proposals;

/// Where the desktop app keeps the private index for `canonical_root`:
/// `<app-data-dir>/workspaces/<blake3(canonical root)>/catdb.db`.
///
/// This mirrors `mdreader_app::engine::db_path_under`, which is `pub(crate)`
/// in the Tauri crate and unreachable from here (the app depends on this crate,
/// not the reverse). Drift is not silent: if the two ever disagree, the app
/// opens a different, un-reviewed workspace and the GUI flow's Constraint
/// Explorer step fails immediately with "reports no violation" rather than
/// passing over an empty interpretation.
fn app_db_path(app_data: &std::path::Path, canonical_root: &std::path::Path) -> PathBuf {
    let hash = blake3::hash(canonical_root.to_string_lossy().as_bytes())
        .to_hex()
        .to_string();
    app_data.join("workspaces").join(hash).join("catdb.db")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let corpus = PathBuf::from(
        args.next()
            .ok_or("usage: <corpus-root> <app-data-dir> <reviewer> [schema…]")?,
    )
    .canonicalize()?;
    let app_data = PathBuf::from(args.next().ok_or("missing <app-data-dir>")?);
    let db = app_db_path(&app_data, &corpus);
    let reviewer = args.next().ok_or("missing <reviewer>")?;
    if reviewer.trim().is_empty() {
        return Err("the reviewer must be a real, non-blank name".into());
    }
    let schemas: Vec<String> = args.collect();

    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let proposals_path = db.with_file_name("proposals.db");
    let store = Arc::new(ProposalStore::open(&proposals_path)?);
    // The SAME embedder the harness launches the app with
    // (`MDREADER_DEV_HASH_EMBED=1`), so the index this seeds is the index the
    // app then serves rather than one it has to rebuild.
    let engine = EmbeddedCatDb::open(
        EngineConfig::new(&db, Arc::new(HashEmbedder::new()))
            .with_domain(registration_with_proposals(Arc::clone(&store)))
            .with_actor(&reviewer),
    )?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let report = runtime.block_on(engine.import(&corpus, ImportOptions::default()))?;
    println!("workspace {}", db.display());
    println!(
        "indexed {} file(s), {} chunk(s)",
        report.files_scanned, report.chunks_embedded
    );

    let mut approved = 0usize;
    for schema in &schemas {
        let proposals = store.list(&ProposalFilter {
            schema: Some(schema.clone()),
            ..ProposalFilter::default()
        })?;
        if proposals.is_empty() {
            return Err(format!(
                "no `{schema}` proposals were extracted from {}: nothing to review, so the \
                 semantic surfaces would have nothing to show",
                corpus.display()
            )
            .into());
        }
        // Objects before morphisms: an edge is withheld until both endpoints
        // are in the graph, so approving objects first lets every edge land on
        // its own approval rather than on a later cascade.
        let (mut objects, mut morphisms) = (Vec::new(), Vec::new());
        for proposal in &proposals {
            match &proposal.fact {
                StoredFact::Object(_) => objects.push(proposal.fact_id),
                StoredFact::Morphism(_) => morphisms.push(proposal.fact_id),
            }
        }
        for fact_id in objects.iter().chain(morphisms.iter()) {
            runtime.block_on(approve_and_promote(&store, &engine, *fact_id, &reviewer))?;
            approved += 1;
        }
        println!(
            "reviewed {} `{schema}` claim(s) as {reviewer}",
            proposals.len()
        );
    }

    // Fail closed: if the approvals did not reach the graph, say so here rather
    // than let the GUI run discover an empty interpretation five steps later.
    for schema in &schemas {
        let committed = engine
            .store()
            .objects_with_entity_prefix(&format!("{schema}."))?;
        if committed.is_empty() {
            return Err(format!("`{schema}` is still empty after approval").into());
        }
        println!("`{schema}` holds {} committed object(s)", committed.len());
    }
    println!("approved {approved} claim(s) in total");

    // Report what the Constraint Explorer will have to draw, so a harness step
    // that expects a violation can be told here — before the app even launches
    // — that the corpus stopped producing one.
    let equations = runtime.block_on(engine.check_equations())?;
    println!(
        "checked {} schema(s): {} violation(s)",
        equations.schemas_checked,
        equations.violations.len()
    );
    for violation in &equations.violations {
        let located = |what: &str, object: Option<&catdb_engine::ViolatingObject>| match object {
            Some(object) => format!(
                "{what}={} @ {}",
                object.entity,
                object
                    .source
                    .as_ref()
                    .map_or_else(|| "no source".to_string(), |span| span.to_string())
            ),
            None => format!("{what}=nowhere"),
        };
        println!(
            "  {} {} | {} | {} | {}",
            violation.schema,
            violation.law,
            located("subject", Some(&violation.subject)),
            located("lhs", violation.lhs.as_ref()),
            located("rhs", violation.rhs.as_ref()),
        );
    }
    Ok(())
}
