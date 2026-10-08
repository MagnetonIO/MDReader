use super::{
    encode, schema, selection_on, transition_on, ActivationOwner, ActivationReceipt, Journal,
    ReceiptKind, SelectionMode, State, MAX_GENERATION,
};
use crate::{model::digest, Error, Result};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

impl Journal<'_> {
    pub fn activate(
        &mut self,
        id: Uuid,
        expected_generation: u64,
        owner: &mut impl ActivationOwner,
    ) -> Result<ActivationReceipt> {
        self.switch(id, expected_generation, ReceiptKind::Activation, owner)
    }
    pub fn rollback(
        &mut self,
        id: Uuid,
        expected_generation: u64,
        owner: &mut impl ActivationOwner,
    ) -> Result<ActivationReceipt> {
        self.switch(id, expected_generation, ReceiptKind::Rollback, owner)
    }
    fn switch(
        &mut self,
        id: Uuid,
        expected_generation: u64,
        kind: ReceiptKind,
        owner: &mut impl ActivationOwner,
    ) -> Result<ActivationReceipt> {
        let op = self.get(id)?;
        // Receipt replay reconciles an uncertain reply; it never selects again.
        if let Some(receipt) = receipt_on(self.connection, id, kind)? {
            return Ok(receipt);
        }
        let candidate = op.candidate.as_ref().ok_or(Error::Invalid)?;
        candidate.validate()?;
        let (expected_mode, new_mode) = match kind {
            ReceiptKind::Activation => (SelectionMode::LegacyCurrent, SelectionMode::ReaderOnly),
            ReceiptKind::Rollback => (SelectionMode::ReaderOnly, SelectionMode::LegacyCurrent),
        };
        let (expected_root, new_root, required_state) = match kind {
            ReceiptKind::Activation => (&op.original, &candidate.candidate, State::Staged),
            ReceiptKind::Rollback => (&candidate.candidate, &op.original, State::Active),
        };
        if op.state != required_state
            || (kind == ReceiptKind::Activation && expected_generation != op.expected_generation)
            || (kind == ReceiptKind::Rollback
                && op.activation_generation != Some(expected_generation))
        {
            return Err(Error::Conflict);
        }
        let next = expected_generation
            .checked_add(1)
            .filter(|g| *g <= MAX_GENERATION)
            .ok_or(Error::Bounds)?;
        owner.with_frozen(&op, |frozen| {
            frozen.revalidate()?;
            frozen.verify_candidate(candidate)?;
            if kind == ReceiptKind::Rollback {
                let activation = receipt_on(self.connection, id, ReceiptKind::Activation)?
                    .ok_or(Error::Invalid)?;
                frozen.verify_rollback_preservation(&activation)?;
            }
            let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
            schema::validate(&tx)?;
            if digest(&transition_on(&tx, id)?)? != digest(&op)? {
                return Err(Error::Conflict);
            }
            let selected = selection_on(&tx)?;
            if selected.mode != expected_mode
                || selected.generation != expected_generation
                || &selected.root != expected_root
            {
                return Err(Error::Conflict);
            }
            let receipt = ActivationReceipt {
                id: Uuid::new_v4(),
                operation: id,
                kind,
                from_generation: expected_generation,
                to_generation: next,
                selected_root: new_root.clone(),
                selected_mode: new_mode,
            };
            tx.execute(
                "UPDATE profile_selection_v1 SET root=?1,generation=?2,mode=?4 \
                 WHERE singleton=1 AND generation=?3",
                params![encode(new_root)?, next, expected_generation, new_mode.sql()],
            )
            .map_err(|_| Error::Storage)?;
            let mut expected = op.clone();
            let column = match kind {
                ReceiptKind::Activation => {
                    expected.state = State::Active;
                    expected.activation_generation = Some(next);
                    "activation_generation"
                }
                ReceiptKind::Rollback => {
                    expected.state = State::RolledBack;
                    expected.rollback_generation = Some(next);
                    "rollback_generation"
                }
            };
            let update =
                format!("UPDATE profile_transitions_v1 SET state=?2,{column}=?3 WHERE id=?1");
            tx.execute(&update, params![id.to_string(), expected.state.sql(), next])
                .map_err(|_| Error::Storage)?;
            tx.execute(
                "INSERT INTO profile_activation_receipts_v1 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    receipt.id.to_string(),
                    id.to_string(),
                    kind.sql(),
                    expected_generation,
                    next,
                    encode(new_root)?,
                    new_mode.sql()
                ],
            )
            .map_err(|_| Error::Storage)?;
            // Verify complete persisted evidence after ALL journal/selector SQL.
            schema::validate(&tx)?;
            let selected = selection_on(&tx)?;
            let current = transition_on(&tx, id)?;
            let persisted_receipt = receipt_on(&tx, id, kind)?;
            if selected.mode != new_mode
                || selected.generation != next
                || selected.root != *new_root
                || digest(&current)? != digest(&expected)?
                || persisted_receipt.as_ref().map(digest).transpose()? != Some(digest(&receipt)?)
            {
                return Err(Error::Conflict);
            }
            frozen.verify_candidate(candidate)?;
            if kind == ReceiptKind::Rollback {
                let activation =
                    receipt_on(&tx, id, ReceiptKind::Activation)?.ok_or(Error::Invalid)?;
                frozen.verify_rollback_preservation(&activation)?;
            }
            // Real owner/session/root fence immediately before commit. No waits.
            frozen.revalidate()?;
            tx.commit().map_err(|_| Error::Storage)?;
            Ok(receipt)
        })
    }
}
fn receipt_on(
    connection: &Connection,
    id: Uuid,
    kind: ReceiptKind,
) -> Result<Option<ActivationReceipt>> {
    let row = connection
        .query_row(
            "SELECT id,from_generation,to_generation,selected_root,selected_mode \
             FROM profile_activation_receipts_v1 WHERE operation=?1 AND kind=?2",
            params![id.to_string(), kind.sql()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    row.map(|(receipt, from, to, root, mode)| {
        if to != from.checked_add(1).ok_or(Error::Invalid)? || to > MAX_GENERATION {
            return Err(Error::Invalid);
        }
        let root: crate::RootBinding = super::decode(root)?;
        root.validate()?;
        Ok(ActivationReceipt {
            id: Uuid::parse_str(&receipt).map_err(|_| Error::Invalid)?,
            operation: id,
            kind,
            from_generation: from,
            to_generation: to,
            selected_root: root,
            selected_mode: SelectionMode::from_sql(&mode)?,
        })
    })
    .transpose()
}
