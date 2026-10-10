//! Return exports every candidate write before selector CAS. It never restores,
//! truncates or renames the original, and never makes archived grants active.
use super::*;
use mdreader_profile::VerifiedSnapshot;
// The actual desktop fixture is cfg(test); shipping native commands never log
// admission details. Preserve the exact Result and emit only a static phase.
fn at<T>(phase: &'static str, result: Result<T>) -> Result<T> {
    #[cfg(test)]
    if result.is_err() {
        eprintln!("Windows rollback preservation refused at {phase}");
    }
    #[cfg(not(test))]
    let _ = phase;
    result
}
pub(super) struct Proof {
    operation: Uuid,
    generation: u64,
    snapshot: VerifiedSnapshot,
    original: Inventory,
    current: Option<Inventory>,
    fence: super::fence::Fence,
}
impl Proof {
    pub(super) fn verify(&self, receipt: &ActivationReceipt) -> Result<()> {
        if receipt.kind != ReceiptKind::Activation
            || receipt.operation != self.operation
            || receipt.to_generation != self.generation
            || receipt.selected_root != self.snapshot.manifest().original
        {
            return Err(Error::Conflict);
        }
        self.original.revalidate()?;
        if let Some(current) = &self.current {
            current.revalidate()?;
        }
        self.fence.verify()
    }
}
impl Prepared {
    pub(in crate::profile_transition::offline) fn rollback(
        &mut self,
        original: &mut dyn original::Original,
    ) -> Result<()> {
        at("initial_controller_fence", self.verify())?;
        at("initial_original_fence", original.revalidate())?;
        let transition = self
            .control
            .with_library_connection(|db| Journal::attach(db)?.get(self.operation))?;
        if !matches!(transition.state, State::Active | State::RolledBack) {
            return Err(Error::Conflict);
        }
        let receipt = transition.candidate.as_ref().ok_or(Error::Conflict)?;
        if original.binding() != &receipt.original {
            return Err(Error::Conflict);
        }
        if self.candidate.is_none() {
            let root = selection::restore(&receipt.candidate)?;
            let owner = at(
                "candidate_owner_admission",
                ClosedOwner::acquire_retaining(&root.path, &mut self.candidate_handoff),
            )?;
            if owner.binding() != &receipt.candidate {
                return Err(Error::Conflict);
            }
            self.candidate = Some(owner);
        }
        let candidate = self.candidate.as_mut().ok_or(Error::Conflict)?;
        at("candidate_owner_fence", candidate.revalidate())?;
        at(
            "candidate_committed_receipt",
            candidate.verify_candidate(receipt),
        )?;
        let limits = Limits::default();
        let original_inventory = at("original_inventory", original.inventory(&limits))?;
        let original_snapshot =
            mdreader_profile::verify(&self.root.path.join("original"), &receipt.snapshot, &limits)?;
        at(
            "original_archive_fence",
            original_fence::verify(&original_inventory, original_snapshot.manifest()),
        )?;
        let generation = transition.activation_generation.ok_or(Error::Conflict)?;
        let existing = self
            .control
            .with_library_connection(|db| super::rollback_record::read(db))?;
        let record = if let Some(record) = existing {
            record
        } else {
            if transition.state != State::Active {
                return Err(Error::ReconciliationRequired);
            }
            let directory = Uuid::new_v4();
            let path = self.root.path.join(format!("rollback-{directory}"));
            let choices =
                super::super::super::backup_inventory::selections(candidate.path(), &limits)?;
            let inventory = at(
                "candidate_inventory",
                Inventory::capture(candidate.path(), &choices, &limits),
            )?;
            let snapshot = at(
                "candidate_archive_publication",
                mdreader_profile::stage(
                    &mut super::super::super::BorrowedFreeze { frozen: candidate },
                    inventory,
                    &path,
                    &limits,
                ),
            )?;
            let record = super::rollback_record::Record {
                operation: self.operation,
                generation,
                directory,
                snapshot: snapshot.identity().clone(),
            };
            snapshot.revalidate(&limits)?;
            candidate.revalidate()?;
            self.root.verify().map_err(|_| Error::Conflict)?;
            self.control
                .with_library_connection(|db| super::rollback_record::record(db, &record))?;
            record
        };
        if record.operation != self.operation || record.generation != generation {
            return Err(Error::Conflict);
        }
        let path = self
            .root
            .path
            .join(format!("rollback-{}", record.directory));
        let snapshot = at(
            "rollback_archive_verification",
            mdreader_profile::verify(&path, &record.snapshot, &limits),
        )?;
        if snapshot.manifest().original != receipt.candidate {
            return Err(Error::Conflict);
        }
        let current = if transition.state == State::Active {
            let choices =
                super::super::super::backup_inventory::selections(candidate.path(), &limits)?;
            let current = Inventory::capture(candidate.path(), &choices, &limits)?;
            if current.identity() != snapshot.manifest().inventory_sha256 {
                return Err(Error::Conflict);
            }
            Some(current)
        } else {
            None
        };
        let proof = Proof {
            operation: self.operation,
            generation,
            fence: at(
                "rollback_archive_membership",
                super::fence::Fence::capture(&path, &snapshot),
            )?,
            snapshot,
            original: original_inventory,
            current,
        };
        let result = self.control.with_library_connection(|db| {
            let mut owners = Owners {
                original,
                candidate,
                rollback: Some(&proof),
            };
            Journal::attach(db)?.rollback(self.operation, generation, &mut owners)?;
            owners.revalidate()?;
            proof.fence.verify()?;
            self.root.verify().map_err(|_| Error::Conflict)
        });
        if result.is_ok() {
            self.rollback_fence = Some(proof.fence);
        }
        at("final_selector_transaction", result)
    }
    pub(in crate::profile_transition::offline) fn reconcile_rollback_archive(
        &mut self,
    ) -> Result<()> {
        if let Some(fence) = &self.rollback_fence {
            return fence.verify();
        }
        let (transition, receipt, record) =
            self.control.with_library_connection(|db| -> Result<_> {
                Ok((
                    Journal::attach(db)?.get(self.operation)?,
                    marker::read(db)?.1,
                    super::rollback_record::read(db)?.ok_or(Error::Integrity)?,
                ))
            })?;
        if transition.state != State::RolledBack
            || record.operation != self.operation
            || Some(record.generation) != transition.activation_generation
        {
            return Err(Error::Conflict);
        }
        let path = self
            .root
            .path
            .join(format!("rollback-{}", record.directory));
        let snapshot = mdreader_profile::verify(&path, &record.snapshot, &Limits::default())?;
        if snapshot.manifest().original != receipt.candidate {
            return Err(Error::Conflict);
        }
        self.rollback_fence = Some(super::fence::Fence::capture(&path, &snapshot)?);
        self.root.verify().map_err(|_| Error::Conflict)
    }
}
