use mdreader_legacy_export::{reviews::ArchiveIdentity, Error, Result};
use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

pub(super) enum Command {
    Export {
        spec: PathBuf,
        destination: PathBuf,
    },
    Verify {
        archive: PathBuf,
    },
    StageReviews {
        archive: PathBuf,
        identity: ArchiveIdentity,
        destination: PathBuf,
    },
    ListReviews {
        stage: PathBuf,
        identity: ArchiveIdentity,
        offset: u64,
        limit: usize,
    },
    GetReview {
        stage: PathBuf,
        identity: ArchiveIdentity,
        record_id: String,
    },
}
fn ascii(value: &OsStr, bound: usize) -> Result<&str> {
    value
        .to_str()
        .filter(|v| !v.is_empty() && v.len() <= bound && v.is_ascii())
        .ok_or(Error::Invalid)
}
fn identity(id: &OsStr, hash: &OsStr) -> Result<ArchiveIdentity> {
    let export_id = uuid::Uuid::parse_str(ascii(id, 36)?).map_err(|_| Error::Invalid)?;
    if export_id.get_version_num() != 4 {
        return Err(Error::Invalid);
    }
    Ok(ArchiveIdentity {
        export_id,
        manifest_sha256: hex_id(hash)?,
    })
}
fn hex_id(value: &OsStr) -> Result<String> {
    let value = ascii(value, 64)?;
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid);
    }
    Ok(value.to_ascii_lowercase())
}
fn number(value: &OsStr) -> Result<u64> {
    let value = ascii(value, 20)?;
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid);
    }
    value.parse().map_err(|_| Error::Invalid)
}
pub(super) fn parse(args: impl Iterator<Item = OsString>) -> Result<Command> {
    // Keep argument collection bounded even for incorrect invocation.
    let args = args.take(8).collect::<Vec<_>>();
    match args.as_slice() {
        [operation, spec, destination] if operation == "export" => Ok(Command::Export {
            spec: spec.into(),
            destination: destination.into(),
        }),
        [operation, archive] if operation == "verify" => Ok(Command::Verify {
            archive: archive.into(),
        }),
        [operation, archive, id, hash, destination] if operation == "stage-reviews" => {
            Ok(Command::StageReviews {
                archive: archive.into(),
                identity: identity(id, hash)?,
                destination: destination.into(),
            })
        }
        [operation, stage, id, hash, offset, limit] if operation == "list-reviews" => {
            let offset = number(offset)?;
            let limit = number(limit)?;
            if offset > 1_000_000 || !(1..=100).contains(&limit) {
                return Err(Error::Invalid);
            }
            Ok(Command::ListReviews {
                stage: stage.into(),
                identity: identity(id, hash)?,
                offset,
                limit: limit as usize,
            })
        }
        [operation, stage, id, hash, record] if operation == "get-review" => {
            Ok(Command::GetReview {
                stage: stage.into(),
                identity: identity(id, hash)?,
                record_id: hex_id(record)?,
            })
        }
        _ => Err(Error::Invalid),
    }
}
