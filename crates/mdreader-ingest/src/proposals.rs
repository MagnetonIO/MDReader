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
//! derives deterministically from `(schema, entity, path, line)`, so a
//! re-import *recognizes* facts rather than re-proposing them. What happens to
//! a review depends on both its state and whether the **cited text** changed
//! (a different `content_hash` under the same id):
//!
//! | stored state | cited text unchanged | cited text CHANGED |
//! |---|---|---|
//! | `Proposed` | stays `Proposed` | stays `Proposed`; payload refreshed (`refreshed`) |
//! | `Approved` | stays `Approved`, reviewer kept (`recognized`) | **resets to `Proposed`**, reviewer cleared (`reproposed`) |
//! | `Rejected` | stays `Rejected`, reviewer kept (`recognized`) | **stays `Rejected`**, reviewer kept, flagged as changed (`rejection_held`) |
//!
//! The asymmetry in the right-hand column is the point, and it is not
//! symmetric because the two errors are not symmetric:
//!
//! * An **approval** that survived a text change would leave a claim in the
//!   authoritative graph vouched for by text nobody read. That is a false
//!   assertion, so the approval is dropped and the fact must be accepted again.
//! * A **rejection** that did not survive a text change would silently
//!   resurrect a claim a human deliberately ruled out — and in a corpus under
//!   active editing, a reworded sentence or a whitespace fix would do it,
//!   over and over, with nothing on screen saying the claim had ever been
//!   rejected. Un-rejecting is not a decision extraction gets to make.
//!
//! So a rejection is preserved, and the information is not thrown away
//! either: [`StoredProposal::reviewed_content_hash`] keeps the hash of the
//! text the human actually ruled on, so
//! [`StoredProposal::source_changed_since_review`] can tell the Inspector to
//! offer *"previously rejected — the source has since changed; review
//! again?"*. Re-review stays a deliberate human act, which is the whole
//! difference between an approval model and a suggestion box. (If the text
//! later changes *back* to what was rejected, the two hashes match again and
//! the flag clears by itself.)
//!
//! A row whose fact stopped extracting is kept, deliberately, as the tombstone
//! that stops a deleted-then-restored heading from re-proposing something
//! already rejected.
//!
//! **Known gap:** resetting an approval to `Proposed` does not retract the
//! fact the earlier approval already committed — `record` runs inside ingest
//! with no engine handle. The divergence is visible rather than hidden: the
//! Inspector measures `in_graph` against storage, so such a fact reads
//! `proposed` *and* `in_graph = true`, which is the truth.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use catdb_sdk::engine::EmbeddedCatDb;
use catdb_sdk::ir::CatOperation;
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
    Engine(#[from] catdb_sdk::engine::EngineError),
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
    /// blake3 of the cited text **as it stood when the reviewer ruled on it**;
    /// `None` while the fact is unreviewed.
    ///
    /// Kept separately from the fact's own `provenance.content_hash` (which
    /// always tracks the *current* extraction) so a preserved rejection can
    /// still say which text it was a rejection *of*. See
    /// [`Self::source_changed_since_review`].
    pub reviewed_content_hash: Option<String>,
}

impl StoredProposal {
    /// The fact's review state.
    pub fn state(&self) -> ApprovalState {
        self.review().state
    }

    /// Whether the cited text has changed since the reviewer ruled on it.
    ///
    /// Only ever true for a **preserved rejection**: an approval whose text
    /// changed is reset to `Proposed` (which clears the review outright), and
    /// an unreviewed fact has nothing to have changed *since*. This is the
    /// signal the Inspector renders as *"previously rejected — the source has
    /// since changed; review again?"*, keeping re-review a deliberate act
    /// instead of something a whitespace edit does silently.
    pub fn source_changed_since_review(&self) -> bool {
        match &self.reviewed_content_hash {
            Some(reviewed) => Some(reviewed) != self.provenance().content_hash.as_ref(),
            None => false,
        }
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
    pub fn provenance(&self) -> &catdb_sdk::core::Provenance {
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

/// What [`ProposalStore::record`] did to the store — one counter per row of
/// the reconciliation table in the module docs, so "what did that re-import
/// do to my review decisions?" has a numeric answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordSummary {
    /// Facts the store had never seen, inserted as `Proposed`.
    pub inserted: u64,
    /// Facts whose cited text is unchanged: the review, whatever it was, was
    /// preserved exactly.
    pub recognized: u64,
    /// Unreviewed facts whose cited text changed: the payload was refreshed
    /// and they stayed `Proposed`. Nothing was decided, so nothing was lost.
    pub refreshed: u64,
    /// **Approved** facts whose cited text changed: the approval was cleared
    /// and they are `Proposed` again. The human accepted text that is no
    /// longer there.
    pub reproposed: u64,
    /// **Rejected** facts whose cited text changed: the rejection was
    /// preserved and flagged
    /// ([`StoredProposal::source_changed_since_review`]). Un-rejecting is a
    /// human act, not something a re-import performs.
    pub rejection_held: u64,
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
///
/// * 1 — initial shape.
/// * 2 — `reviewed_hash`: the cited-text hash a review was made against, so a
///   preserved rejection can be flagged when the source moves under it.
const STORE_VERSION: i64 = 2;

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS proposal (
    fact_id       TEXT PRIMARY KEY NOT NULL,
    kind          TEXT NOT NULL,
    schema_name   TEXT NOT NULL,
    extractor     TEXT NOT NULL,
    rule          TEXT NOT NULL,
    qualified     TEXT NOT NULL,
    source_path   TEXT,
    start_line    INTEGER,
    end_line      INTEGER,
    content_hash  TEXT,
    endpoint_src  TEXT,
    endpoint_dst  TEXT,
    state         TEXT NOT NULL,
    reviewer      TEXT,
    reviewed_hash TEXT,
    payload       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS proposal_by_span ON proposal (source_path, start_line, end_line);
CREATE INDEX IF NOT EXISTS proposal_by_schema ON proposal (schema_name, kind);
CREATE INDEX IF NOT EXISTS proposal_by_src ON proposal (endpoint_src);
CREATE INDEX IF NOT EXISTS proposal_by_dst ON proposal (endpoint_dst);
";

/// Column list shared by every read, in the order [`row_to_proposal`] expects.
const COLUMNS: &str = "fact_id, kind, schema_name, extractor, rule, state, reviewer, payload, \
     reviewed_hash";

/// Objects before morphisms, then a stable tie-break — so a UI listing and a
/// golden test see the same order every time.
const ORDER: &str = "ORDER BY source_path, start_line, end_line, \
     CASE kind WHEN 'object' THEN 0 ELSE 1 END, qualified, fact_id";

/// Bring a store written by an older revision up to [`STORE_VERSION`].
///
/// `CREATE TABLE IF NOT EXISTS` is a no-op on an existing table, so a column
/// added after the fact has to be added explicitly. The check is on the real
/// column list rather than on `user_version`, and the `ALTER` runs only when
/// the column is genuinely absent — so a failure is a real failure and is
/// reported, never swallowed as "probably already migrated".
///
/// A v1 row gets `reviewed_hash = NULL`, which reads as "never reviewed
/// against a recorded hash". That is the safe direction: an existing rejection
/// keeps its state, and simply is not flagged as source-changed until the next
/// review stamps a hash.
fn migrate(connection: &Connection) -> ProposalResult<()> {
    let mut columns = connection.prepare("PRAGMA table_info(proposal)")?;
    let has_reviewed_hash = columns
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<String>>>()?
        .iter()
        .any(|name| name == "reviewed_hash");
    drop(columns);
    if !has_reviewed_hash {
        connection.execute("ALTER TABLE proposal ADD COLUMN reviewed_hash TEXT", [])?;
    }
    Ok(())
}

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
        migrate(&connection)?;
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

    /// Write a review transition back to the store, stamping the cited-text
    /// hash the reviewer ruled *against*.
    ///
    /// `reviewed_hash = content_hash` is set from the row itself rather than
    /// passed in, so the recorded hash is by construction the one the fact
    /// currently carries — a caller cannot stamp a review against text the
    /// store does not hold.
    fn persist_review(&self, fact_id: Uuid, review: &Review) -> ProposalResult<()> {
        let connection = self.lock();
        let changed = connection.execute(
            "UPDATE proposal SET state = ?2, reviewer = ?3, reviewed_hash = content_hash \
             WHERE fact_id = ?1",
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

/// What one fact's upsert did — one variant per row of the reconciliation
/// table in the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Upserted {
    Inserted,
    Recognized,
    Refreshed,
    Reproposed,
    RejectionHeld,
}

fn summary_add(summary: &mut RecordSummary, outcome: Upserted) {
    match outcome {
        Upserted::Inserted => summary.inserted += 1,
        Upserted::Recognized => summary.recognized += 1,
        Upserted::Refreshed => summary.refreshed += 1,
        Upserted::Reproposed => summary.reproposed += 1,
        Upserted::RejectionHeld => summary.rejection_held += 1,
    }
}

/// The review a re-import leaves on an existing fact.
struct Reconciled {
    outcome: Upserted,
    state: &'static str,
    reviewer: Option<String>,
    /// The hash the surviving review was made against; `None` once there is
    /// no surviving review.
    reviewed_hash: Option<String>,
}

/// Decide what a re-import does to one already-stored fact.
///
/// Split out from the SQL so the state table in the module docs is one
/// readable `match` that a test can drive directly.
fn reconcile(
    stored_state: &str,
    stored_reviewer: Option<String>,
    stored_reviewed_hash: Option<String>,
    text_unchanged: bool,
) -> Reconciled {
    if text_unchanged {
        // Same text the human ruled on: the decision stands, verbatim.
        return Reconciled {
            outcome: Upserted::Recognized,
            state: leak_state(stored_state),
            reviewer: stored_reviewer,
            reviewed_hash: stored_reviewed_hash,
        };
    }
    match parse_state(stored_state) {
        // Nothing was decided, so nothing can be lost: the refreshed payload
        // simply carries on as a proposal.
        ApprovalState::Proposed => Reconciled {
            outcome: Upserted::Refreshed,
            state: "proposed",
            reviewer: None,
            reviewed_hash: None,
        },
        // The approval vouched for text that is no longer there. Keeping it
        // would leave a claim in the authoritative graph nobody has read the
        // evidence for, so it must be accepted again.
        ApprovalState::Approved => Reconciled {
            outcome: Upserted::Reproposed,
            state: "proposed",
            reviewer: None,
            reviewed_hash: None,
        },
        // The rejection HOLDS. Dropping it would silently resurrect a claim a
        // human ruled out — and a reworded sentence would do it, repeatedly,
        // with nothing on screen saying it had ever been rejected. The
        // reviewed hash is kept so the Inspector can flag the drift and offer
        // an explicit re-review (see the module docs).
        ApprovalState::Rejected => Reconciled {
            outcome: Upserted::RejectionHeld,
            state: "rejected",
            reviewer: stored_reviewer,
            reviewed_hash: stored_reviewed_hash,
        },
    }
}

fn upsert(
    tx: &rusqlite::Transaction<'_>,
    extraction: &Extraction,
    fact: &FactRow,
) -> ProposalResult<Upserted> {
    type ExistingRow = (String, Option<String>, Option<String>, Option<String>);
    let existing: Option<ExistingRow> = tx
        .prepare(
            "SELECT state, reviewer, content_hash, reviewed_hash FROM proposal WHERE fact_id = ?1",
        )?
        .query_row(params![fact.fact_id.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;

    let reconciled = match existing {
        None => Reconciled {
            outcome: Upserted::Inserted,
            state: "proposed",
            reviewer: None,
            reviewed_hash: None,
        },
        Some((state, reviewer, content_hash, reviewed_hash)) => reconcile(
            &state,
            reviewer,
            reviewed_hash,
            content_hash == fact.content_hash,
        ),
    };

    tx.execute(
        "INSERT INTO proposal (fact_id, kind, schema_name, extractor, rule, qualified, \
             source_path, start_line, end_line, content_hash, endpoint_src, endpoint_dst, \
             state, reviewer, reviewed_hash, payload) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
         ON CONFLICT(fact_id) DO UPDATE SET \
             kind = excluded.kind, schema_name = excluded.schema_name, \
             extractor = excluded.extractor, rule = excluded.rule, \
             qualified = excluded.qualified, source_path = excluded.source_path, \
             start_line = excluded.start_line, end_line = excluded.end_line, \
             content_hash = excluded.content_hash, endpoint_src = excluded.endpoint_src, \
             endpoint_dst = excluded.endpoint_dst, state = excluded.state, \
             reviewer = excluded.reviewer, reviewed_hash = excluded.reviewed_hash, \
             payload = excluded.payload",
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
            reconciled.state,
            reconciled.reviewer,
            reconciled.reviewed_hash,
            serde_json::to_string(&fact.payload).expect("json value serializes"),
        ],
    )?;
    Ok(reconciled.outcome)
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
    let reviewed_content_hash: Option<String> = row.get(8)?;

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
        reviewed_content_hash,
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

    /// The reconciliation table from the module docs, driven directly — one
    /// assertion per cell, so the documented invariant and the code are the
    /// same statement.
    #[test]
    fn the_reconciliation_table_is_exactly_what_the_docs_state() {
        let reviewed = || Some("blake3:old".to_string());
        let alice = || Some("alice".to_string());

        // Left column: the cited text is unchanged. Every decision stands.
        for state in ["proposed", "approved", "rejected"] {
            let result = reconcile(state, alice(), reviewed(), true);
            assert_eq!(result.outcome, Upserted::Recognized, "{state}");
            assert_eq!(result.state, state, "{state}");
            assert_eq!(result.reviewer, alice(), "{state}");
            assert_eq!(result.reviewed_hash, reviewed(), "{state}");
        }

        // Right column, unreviewed: nothing was decided, nothing is lost.
        let refreshed = reconcile("proposed", None, None, false);
        assert_eq!(refreshed.outcome, Upserted::Refreshed);
        assert_eq!(refreshed.state, "proposed");
        assert_eq!(refreshed.reviewer, None);

        // Right column, approved: the approval vouched for text that is gone,
        // so it is cleared and the fact must be accepted again.
        let reproposed = reconcile("approved", alice(), reviewed(), false);
        assert_eq!(reproposed.outcome, Upserted::Reproposed);
        assert_eq!(reproposed.state, "proposed");
        assert_eq!(
            reproposed.reviewer, None,
            "the approval is not carried over"
        );
        assert_eq!(reproposed.reviewed_hash, None);

        // Right column, rejected: the rejection HOLDS, with the hash it was
        // made against kept so the change can be surfaced.
        let held = reconcile("rejected", alice(), reviewed(), false);
        assert_eq!(held.outcome, Upserted::RejectionHeld);
        assert_eq!(held.state, "rejected", "a rejection is never re-proposed");
        assert_eq!(held.reviewer, alice(), "and it still names who said no");
        assert_eq!(held.reviewed_hash, reviewed());
    }

    /// The flag is derived from the two hashes, so it clears by itself if the
    /// text is edited back to what was rejected.
    #[test]
    fn source_changed_is_the_difference_between_the_current_and_reviewed_hash() {
        let proposal = |content: &str, reviewed: Option<&str>| StoredProposal {
            fact_id: Uuid::nil(),
            schema: "decision_overlay".into(),
            extractor: "mdreader.decision@1".into(),
            fact: StoredFact::Object(ExtractedObject {
                object: catdb_sdk::core::Object {
                    id: catdb_sdk::core::ObjectId::from_uuid(Uuid::nil()),
                    entity: "decision_overlay.Decision".into(),
                    value: serde_json::Value::Null,
                    provenance: catdb_sdk::core::Provenance {
                        content_hash: Some(content.to_string()),
                        ..catdb_sdk::core::Provenance::default()
                    },
                },
                rule: "D1".into(),
                review: Review::default(),
            }),
            reviewed_content_hash: reviewed.map(ToString::to_string),
        };
        assert!(!proposal("blake3:a", None).source_changed_since_review());
        assert!(!proposal("blake3:a", Some("blake3:a")).source_changed_since_review());
        assert!(proposal("blake3:b", Some("blake3:a")).source_changed_since_review());
        // Edited back to the rejected text: the flag clears on its own.
        assert!(!proposal("blake3:a", Some("blake3:a")).source_changed_since_review());
    }

    /// A store written before `reviewed_hash` existed opens, migrates, and
    /// keeps its rows — a pending review decision must not be lost to an
    /// upgrade.
    #[test]
    fn a_v1_store_migrates_in_place_without_losing_a_decision() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let path = scratch.path().join("proposals.db");

        // Hand-build the v1 shape (no `reviewed_hash`) and put a rejection in it.
        let legacy = Connection::open(&path).expect("open");
        legacy
            .execute_batch(
                "CREATE TABLE proposal (
                     fact_id TEXT PRIMARY KEY NOT NULL, kind TEXT NOT NULL,
                     schema_name TEXT NOT NULL, extractor TEXT NOT NULL, rule TEXT NOT NULL,
                     qualified TEXT NOT NULL, source_path TEXT, start_line INTEGER,
                     end_line INTEGER, content_hash TEXT, endpoint_src TEXT, endpoint_dst TEXT,
                     state TEXT NOT NULL, reviewer TEXT, payload TEXT NOT NULL);
                 INSERT INTO proposal VALUES (
                     '00000000-0000-0000-0000-000000000001', 'object', 'decision_overlay',
                     'mdreader.decision@1', 'D1', 'decision_overlay.Decision', 'a.md', 1, 3,
                     'blake3:a', NULL, NULL, 'rejected', 'bob',
                     '{\"id\":\"00000000-0000-0000-0000-000000000001\",\
                        \"entity\":\"decision_overlay.Decision\",\"value\":null,\
                        \"provenance\":{\"content_hash\":\"blake3:a\"}}');",
            )
            .expect("legacy schema");
        drop(legacy);

        let store = ProposalStore::open(&path).expect("migrating open");
        let rows = store.list(&ProposalFilter::default()).expect("list");
        assert_eq!(rows.len(), 1, "the row survived the migration");
        assert_eq!(rows[0].state(), ApprovalState::Rejected);
        assert_eq!(rows[0].review().reviewer.as_deref(), Some("bob"));
        assert_eq!(
            rows[0].reviewed_content_hash, None,
            "a v1 row has no recorded review hash, so it is not falsely flagged"
        );
        assert!(!rows[0].source_changed_since_review());
        // Re-opening an already-migrated store is a no-op, not an error.
        assert!(ProposalStore::open(&path).is_ok());
    }
}
