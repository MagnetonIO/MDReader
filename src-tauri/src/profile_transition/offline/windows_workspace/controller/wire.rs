//! Bounded one-shot native transport. Tokens never enter renderer replies/logs.
use super::*;
use serde::de::DeserializeOwned;
use std::io::{Read, Write};
use std::net::TcpStream;
const MAX_FRAME: usize = 4096;
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Status,
    Rollback,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub(super) version: u8,
    pub(super) operation: Uuid,
    pub(super) token: String,
    pub(super) action: Action,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Active,
    CloseCandidateRequired,
    Returned,
    Unconfirmed,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reply {
    pub(super) version: u8,
    pub(super) operation: Uuid,
    pub(super) status: Status,
}
pub(super) fn configure(stream: &TcpStream) -> Result<()> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|_| Error::Storage)?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|_| Error::Storage)
}
pub(super) fn read<T: DeserializeOwned>(stream: &mut TcpStream) -> Result<T> {
    let mut length = [0; 4];
    stream
        .read_exact(&mut length)
        .map_err(|_| Error::ReconciliationRequired)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(Error::Bounds);
    }
    let mut bytes = vec![0; length];
    stream
        .read_exact(&mut bytes)
        .map_err(|_| Error::ReconciliationRequired)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)
}
pub(super) fn write<T: Serialize>(stream: &mut TcpStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Invalid)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err(Error::Bounds);
    }
    stream
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .map_err(|_| Error::ReconciliationRequired)?;
    stream
        .write_all(&bytes)
        .map_err(|_| Error::ReconciliationRequired)
}
pub(super) fn token_matches(actual: &str, expected: &str) -> bool {
    actual.len() == expected.len()
        && actual
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}
