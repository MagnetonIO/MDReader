//! One borrowed profile-selection connection, separate from all library owners.
//! Original files are never restored/renamed/deleted by these transactions.
mod schema;
mod switch;
mod types;
use crate::{
    model::{digest, valid_hash},
    Error, Inventory, Limits, Result, RootBinding, VerifiedSnapshot,
};
use rusqlite::{params, Connection, OptionalExtension};
pub use types::*;
use uuid::Uuid;

pub struct Journal<'a> {
    connection: &'a mut Connection,
}
impl<'a> Journal<'a> {
    /// The caller is the sole native lifecycle owner; this never opens a DB.
    pub fn attach(connection: &'a mut Connection) -> Result<Self> {
        schema::initialize(connection)?;
        Ok(Self { connection })
    }
    /// Explicit trusted-native initial setup, only for a fresh selector. An
    /// existing selector must match; it is never inferred from legacy JSON.
    pub fn bootstrap(&mut self, inventory: &Inventory) -> Result<Selection> {
        inventory.revalidate()?;
        let root = inventory.binding().clone();
        let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
        schema::validate(&tx)?;
        tx.execute(
            "INSERT OR IGNORE INTO profile_selection_v1 VALUES(1,?1,0,'legacy_current')",
            [encode(&root)?],
        )
        .map_err(|_| Error::Storage)?;
        let selected = selection_on(&tx)?;
        if selected.root != root {
            return Err(Error::Conflict);
        }
        inventory.revalidate()?;
        tx.commit().map_err(|_| Error::Storage)?;
        Ok(selected)
    }
    pub fn selection(&mut self) -> Result<Selection> {
        let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
        schema::validate(&tx)?;
        let result = selection_on(&tx)?;
        tx.commit().map_err(|_| Error::Storage)?;
        Ok(result)
    }
    pub fn prepare(&mut self, inventory: &Inventory, expected_generation: u64) -> Result<Uuid> {
        inventory.revalidate()?;
        let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
        schema::validate(&tx)?;
        let selected = selection_on(&tx)?;
        if selected.mode != SelectionMode::LegacyCurrent
            || selected.generation != expected_generation
            || &selected.root != inventory.binding()
        {
            return Err(Error::Conflict);
        }
        let count: u64 = tx
            .query_row("SELECT COUNT(*) FROM profile_transitions_v1", [], |r| {
                r.get(0)
            })
            .map_err(|_| Error::Storage)?;
        if count >= 1024 {
            return Err(Error::Bounds);
        }
        let id = Uuid::new_v4();
        tx.execute(
            "INSERT INTO profile_transitions_v1 \
             (id,original,expected_generation,inventory_hash,state) \
             VALUES(?1,?2,?3,?4,'prepared')",
            params![
                id.to_string(),
                encode(inventory.binding())?,
                expected_generation,
                inventory.identity()
            ],
        )
        .map_err(|_| Error::Storage)?;
        inventory.revalidate()?;
        tx.commit().map_err(|_| Error::Storage)?;
        Ok(id)
    }
    pub fn preserved(
        &mut self,
        id: Uuid,
        snapshot: &VerifiedSnapshot,
        limits: &Limits,
    ) -> Result<()> {
        snapshot.revalidate(limits)?;
        let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
        schema::validate(&tx)?;
        let op = transition_on(&tx, id)?;
        if op.original != snapshot.manifest().original
            || op.inventory_hash != snapshot.manifest().inventory_sha256
        {
            return Err(Error::Conflict);
        }
        if op.state != State::Prepared && op.snapshot.as_ref() != Some(snapshot.identity()) {
            return Err(Error::Conflict);
        }
        if op.state == State::Prepared {
            tx.execute(
                "UPDATE profile_transitions_v1 SET state='preserved',snapshot=?2 WHERE id=?1",
                params![id.to_string(), encode(snapshot.identity())?],
            )
            .map_err(|_| Error::Storage)?;
        }
        tx.commit().map_err(|_| Error::Storage)
    }
    /// Candidate receipt is checked by the real frozen owner. A serialized
    /// receipt alone never establishes committed imported data.
    pub fn staged(
        &mut self,
        id: Uuid,
        receipt: &CandidateReceipt,
        owner: &mut impl ActivationOwner,
    ) -> Result<()> {
        receipt.validate()?;
        let op = self.get(id)?;
        if receipt.operation != id
            || receipt.original != op.original
            || Some(&receipt.snapshot) != op.snapshot.as_ref()
            || !matches!(op.state, State::Preserved | State::Staged)
        {
            return Err(Error::Conflict);
        }
        let mut freeze_scope = op.clone();
        freeze_scope.candidate = Some(receipt.clone());
        owner.with_frozen(&freeze_scope, |frozen| {
            frozen.revalidate()?;
            frozen.verify_candidate(receipt)?;
            let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
            schema::validate(&tx)?;
            if digest(&transition_on(&tx, id)?)? != digest(&op)? {
                return Err(Error::Conflict);
            }
            if op
                .candidate
                .as_ref()
                .is_some_and(|old| digest(old).ok() != digest(receipt).ok())
            {
                return Err(Error::Conflict);
            }
            tx.execute(
                "UPDATE profile_transitions_v1 SET state='staged',candidate=?2 WHERE id=?1",
                params![id.to_string(), encode(receipt)?],
            )
            .map_err(|_| Error::Storage)?;
            let mut expected = op.clone();
            expected.state = State::Staged;
            expected.candidate = Some(receipt.clone());
            schema::validate(&tx)?;
            if digest(&transition_on(&tx, id)?)? != digest(&expected)? {
                return Err(Error::Conflict);
            }
            frozen.verify_candidate(receipt)?;
            frozen.revalidate()?;
            tx.commit().map_err(|_| Error::Storage)
        })
    }
    /// Bounded restart inventory; native UI must further sanitize paths/metadata.
    pub fn recover(&mut self, after: Option<Uuid>, limit: usize) -> Result<Vec<Transition>> {
        if limit == 0 || limit > 16 {
            return Err(Error::Bounds);
        }
        let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
        schema::validate(&tx)?;
        let ids = tx
            .prepare("SELECT id FROM profile_transitions_v1 WHERE id>?1 ORDER BY id LIMIT ?2")
            .map_err(|_| Error::Storage)?
            .query_map(
                params![after.map(|id| id.to_string()).unwrap_or_default(), limit],
                |r| r.get::<_, String>(0),
            )
            .map_err(|_| Error::Storage)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| Error::Storage)?;
        let result = ids
            .into_iter()
            .map(|id| transition_on(&tx, Uuid::parse_str(&id).map_err(|_| Error::Invalid)?))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().map_err(|_| Error::Storage)?;
        Ok(result)
    }
    pub fn get(&mut self, id: Uuid) -> Result<Transition> {
        let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
        schema::validate(&tx)?;
        let result = transition_on(&tx, id)?;
        tx.commit().map_err(|_| Error::Storage)?;
        Ok(result)
    }
}
/// Read the exact selection and operation from a caller's single read snapshot.
/// No schema initialization or lifecycle mutation is performed.
pub fn observe(tx: &rusqlite::Transaction<'_>, id: Uuid) -> Result<(Transition, Selection)> {
    schema::validate(tx)?;
    Ok((transition_on(tx, id)?, selection_on(tx)?))
}
pub(super) fn encode(value: &impl serde::Serialize) -> Result<String> {
    let value = serde_json::to_string(value).map_err(|_| Error::Invalid)?;
    if value.len() > 262144 {
        return Err(Error::Bounds);
    }
    Ok(value)
}
fn decode<T: serde::de::DeserializeOwned>(text: String) -> Result<T> {
    if text.len() > 262144 {
        return Err(Error::Bounds);
    }
    serde_json::from_str(&text).map_err(|_| Error::Invalid)
}
pub(super) fn selection_on(connection: &Connection) -> Result<Selection> {
    let (root, generation, mode) = connection
        .query_row(
            "SELECT root,generation,mode FROM profile_selection_v1 WHERE singleton=1",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|_| Error::Invalid)?;
    let root: RootBinding = decode(root)?;
    root.validate()?;
    if generation > MAX_GENERATION {
        return Err(Error::Invalid);
    }
    Ok(Selection {
        root,
        generation,
        mode: SelectionMode::from_sql(&mode)?,
    })
}
pub(super) fn transition_on(connection: &Connection, id: Uuid) -> Result<Transition> {
    let row = connection
        .query_row(
            "SELECT original,expected_generation,inventory_hash,state,snapshot,candidate, \
         activation_generation,rollback_generation FROM profile_transitions_v1 WHERE id=?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<u64>>(6)?,
                    r.get::<_, Option<u64>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|_| Error::Storage)?
        .ok_or(Error::Invalid)?;
    let result = Transition {
        id,
        original: decode(row.0)?,
        expected_generation: row.1,
        inventory_hash: row.2,
        state: State::from_sql(&row.3)?,
        snapshot: row.4.map(decode).transpose()?,
        candidate: row.5.map(decode).transpose()?,
        activation_generation: row.6,
        rollback_generation: row.7,
    };
    result.original.validate()?;
    if !valid_hash(&result.inventory_hash) || result.expected_generation > MAX_GENERATION {
        return Err(Error::Invalid);
    }
    if result
        .snapshot
        .as_ref()
        .is_some_and(|s| !valid_hash(&s.manifest_sha256))
        || result
            .activation_generation
            .is_some_and(|g| g > MAX_GENERATION)
        || result
            .rollback_generation
            .is_some_and(|g| g > MAX_GENERATION)
    {
        return Err(Error::Invalid);
    }
    if let Some(candidate) = &result.candidate {
        candidate.validate()?;
        if candidate.operation != id
            || candidate.original != result.original
            || Some(&candidate.snapshot) != result.snapshot.as_ref()
        {
            return Err(Error::Invalid);
        }
    }
    let valid = match result.state {
        State::Prepared => {
            result.snapshot.is_none()
                && result.candidate.is_none()
                && result.activation_generation.is_none()
                && result.rollback_generation.is_none()
        }
        State::Preserved => {
            result.snapshot.is_some()
                && result.candidate.is_none()
                && result.activation_generation.is_none()
                && result.rollback_generation.is_none()
        }
        State::Staged => {
            result.snapshot.is_some()
                && result.candidate.is_some()
                && result.activation_generation.is_none()
                && result.rollback_generation.is_none()
        }
        State::Active => {
            result.snapshot.is_some()
                && result.candidate.is_some()
                && result.activation_generation == result.expected_generation.checked_add(1)
                && result.rollback_generation.is_none()
        }
        State::RolledBack => {
            result.snapshot.is_some()
                && result.candidate.is_some()
                && result.activation_generation == result.expected_generation.checked_add(1)
                && result.rollback_generation
                    == result.activation_generation.and_then(|g| g.checked_add(1))
        }
    };
    if !valid {
        return Err(Error::Invalid);
    }
    Ok(result)
}
