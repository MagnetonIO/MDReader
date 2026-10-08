//! Argument/JSON transport only; archival rules and storage stay in the library.
mod args;
use args::Command;
use mdreader_legacy_export::{
    export, read_spec,
    reviews::{stage_reviews, ReviewArchive},
    verify, Error, Limits, Manifest, Result,
};
use serde::Serialize;
use std::{
    ffi::OsString,
    io::{self, Write},
};

const RECEIPT_BOUND: usize = 4096;
const READ_BOUND: usize = 4 * 1024 * 1024 + RECEIPT_BOUND;
pub(super) const USAGE: &str = concat!(
    "usage: mdreader-legacy-export\n",
    "  export ABS_SPEC_JSON ABS_DESTINATION\n",
    "  verify ABS_ARCHIVE\n",
    "  stage-reviews ABS_ARCHIVE EXPORT_UUID MANIFEST_SHA256 ABS_NEW_DEST\n",
    "  list-reviews ABS_STAGE EXPORT_UUID MANIFEST_SHA256 OFFSET LIMIT\n",
    "  get-review ABS_STAGE EXPORT_UUID MANIFEST_SHA256 RECORD_ID"
);
struct BoundedOutput {
    bytes: Vec<u8>,
    bound: usize,
    exceeded: bool,
}
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.bound.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode(value: &impl Serialize, bound: usize) -> Result<Vec<u8>> {
    let mut writer = BoundedOutput {
        bytes: Vec::new(),
        bound: bound - 1,
        exceeded: false,
    };
    let result = serde_json::to_writer(&mut writer, value);
    if writer.exceeded {
        return Err(Error::Bounds);
    }
    result?;
    writer.bytes.push(b'\n');
    Ok(writer.bytes)
}
fn receipt(manifest: Manifest) -> Result<Vec<u8>> {
    encode(
        &serde_json::json!({"format":manifest.format,"version":manifest.version,"exportId":manifest.export_id,"artifacts":manifest.artifacts.len(),"members":manifest.members.len()}),
        RECEIPT_BOUND,
    )
}
pub(super) fn run(args: impl Iterator<Item = OsString>) -> Result<Vec<u8>> {
    let limits = Limits::default();
    match args::parse(args)? {
        Command::Export { spec, destination } => {
            receipt(export(&read_spec(&spec, &limits)?, &destination, &limits)?)
        }
        Command::Verify { archive } => receipt(verify(&archive, &limits)?),
        Command::StageReviews {
            archive,
            identity,
            destination,
        } => {
            let receipt = stage_reviews(&archive, &identity, &destination, &limits)?;
            encode(
                &serde_json::json!({"operation":"stage_reviews","scope":"archival_only","receipt":receipt}),
                RECEIPT_BOUND,
            )
        }
        Command::ListReviews {
            stage,
            identity,
            offset,
            limit,
        } => {
            let archive = ReviewArchive::open(&stage, &identity, &limits)?;
            let page = archive.list(offset, limit)?;
            encode(
                &serde_json::json!({"operation":"list_reviews","scope":"archival_only","archive":identity,"currentDocumentEvidence":"unavailable","sourceBinding":"not_performed","page":page}),
                READ_BOUND,
            )
        }
        Command::GetReview {
            stage,
            identity,
            record_id,
        } => {
            let archive = ReviewArchive::open(&stage, &identity, &limits)?;
            let record = archive.get(&record_id)?;
            encode(
                &serde_json::json!({"operation":"get_review","scope":"archival_only","archive":identity,"currentDocumentEvidence":"unavailable","sourceBinding":"not_performed","record":record}),
                READ_BOUND,
            )
        }
    }
}
