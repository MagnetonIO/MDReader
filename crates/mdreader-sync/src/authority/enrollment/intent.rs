use super::*;
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in super::super) enum Kind {
    Bootstrap,
    Recover,
    Pair,
}
/// Stored only inside the native vault record. No bearer/login tokens or PKCE data.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Intent {
    pub(in super::super) progress: Progress,
    pub(in super::super) kind: Kind,
    pub(in super::super) recovery_grant: Option<String>,
    pub(in super::super) challenge: String,
    pub(in super::super) nonce: String,
    pub(in super::super) expires: u64,
    pub(in super::super) body: String,
}
impl Intent {
    pub(in super::super) fn owner_summary(
        &self,
    ) -> AuthorityResult<super::super::EnrollmentSummary> {
        Ok(super::super::EnrollmentSummary {
            uncertain: self.progress.state == State::Uncertain,
            expired: self.expires <= now()?,
        })
    }
    pub(in super::super) fn digest(&self) -> String {
        crate::blob_digest(self.body.as_bytes())
    }
    pub(in super::super) fn validate(&self, record: &CustodyRecord) -> AuthorityResult<()> {
        if self.progress.history.len() > 8 {
            return Err(AuthorityError::Bounds);
        }
        if record.state == custody::EnrollmentState::Confirmed
            && self.progress.state != State::Confirmed
        {
            return Err(AuthorityError::Custody);
        }
        let mut ids = std::collections::HashSet::new();
        for attempt in &self.progress.history {
            crate::crypto::validate_uuid(&attempt.challenge_id)
                .map_err(|_| AuthorityError::Custody)?;
            crate::crypto::validate_digest(&attempt.digest).map_err(|_| AuthorityError::Custody)?;
            if attempt.nonce.len() != 43
                || crate::crypto::decode_canonical(&attempt.nonce)
                    .map_err(|_| AuthorityError::Custody)?
                    .len()
                    != 32
                || attempt.expires == 0
                || !ids.insert(&attempt.challenge_id)
            {
                return Err(AuthorityError::Custody);
            }
        }
        if (self.progress.state == State::Confirmed) != (self.progress.receipt.is_some()) {
            return Err(AuthorityError::Custody);
        }
        if let Some(receipt) = &self.progress.receipt {
            if record.state != custody::EnrollmentState::Confirmed
                || receipt != &Self::receipt(record)?
            {
                return Err(AuthorityError::Custody);
            }
        }
        let claims = challenge::Claims::parse(&self.challenge)?;
        claims.check_record(record)?;
        if !matches!(
            (self.kind, claims.kind.as_str()),
            (Kind::Bootstrap, "bootstrap") | (Kind::Recover, "recover") | (Kind::Pair, "pair")
        ) {
            return Err(AuthorityError::Custody);
        }
        let expected = match self.kind {
            Kind::Pair if self.recovery_grant.is_none() => {
                super::super::pairing::request_body(record, &self.challenge)?
            }
            Kind::Bootstrap if self.recovery_grant.is_none() => {
                bootstrap::body(record, &self.challenge)?
            }
            Kind::Recover => super::super::recovery::request_body(
                record,
                &self.challenge,
                self.recovery_grant
                    .as_deref()
                    .ok_or(AuthorityError::Custody)?,
            )?,
            _ => return Err(AuthorityError::Custody),
        };
        if claims.nonce != self.nonce || claims.exp != self.expires || self.body != expected {
            return Err(AuthorityError::Custody);
        }
        Ok(())
    }
    pub(in super::super) fn binding(
        &self,
        record: &CustodyRecord,
        epoch: u64,
    ) -> AuthorityResult<EnrollmentLogin> {
        if now()? >= self.expires || self.progress.state == State::Confirmed {
            return Err(AuthorityError::Enrollment);
        }
        Ok(EnrollmentLogin {
            reference: record.reference.clone(),
            registration: record.registration.clone(),
            account: record.scope.account_digest().into(),
            generation: record.generation,
            digest: self.digest(),
            nonce: self.nonce.clone(),
            expires: self.expires,
            epoch,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum State {
    #[default]
    Prepared,
    Uncertain,
    Confirmed,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    digest: String,
    challenge_id: String,
    nonce: String,
    expires: u64,
    uncertain: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    account: String,
    library: String,
    device: String,
    recovery_thumb: Option<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Progress {
    state: State,
    history: Vec<Attempt>,
    receipt: Option<Receipt>,
}
impl Intent {
    fn summary(&self) -> AuthorityResult<Attempt> {
        Ok(Attempt {
            digest: self.digest(),
            challenge_id: challenge::Claims::parse(&self.challenge)?.jti,
            nonce: self.nonce.clone(),
            expires: self.expires,
            uncertain: self.progress.state == State::Uncertain,
        })
    }
    pub(in super::super) fn renew_from(&mut self, old: &Self) -> AuthorityResult<()> {
        if old.expires > now()?
            || old.kind != self.kind
            || old.progress.history.len() >= 8
            || old.progress.state == State::Confirmed
        {
            return Err(AuthorityError::Generation);
        }
        self.progress.history = old.progress.history.clone();
        self.progress.history.push(old.summary()?);
        Ok(())
    }
    pub(in super::super) fn permits_update(&self, next: &Self) -> AuthorityResult<bool> {
        if self.progress.state == State::Prepared && next.progress.state == State::Uncertain {
            let mut expected = self.clone();
            expected.progress.state = State::Uncertain;
            return Ok(&expected == next);
        }
        if next.progress.state != State::Prepared || next.progress.receipt.is_some() {
            return Ok(false);
        }
        let mut expected = next.clone();
        if self.expires <= now()? && expected.renew_from(self).is_ok() {
            return Ok(&expected == next);
        }
        Ok(false)
    }
    fn receipt(record: &CustodyRecord) -> AuthorityResult<Receipt> {
        let custody::Lifecycle::V2 { recovery, .. } = &record.lifecycle else {
            return Err(AuthorityError::Custody);
        };
        if recovery.is_none() && record.intent.as_ref().is_none_or(|i| i.kind != Kind::Pair) {
            return Err(AuthorityError::Custody);
        }
        Ok(Receipt {
            account: record.scope.account_digest().into(),
            library: record.scope.library_id().into(),
            device: record.scope.device_id().into(),
            recovery_thumb: recovery
                .as_ref()
                .map(|raw| signing::SigningKey::load(raw).map(|s| s.device))
                .transpose()?,
        })
    }
    pub(in super::super) fn confirm(&mut self, record: &CustodyRecord) -> AuthorityResult<()> {
        self.progress.receipt = Some(Self::receipt(record)?);
        self.progress.state = State::Confirmed;
        Ok(())
    }
    pub(in super::super) fn permits_confirmation(&self, next: &Self) -> bool {
        let mut expected = next.clone();
        expected.progress = self.progress.clone();
        &expected == self
            && next.progress.state == State::Confirmed
            && next.progress.history == self.progress.history
    }
}
impl NativeAuthorityFactory {
    /// Persist uncertainty before dispatch. Cancellation/timeout cannot forget a sent request.
    pub(in super::super) async fn enrollment_dispatch(
        &self,
        login: &VerifiedLogin,
        record: &mut CustodyRecord,
    ) -> AuthorityResult<()> {
        self.check_controller(&login.controller, login.epoch)?;
        login
            .enrollment
            .as_ref()
            .ok_or(AuthorityError::Identity)?
            .check(record, login.epoch)?;
        let intent = record.intent.as_ref().ok_or(AuthorityError::Custody)?;
        match intent.progress.state {
            State::Confirmed => return Err(AuthorityError::Enrollment),
            State::Uncertain => return Ok(()),
            State::Prepared => {}
        }
        let generation = record.generation;
        record
            .intent
            .as_mut()
            .ok_or(AuthorityError::Custody)?
            .progress
            .state = State::Uncertain;
        record.generation = generation
            .checked_add(1)
            .ok_or(AuthorityError::Generation)?;
        self.vault
            .exchange(
                generation,
                record,
                std::time::Instant::now() + std::time::Duration::from_secs(5),
            )
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        login.identity.check(&self.client.registration)?;
        let current = self
            .vault
            .get(
                &record.reference,
                std::time::Instant::now() + std::time::Duration::from_secs(5),
            )
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        if current.generation != record.generation || !current.same_material(record) {
            return Err(AuthorityError::Generation);
        }
        Ok(())
    }
}
