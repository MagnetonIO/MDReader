use super::super::wrapping::WrappingKey;
use super::*;
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Pair,
    Recover,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "kind",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(in super::super) enum Phase {
    BootstrapPending,
    CandidatePending(CandidateKind),
    Enrolled,
}
#[derive(Clone)]
pub(in super::super) enum Lifecycle {
    /// Original bytes are retained, including whitespace/member order. Never upgraded implicitly.
    Legacy(Zeroizing<Vec<u8>>),
    V2 {
        wrapping: Zeroizing<Vec<u8>>,
        recovery: Option<Zeroizing<Vec<u8>>>,
        phase: Phase,
    },
}
impl Lifecycle {
    pub(super) fn bootstrap() -> AuthorityResult<Self> {
        let recovery =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .map_err(|_| AuthorityError::Custody)?;
        Ok(Self::V2 {
            wrapping: WrappingKey::generate()?.secret(),
            recovery: Some(Zeroizing::new(recovery.as_ref().to_vec())),
            phase: Phase::BootstrapPending,
        })
    }
    pub(super) fn candidate(kind: CandidateKind) -> AuthorityResult<Self> {
        Ok(Self::V2 {
            wrapping: WrappingKey::generate()?.secret(),
            recovery: None,
            phase: Phase::CandidatePending(kind),
        })
    }
    pub(super) fn validate(
        &self,
        state: EnrollmentState,
        keys: usize,
        signing: &str,
    ) -> AuthorityResult<()> {
        match self {
            Self::Legacy(_) if keys == 0 => return Err(AuthorityError::Custody),
            Self::Legacy(_) => return Ok(()),
            Self::V2 {
                wrapping,
                recovery,
                phase,
            } => {
                let raw = Zeroizing::new(
                    wrapping
                        .as_slice()
                        .try_into()
                        .map_err(|_| AuthorityError::Custody)?,
                );
                let public = WrappingKey::from_bytes(raw)?.public()?.thumbprint()?;
                if public == signing {
                    return Err(AuthorityError::Custody);
                }
                if let Some(private) = recovery {
                    if private.len() > 1024 {
                        return Err(AuthorityError::Bounds);
                    }
                    let thumb = SigningKey::load(private)?.device;
                    if thumb == signing || thumb == public {
                        return Err(AuthorityError::Custody);
                    }
                }
                let valid = match phase {
                    Phase::BootstrapPending => {
                        state == EnrollmentState::Pending && keys > 0 && recovery.is_some()
                    }
                    Phase::CandidatePending(_) => {
                        state == EnrollmentState::Pending && keys == 0 && recovery.is_none()
                    }
                    Phase::Enrolled => state == EnrollmentState::Confirmed && keys > 0,
                };
                if !valid {
                    return Err(AuthorityError::Custody);
                }
            }
        }
        Ok(())
    }
    pub(in super::super) fn same_keys(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Legacy(a), Self::Legacy(b)) => a == b,
            (
                Self::V2 {
                    wrapping: a,
                    recovery: b,
                    ..
                },
                Self::V2 {
                    wrapping: c,
                    recovery: d,
                    ..
                },
            ) => a == c && b == d,
            _ => false,
        }
    }
    pub(super) fn confirm_bootstrap(&mut self) -> AuthorityResult<()> {
        if let Self::V2 {
            phase: Phase::BootstrapPending,
            ..
        } = self
        {
            if let Self::V2 { phase, .. } = self {
                *phase = Phase::Enrolled;
            }
            Ok(())
        } else {
            Err(AuthorityError::LifecycleGated)
        }
    }
    pub(super) fn same_phase(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Legacy(_), Self::Legacy(_)) => true,
            (Self::V2 { phase: a, .. }, Self::V2 { phase: b, .. }) => a == b,
            _ => false,
        }
    }
    pub(super) fn can_confirm_to(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (
                Self::V2 {
                    phase: Phase::BootstrapPending,
                    ..
                },
                Self::V2 {
                    phase: Phase::Enrolled,
                    ..
                }
            )
        )
    }
}
