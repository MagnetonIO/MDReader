use super::super::{
    custody::{Lifecycle, Phase},
    signing::SigningKey,
    wrapping,
};
use super::*;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub(in super::super) fn public_material(
    record: &CustodyRecord,
) -> AuthorityResult<(serde_json::Value, serde_json::Value, serde_json::Value)> {
    let Lifecycle::V2 {
        wrapping: raw,
        recovery,
        ..
    } = &record.lifecycle
    else {
        return Err(AuthorityError::LifecycleGated);
    };
    let wrapping = wrapping::WrappingKey::from_bytes(Zeroizing::new(
        raw.as_slice()
            .try_into()
            .map_err(|_| AuthorityError::Custody)?,
    ))?;
    Ok((
        SigningKey::load(&record.pkcs8)?.jwk,
        serde_json::to_value(wrapping.public()?).map_err(|_| AuthorityError::Custody)?,
        recovery
            .as_ref()
            .map(|raw| SigningKey::load(raw).map(|s| s.jwk))
            .transpose()?
            .unwrap_or(serde_json::Value::Null),
    ))
}
pub(in super::super) fn body(record: &CustodyRecord, challenge: &str) -> AuthorityResult<String> {
    let (signing, wrapping, recovery) = public_material(record)?;
    serde_json::to_string(&serde_json::json!({"challenge":challenge,"signingJwk":signing,"wrappingJwk":wrapping,"recoveryJwk":recovery}))
        .map_err(|_| AuthorityError::Custody)
}
impl NativeAuthorityFactory {
    /// Persist a challenge and exact body before any enrollment mutation request.
    /// Requires an already verified account; a second PKCE login binds its nonce.
    pub async fn prepare_bootstrap(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<EnrollmentLogin> {
        self.check_controller(&login.controller, login.epoch)?;
        let mut record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        if !matches!(
            record.lifecycle,
            Lifecycle::V2 {
                phase: Phase::BootstrapPending,
                ..
            }
        ) {
            return Err(AuthorityError::LifecycleGated);
        }
        if let Some(intent) = &record.intent {
            return intent.binding(&record, login.epoch);
        }
        let (signing, wrapping, recovery) = public_material(&record)?;
        let request = Zeroizing::new(serde_json::to_vec(&serde_json::json!({"kind":"bootstrap","libraryId":library,"signingJwk":signing,"wrappingJwk":wrapping,"recoveryJwk":recovery}))
            .map_err(|_| AuthorityError::Custody)?);
        let url = self
            .client
            .registration
            .sync
            .join("/v1/enrollment/challenge")
            .map_err(|_| AuthorityError::Configuration)?;
        let bytes = self
            .client
            .http
            .send(
                reqwest::Method::POST,
                url,
                request,
                None,
                "application/json",
                8192,
            )
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Response {
            challenge: String,
            nonce: String,
            expires: u64,
        }
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Enrollment)?;
        let claims = challenge::Claims::parse(&response.challenge)?;
        claims.check_record(&record)?;
        claims.check_fresh(&self.client.registration.api_audience)?;
        if claims.nonce != response.nonce || claims.exp != response.expires {
            return Err(AuthorityError::Scope);
        }
        let generation = record.generation;
        record.intent = Some(Intent {
            progress: Default::default(),
            kind: Kind::Bootstrap,
            recovery_grant: None,
            body: body(&record, &response.challenge)?,
            challenge: response.challenge,
            nonce: response.nonce,
            expires: response.expires,
        });
        record.generation = generation
            .checked_add(1)
            .ok_or(AuthorityError::Generation)?;
        self.vault
            .exchange(generation, &record, Instant::now() + Duration::from_secs(5))
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        let current = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        if current.generation != record.generation || !current.same_material(&record) {
            return Err(AuthorityError::Generation);
        }
        record
            .intent
            .as_ref()
            .ok_or(AuthorityError::Custody)?
            .binding(&record, login.epoch)
    }
    /// Send immutable persisted bytes with a verified challenge-nonce token pair.
    /// An uncertain reply leaves all custody intact; fresh ordinary login + resume
    /// reconciles an already enrolled device without replaying an expired challenge.
    pub async fn bootstrap(
        &self,
        login: VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<NativeAuthority> {
        self.check_controller(&login.controller, login.epoch)?;
        let mut record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        login
            .enrollment
            .as_ref()
            .ok_or(AuthorityError::Identity)?
            .check(&record, login.epoch)?;
        self.enrollment_dispatch(&login, &mut record).await?;
        let intent = record.intent.as_ref().ok_or(AuthorityError::Enrollment)?;
        if intent.kind != Kind::Bootstrap {
            return Err(AuthorityError::LifecycleGated);
        }
        let url = self
            .client
            .registration
            .sync
            .join("/v1/enrollment/bootstrap")
            .map_err(|_| AuthorityError::Configuration)?;
        let auth = SigningKey::load(&record.pkcs8)?.authorize(
            &login.identity,
            "POST",
            url.as_str(),
            intent.body.as_bytes(),
        )?;
        let response = self
            .client
            .http
            .send(
                reqwest::Method::POST,
                url,
                Zeroizing::new(intent.body.as_bytes().to_vec()),
                Some(auth),
                "application/json",
                4096,
            )
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Receipt {
            account: String,
            library_id: String,
            device: String,
            enrolled: bool,
        }
        let receipt: Receipt =
            serde_json::from_slice(&response).map_err(|_| AuthorityError::Enrollment)?;
        if !receipt.enrolled
            || receipt.account != record.scope.account_digest()
            || receipt.library_id != library
            || receipt.device != record.scope.device_id()
        {
            return Err(AuthorityError::Scope);
        }
        self.resume(login, reference, library).await
    }
}
