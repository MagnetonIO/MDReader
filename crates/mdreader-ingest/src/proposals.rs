//! The durable **proposal store**, and the two transitions that move a fact
//! between it and the authoritative overlay graph.
//!
//! # Why proposals need a home that is not the graph
//!
//! Extraction during ingest produces *proposals*, and a proposal is by
//! definition unapproved. The engine declares the overlay schemas
//! approval-required, so an unapproved overlay fact is refused at **every**
//! write boundary — correctly. Yet the Inspector must be able to review a
//! corpus that was indexed yesterday, which means proposals have to survive
//! the process that produced them.
//!
//! So they live here: a SQLite database beside the workspace index, holding
//! *reviewable claims*. Nothing in this file constructs a
//! [`CatOperation::AssertObject`] or [`CatOperation::AssertMorphism`]; the
//! only assertions that ever leave this module come out of
//! [`Extraction::approved_operations`], which emits a fact solely once a human
//! has accepted it. The engine's check is untouched, and no fact is
//! auto-approved to smuggle it past.
//!
//! ```text
//!    ingest ──extract──► proposal store  ──approve──► overlay graph
//!                        (reviewable)     (the ONLY   (authoritative)
//!                                          door: approved_operations)
//!                              ▲
//!                              └── reject: stays here, never asserted
//! ```
//!
//! # Order of writes on approval
//!
//! [`approve_and_promote`] applies to the **engine first** and records the
//! approval **second**. The two stores cannot be updated atomically, so the
//! question is only which way a crash between them should fail. Store-first
//! would leave a fact marked approved that is not in the graph: invisible,
//! and nothing would ever repair it. Engine-first leaves a fact in the graph
//! that the store still shows as proposed — the reviewer sees it again, and
//! re-approving is a byte-identical upsert (extraction ids are deterministic).
//! Re-asking a question is a recoverable failure; silently losing a fact is
//! not.
//!
//! # Reconciliation across re-imports
//!
//! [`ProposalStore::record`] keys on the fact's own id, which extraction
//! derives deterministically from `(schema, entity, path, line)`. Re-importing
//! an unchanged corpus therefore finds every row and changes no review state —
//! in particular a **rejection is never re-proposed**. When the cited text
//! itself changed (a different `content_hash` under the same id) the review
//! resets to `Proposed`: the human accepted *that* text, and this is no longer
//! it. A row whose fact stopped extracting is kept, deliberately, as the
//! tombstone that stops a deleted-then-restored heading from re-proposing
//! something already rejected.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use catdb_engine::EmbeddedCatDb;
use catdb_ir::CatOperation;
use mdreader_domain::extract::{
    ApprovalError, ApprovalState, ExtractedMorphism, ExtractedObject, Extraction, Review,
};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

/// Why a proposal-store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum ProposalError {
    /// The underlying database rejected the read or write.
    #[error("proposal store: {0}")]
    Storage(#[from] rusqlite::Error),
    /// A stored payload could not be read back as the fact it claims to be.
    #[error("proposal store: corrupt payload for {id}: {source}")]
    Payload {
        /// The fact whose payload failed to parse.
        id: Uuid,
        /// The serde failure.
        source: serde_json::Error,
    },
    /// No proposal carries that id.
    #[error("no proposal with id {0}")]
    Unknown(Uuid),
    /// The requested approval transition is not legal from the current state.
    #[error(transparent)]
    Approval(#[from] ApprovalError),
    /// Promoting the fact into the overlay graph failed.
    #[error("promote: {0}")]
    Engine(#[from] catdb_engine::EngineError),
}

/// Result alias for proposal-store operations.
pub type ProposalResult<T> = Result<T, ProposalError>;

/// A stored fact: whichever of the two kinds of overlay claim it is.
#[derive(Debug, Clone, PartialEq)]
pub enum StoredFact {
    /// A proposed object.
    Object(ExtractedObject),
    /// A proposed morphism.
    Morphism(ExtractedMorphism),
}

/// One reviewable claim as the store holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredProposal {
    /// The fact's id — the object id or the morphism id.
    pub fact_id: Uuid,
    /// The overlay schema the fact is typed against.
    pub schema: String,
    /// The extractor (id and version) that proposed it.
    pub extractor: String,
    /// The fact itself, carrying its provenance, rule, and review state.
    pub fact: StoredFact,
}

impl StoredProposal {
    /// The fact's review state.
    pub fn state(&self) -> ApprovalState {
        self.review().state
    }

    /// The fact's review record.
    pub fn review(&self) -> &Review {
        match &self.fact {
            StoredFact::Object(object) => &object.review,
            StoredFact::Morphism(morphism) => &morphism.review,
        }
    }

    /// The rule id that proposed the fact (e.g. `D1`).
    pub fn rule(&self) -> &str {
        match &self.fact {
            StoredFact::Object(object) => &object.rule,
            StoredFact::Morphism(morphism) => &morphism.rule,
        }
    }

    /// The qualified entity (objects) or arrow (morphisms) name.
    pub fn qualified(&self) -> String {
        match &self.fact {
            StoredFact::Object(object) => object.object.entity.to_string(),
            StoredFact::Morphism(morphism) => morphism.morphism.arrow.to_string(),
        }
    }

    /// The fact's kernel provenance.
    pub fn provenance(&self) -> &catdb_core::Provenance {
        match &self.fact {
            StoredFact::Object(object) => &object.object.provenance,
            StoredFact::Morphism(morphism) => &morphism.morphism.provenance,
        }
    }
}

/// Which proposals to list.
///
/// Every field is a conjunctive narrowing; `Default` selects the whole store.
#[derive(Debug, Clone, Default)]
pub struct ProposalFilter {
    /// Restrict to one overlay schema.
    pub schema: Option<String>,
    /// Restrict to facts derived from one source file (repo-relative path).
    pub source_path: Option<String>,
    /// Restrict to facts whose cited span **overlaps** this inclusive,
    /// 1-based line range — a section selection catches every fact anywhere
    /// inside it, and a single-line selection catches the facts covering it.
    pub line_range: Option<(u32, u32)>,
    /// Restrict to one review state.
    pub state: Option<ApprovalState>,
}

/// What [`ProposalStore::record`] did to the store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordSummary {
    /// Facts the store had never seen, inserted as `Proposed`.
    pub inserted: u64,
    /// Facts already recorded whose cited text is unchanged; their review
    /// state was preserved exactly.
    pub unchanged: u64,
    /// Facts already recorded whose cited text **changed**, so the review was
    /// reset to `Proposed` (see the module docs).
    pub reproposed: u64,
}

/// What an approval wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalOutcome {
    /// The fact that was approved.
    pub fact_id: Uuid,
    /// Ids newly asserted into the overlay graph. More than one when
    /// approving an object also unlocked morphisms whose other endpoint was
    /// already approved (an edge is withheld until both its endpoints are).
    pub promoted: Vec<Uuid>,
}

/// What a rejection wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct RejectionOutcome {
    /// The fact that was rejected.
    pub fact_id: Uuid,
    /// Ids retracted from the overlay graph, because the rejection removed
    /// their support. Empty when the fact had never been approved — nothing
    /// was ever written, so nothing is undone.
    pub retracted: Vec<Uuid>,
    /// Morphisms rejected alongside an object rejection: an edge whose
    /// endpoint a human has rejected cannot stand.
    pub cascaded: Vec<Uuid>,
}

/// A durable store of reviewable overlay claims.
///
/// One SQLite database, one connection behind a mutex (the desktop app has a
/// single writer). Deliberately **separate from the workspace index**: the
/// index holds facts the corpus mechanically implies, this holds claims a
/// human has not ruled on yet, and conflating the two is exactly the mistake
/// the approval model exists to prevent.
#[derive(Debug)]
pub struct ProposalStore {
    connection: Mutex<Connection>,
}

/// The store's own schema revision, kept in SQLite's `user_version`.
const STORE_VERSION: i64 = 1;

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS proposal (
    fact_id      TEXT PRIMARY KEY NOT NULL,
    kind         TEXT NOT NULL,
    schema_name  TEXT NOT NULL,
    extractor    TEXT NOT NULL,
    rule         TEXT NOT NULL,
    qualified    TEXT NOT NULL,
    source_path  TEXT,
    start_line   INTEGER,
    end_line     INTEGER,
    content_hash TEXT,
    endpoint_src TEXT,
    endpoint_dst TEXT,
    state        TEXT NOT NULL,
    reviewer     TEXT,
    payload      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS proposal_by_span ON proposal (source_path, start_line, end_line);
CREATE INDEX IF NOT EXISTS proposal_by_schema ON proposal (schema_name, kind);
CREATE INDEX IF NOT EXISTS proposal_by_src ON proposal (endpoint_src);
CREATE INDEX IF NOT EXISTS proposal_by_dst ON proposal (endpoint_dst);
";

/// Column list shared by every read, in the order [`row_to_proposal`] expects.
const COLUMNS: &str = "fact_id, kind, schema_name, extractor, rule, state, reviewer, payload";

/// Objects before morphisms, then a stable tie-break — so a UI listing and a
/// golden test see the same order every time.
const ORDER: &str = "ORDER BY source_path, start_line, end_line, \
     CASE kind WHEN 'object' THEN 0 ELSE 1 END, qualified, fact_id";

fn state_str(state: ApprovalState) -> &'static str {
    match state {
        ApprovalState::Proposed => "proposed",
        ApprovalState::Approved => "approved",
        ApprovalState::Rejected => "rejected",
    }
}

fn parse_state(raw: &str) -> ApprovalState {
    match raw {
        "approved" => ApprovalState::Approved,
        "rejected" => ApprovalState::Rejected,
        // An unrecognized state is treated as unreviewed. Failing *open*
        // (approved) is the one direction that would be unsafe.
        _ => ApprovalState::Proposed,
    }
}

impl ProposalStore {
    /// Open (creating if needed) the store at `path`.
    pub fn open(path: impl AsRef<Path>) -> ProposalResult<Self> {
        if let Some(parent) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::from_connection(Connection::open(path)?)
    }

    /// An in-memory store — for tests, and for a workspace with nowhere to put
    /// a file.
    pub fn in_memory() -> ProposalResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> ProposalResult<Self> {
        connection.execute_batch(DDL)?;
        connection.pragma_update(None, "user_version", STORE_VERSION)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A poisoned mutex means a previous caller panicked mid-statement;
        // the connection itself is still usable and rusqlite reports any real
        // corruption, so recovering the guard beats making every later read
        // fail for a reason unrelated to the data.
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record everything `extraction` proposes, preserving review decisions
    /// already made (see the module docs on reconciliation).
    ///
    /// **This writes nothing to the overlay graph.** It cannot: no path out of
    /// this method produces a [`CatOperation`].
    pub fn record(&self, extraction: &Extraction) -> ProposalResult<RecordSummary> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let mut summary = RecordSummary::default();
        for fact in facts_of(extraction) {
            summary_add(&mut summary, upsert(&tx, extraction, &fact)?);
        }
        tx.commit()?;
        Ok(summary)
    }

    /// One proposal by id.
    pub fn get(&self, fact_id: Uuid) -> ProposalResult<Option<StoredProposal>> {
        let connection = self.lock();
        let sql = format!("SELECT {COLUMNS} FROM proposal WHERE fact_id = ?1");
        let found = connection
            .prepare(&sql)?
            .query_row(params![fact_id.to_string()], |row| Ok(row_to_proposal(row)))
            .optional()?;
        found.transpose()
    }

    /// Every proposal matching `filter`, in a stable order (see [`ORDER`]).
    pub fn list(&self, filter: &ProposalFilter) -> ProposalResult<Vec<StoredProposal>> {
        let mut clauses: Vec<String> = Vec::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(schema) = &filter.schema {
            binds.push(Box::new(schema.clone()));
            clauses.push(format!("schema_name = ?{}", binds.len()));
        }
        if let Some(path) = &filter.source_path {
            binds.push(Box::new(path.clone()));
            clauses.push(format!("source_path = ?{}", binds.len()));
        }
        if let Some(state) = filter.state {
            binds.push(Box::new(state_str(state)));
            clauses.push(format!("state = ?{}", binds.len()));
        }
        if let Some((start, end)) = filter.line_range {
            // Overlap, not containment: a fact cited to the whole file is part
            // of what a section selection is showing.
            binds.push(Box::new(end));
            let end_slot = binds.len();
            binds.push(Box::new(start));
            clauses.push(format!(
                "start_line <= ?{end_slot} AND end_line >= ?{}",
                binds.len()
            ));
        }
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let sql = format!("SELECT {COLUMNS} FROM proposal {where_clause} {ORDER}");

        let connection = self.lock();
        let mut statement = connection.prepare(&sql)?;
        let bound: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let rows = statement.query_map(bound.as_slice(), |row| Ok(row_to_proposal(row)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    /// How many proposals sit in each review state, for the whole store.
    pub fn counts(&self) -> ProposalResult<BTreeMap<String, u64>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT state, COUNT(*) FROM proposal GROUP BY state")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (state, count) = row?;
            out.insert(state, count);
        }
        Ok(out)
    }

    /// Every approved morphism in `schema` with `object_id` as an endpoint.
    ///
    /// Both transitions need exactly this set, from opposite directions:
    /// approving `object_id` may be the second endpoint one of these edges was
    /// waiting on, and rejecting `object_id` must take every one of them down.
    fn approved_edges_touching(
        &self,
        schema: &str,
        object_id: Uuid,
    ) -> ProposalResult<Vec<StoredProposal>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM proposal \
             WHERE kind = 'morphism' AND schema_name = ?1 AND state = 'approved' \
               AND (endpoint_src = ?2 OR endpoint_dst = ?2) {ORDER}"
        );
        let connection = self.lock();
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params![schema, object_id.to_string()], |row| {
            Ok(row_to_proposal(row))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    fn approved_object(&self, id: Uuid) -> ProposalResult<Option<ExtractedObject>> {
        Ok(self.get(id)?.and_then(|proposal| match proposal.fact {
            StoredFact::Object(object) if object.review.state == ApprovalState::Approved => {
                Some(object)
            }
            _ => None,
        }))
    }

    /// Write a review transition back to the store.
    fn persist_review(&self, fact_id: Uuid, review: &Review) -> ProposalResult<()> {
        let connection = self.lock();
        let changed = connection.execute(
            "UPDATE proposal SET state = ?2, reviewer = ?3 WHERE fact_id = ?1",
            params![
                fact_id.to_string(),
                state_str(review.state),
                review.reviewer.as_deref()
            ],
        )?;
        if changed == 0 {
            return Err(ProposalError::Unknown(fact_id));
        }
        Ok(())
    }

    /// The extraction whose [`Extraction::approved_operations`] are exactly
    /// what approving `fact` newly makes emittable.
    ///
    /// `fact` carries the *pending* approval (applied in memory, not yet
    /// persisted) so the operations can be built and applied before the store
    /// commits — see the module docs on write order.
    fn promotion(&self, proposal: &StoredProposal) -> ProposalResult<Extraction> {
        let mut objects: Vec<ExtractedObject> = Vec::new();
        let mut morphisms: Vec<ExtractedMorphism> = Vec::new();
        match &proposal.fact {
            StoredFact::Object(object) => {
                objects.push(object.clone());
                // Approving the last endpoint of an already-approved edge is
                // what finally lets that edge through.
                for edge in self.approved_edges_touching(&proposal.schema, proposal.fact_id)? {
                    let StoredFact::Morphism(morphism) = edge.fact else {
                        continue;
                    };
                    let (source, target) = (
                        *morphism.morphism.source.as_uuid(),
                        *morphism.morphism.target.as_uuid(),
                    );
                    let other = if source == proposal.fact_id {
                        target
                    } else {
                        source
                    };
                    let Some(other) = self.approved_object(other)? else {
                        continue; // still dangling; withheld
                    };
                    objects.push(other);
                    morphisms.push(morphism);
                }
            }
            StoredFact::Morphism(morphism) => {
                let source = self.approved_object(*morphism.morphism.source.as_uuid())?;
                let target = self.approved_object(*morphism.morphism.target.as_uuid())?;
                // Both endpoints must already be approved, or the edge stays
                // withheld — `approved_operations` enforces this too, but
                // building the batch honestly means not pretending otherwise.
                if let (Some(source), Some(target)) = (source, target) {
                    objects.push(source);
                    objects.push(target);
                    morphisms.push(morphism.clone());
                }
            }
        }
        // Endpoints of several unlocked edges can repeat; assert each once.
        let mut seen = std::collections::HashSet::new();
        objects.retain(|object| seen.insert(*object.object.id.as_uuid()));
        Ok(Extraction {
            extractor: proposal.extractor.clone(),
            schema: proposal.schema.clone(),
            objects,
            morphisms,
        })
    }
}

/// Approve `fact_id` and promote it into the overlay graph.
///
/// The promotion goes through [`Extraction::approved_operations`] — the one
/// method that yields [`CatOperation`]s, and only for facts a human accepted.
/// The engine's approval check therefore passes because the fact *genuinely
/// was approved*, with the reviewer stamped on `provenance.approved_by`;
/// nothing here weakens or side-steps it.
///
/// Approving an object also emits any already-approved morphism it was the
/// missing endpoint of (an edge is withheld until both endpoints are
/// approved, so it has to be re-offered when the second one lands).
pub async fn approve_and_promote(
    store: &ProposalStore,
    engine: &EmbeddedCatDb,
    fact_id: Uuid,
    reviewer: &str,
) -> ProposalResult<ApprovalOutcome> {
    let mut proposal = store.get(fact_id)?.ok_or(ProposalError::Unknown(fact_id))?;
    // Transition in memory first: an illegal transition (re-approving what is
    // already approved) must fail before anything is written anywhere.
    let mut review = proposal.review().clone();
    review.approve(reviewer)?;
    set_review(&mut proposal.fact, review.clone());

    let extraction = store.promotion(&proposal)?;
    let operations = extraction.approved_operations();
    let promoted = operation_ids(&operations);
    if !operations.is_empty() {
        engine
            .apply_operations(operations, Some(reviewer.to_string()))
            .await?;
    }
    // Engine first, store second — see the module docs.
    store.persist_review(fact_id, &review)?;
    Ok(ApprovalOutcome { fact_id, promoted })
}

/// Reject `fact_id`, and undo it if a previous approval had already put it in
/// the graph.
///
/// Rejecting an approved **object** cascades: every approved morphism with
/// that object as an endpoint is rejected and retracted too, because an edge
/// whose endpoint a human has ruled out cannot stand — the same "no dangling
/// edges" invariant [`Extraction::approved_operations`] enforces on the way
/// in, applied on the way out.
///
/// A fact that was never approved is simply marked rejected: nothing was
/// written, so nothing is retracted.
pub async fn reject_and_retract(
    store: &ProposalStore,
    engine: &EmbeddedCatDb,
    fact_id: Uuid,
    reviewer: &str,
) -> ProposalResult<RejectionOutcome> {
    let proposal = store.get(fact_id)?.ok_or(ProposalError::Unknown(fact_id))?;
    let mut review = proposal.review().clone();
    let was_approved = review.state == ApprovalState::Approved;
    review.reject(reviewer)?;

    let mut cascaded: Vec<(Uuid, Review)> = Vec::new();
    let mut operations: Vec<CatOperation> = Vec::new();
    if was_approved {
        match &proposal.fact {
            StoredFact::Object(_) => {
                // Edges first, then the object they hang off.
                for edge in store.approved_edges_touching(&proposal.schema, fact_id)? {
                    let mut edge_review = edge.review().clone();
                    edge_review.reject(reviewer)?;
                    cascaded.push((edge.fact_id, edge_review));
                    operations.push(CatOperation::RetractMorphism {
                        morphism_id: edge.fact_id,
                    });
                }
                operations.push(CatOperation::RetractObject { object_id: fact_id });
            }
            StoredFact::Morphism(_) => operations.push(CatOperation::RetractMorphism {
                morphism_id: fact_id,
            }),
        }
    }
    let retracted = operation_ids(&operations);
    if !operations.is_empty() {
        engine
            .apply_operations(operations, Some(reviewer.to_string()))
            .await?;
    }
    for (edge, edge_review) in &cascaded {
        store.persist_review(*edge, edge_review)?;
    }
    store.persist_review(fact_id, &review)?;
    Ok(RejectionOutcome {
        fact_id,
        retracted,
        cascaded: cascaded.into_iter().map(|(id, _)| id).collect(),
    })
}

// ---------------------------------------------------------------------------
// Row plumbing
// ---------------------------------------------------------------------------

/// One fact's worth of columns, ready to upsert.
struct FactRow {
    fact_id: Uuid,
    kind: &'static str,
    rule: String,
    qualified: String,
    source_path: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    content_hash: Option<String>,
    endpoint_src: Option<String>,
    endpoint_dst: Option<String>,
    payload: serde_json::Value,
}

fn facts_of(extraction: &Extraction) -> Vec<FactRow> {
    let mut rows = Vec::with_capacity(extraction.objects.len() + extraction.morphisms.len());
    for object in &extraction.objects {
        let provenance = &object.object.provenance;
        rows.push(FactRow {
            fact_id: *object.object.id.as_uuid(),
            kind: "object",
            rule: object.rule.clone(),
            qualified: object.object.entity.to_string(),
            source_path: provenance.source.clone(),
            start_line: provenance.range.map(|(start, _)| start),
            end_line: provenance.range.map(|(_, end)| end),
            content_hash: provenance.content_hash.clone(),
            endpoint_src: None,
            endpoint_dst: None,
            payload: serde_json::to_value(&object.object).expect("kernel object serializes"),
        });
    }
    for morphism in &extraction.morphisms {
        let provenance = &morphism.morphism.provenance;
        rows.push(FactRow {
            fact_id: *morphism.morphism.id.as_uuid(),
            kind: "morphism",
            rule: morphism.rule.clone(),
            qualified: morphism.morphism.arrow.to_string(),
            source_path: provenance.source.clone(),
            start_line: provenance.range.map(|(start, _)| start),
            end_line: provenance.range.map(|(_, end)| end),
            content_hash: provenance.content_hash.clone(),
            endpoint_src: Some(morphism.morphism.source.to_string()),
            endpoint_dst: Some(morphism.morphism.target.to_string()),
            payload: serde_json::to_value(&morphism.morphism).expect("kernel morphism serializes"),
        });
    }
    rows
}

/// What one fact's upsert did.
#[derive(Clone, Copy)]
enum Upserted {
    Inserted,
    Unchanged,
    Reproposed,
}

fn summary_add(summary: &mut RecordSummary, outcome: Upserted) {
    match outcome {
        Upserted::Inserted => summary.inserted += 1,
        Upserted::Unchanged => summary.unchanged += 1,
        Upserted::Reproposed => summary.reproposed += 1,
    }
}

fn upsert(
    tx: &rusqlite::Transaction<'_>,
    extraction: &Extraction,
    fact: &FactRow,
) -> ProposalResult<Upserted> {
    let existing: Option<(String, Option<String>, Option<String>)> = tx
        .prepare("SELECT state, reviewer, content_hash FROM proposal WHERE fact_id = ?1")?
        .query_row(params![fact.fact_id.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;

    let (outcome, state, reviewer) = match existing {
        None => (Upserted::Inserted, "proposed", None),
        // The cited text is the same text the human ruled on: their decision
        // stands (and a rejection is NOT re-proposed).
        Some((state, reviewer, hash)) if hash == fact.content_hash => {
            (Upserted::Unchanged, leak_state(&state), reviewer)
        }
        // The cited text changed under the same fact id — whatever was
        // decided was decided about different text.
        Some(_) => (Upserted::Reproposed, "proposed", None),
    };

    tx.execute(
        "INSERT INTO proposal (fact_id, kind, schema_name, extractor, rule, qualified, \
             source_path, start_line, end_line, content_hash, endpoint_src, endpoint_dst, \
             state, reviewer, payload) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) \
         ON CONFLICT(fact_id) DO UPDATE SET \
             kind = excluded.kind, schema_name = excluded.schema_name, \
             extractor = excluded.extractor, rule = excluded.rule, \
             qualified = excluded.qualified, source_path = excluded.source_path, \
             start_line = excluded.start_line, end_line = excluded.end_line, \
             content_hash = excluded.content_hash, endpoint_src = excluded.endpoint_src, \
             endpoint_dst = excluded.endpoint_dst, state = excluded.state, \
             reviewer = excluded.reviewer, payload = excluded.payload",
        params![
            fact.fact_id.to_string(),
            fact.kind,
            extraction.schema,
            extraction.extractor,
            fact.rule,
            fact.qualified,
            fact.source_path,
            fact.start_line,
            fact.end_line,
            fact.content_hash,
            fact.endpoint_src,
            fact.endpoint_dst,
            state,
            reviewer,
            serde_json::to_string(&fact.payload).expect("json value serializes"),
        ],
    )?;
    Ok(outcome)
}

/// The canonical spelling of a state read back from the database, so an
/// unrecognized value cannot round-trip untouched.
fn leak_state(raw: &str) -> &'static str {
    state_str(parse_state(raw))
}

fn row_to_proposal(row: &rusqlite::Row<'_>) -> ProposalResult<StoredProposal> {
    let fact_id: String = row.get(0)?;
    let kind: String = row.get(1)?;
    let schema: String = row.get(2)?;
    let extractor: String = row.get(3)?;
    let rule: String = row.get(4)?;
    let state: String = row.get(5)?;
    let reviewer: Option<String> = row.get(6)?;
    let payload: String = row.get(7)?;

    let fact_id = Uuid::parse_str(&fact_id).unwrap_or_default();
    let review = Review {
        state: parse_state(&state),
        reviewer,
    };
    let fact = if kind == "object" {
        StoredFact::Object(ExtractedObject {
            object: serde_json::from_str(&payload).map_err(|source| ProposalError::Payload {
                id: fact_id,
                source,
            })?,
            rule,
            review,
        })
    } else {
        StoredFact::Morphism(ExtractedMorphism {
            morphism: serde_json::from_str(&payload).map_err(|source| ProposalError::Payload {
                id: fact_id,
                source,
            })?,
            rule,
            review,
        })
    };
    Ok(StoredProposal {
        fact_id,
        schema,
        extractor,
        fact,
    })
}

fn set_review(fact: &mut StoredFact, review: Review) {
    match fact {
        StoredFact::Object(object) => object.review = review,
        StoredFact::Morphism(morphism) => morphism.review = review,
    }
}

/// The ids an operation batch touches, in batch order.
fn operation_ids(operations: &[CatOperation]) -> Vec<Uuid> {
    operations
        .iter()
        .filter_map(|op| match op {
            CatOperation::AssertObject { object_id, .. } => Some(*object_id),
            CatOperation::AssertMorphism { morphism_id, .. } => Some(*morphism_id),
            CatOperation::RetractObject { object_id } => Some(*object_id),
            CatOperation::RetractMorphism { morphism_id } => Some(*morphism_id),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip_through_their_stored_spelling() {
        for state in [
            ApprovalState::Proposed,
            ApprovalState::Approved,
            ApprovalState::Rejected,
        ] {
            assert_eq!(parse_state(state_str(state)), state);
        }
    }

    /// An unreadable state must never read back as approved — the one
    /// direction in which failing open would put an unreviewed fact into the
    /// authoritative graph.
    #[test]
    fn an_unrecognized_state_reads_back_as_unreviewed() {
        assert_eq!(parse_state("APPROVED"), ApprovalState::Proposed);
        assert_eq!(parse_state(""), ApprovalState::Proposed);
        assert_eq!(leak_state("nonsense"), "proposed");
    }

    #[test]
    fn a_fresh_store_is_empty_and_versioned() {
        let store = ProposalStore::in_memory().expect("open");
        assert!(store
            .list(&ProposalFilter::default())
            .expect("list")
            .is_empty());
        let version: i64 = store
            .lock()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("user_version");
        assert_eq!(version, STORE_VERSION);
    }
}
