//! Build identity of the running owner. A reinstall replaces the executable
//! while a detached owner from the old build may keep running; clients compare
//! identities and retire a mismatched owner before starting their own.
use super::{call, protocol_error, Request};
use mdreader_core::Result;
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Set by the launching client so the owner reports the identity that client
/// computed for the same executable.
pub(crate) const ENV: &str = "MDREADER_SERVICE_BUILD";
const MAX_BUILD: usize = 256;
const RETIRE_WAIT: Duration = Duration::from_secs(10);

fn valid(build: &str) -> bool {
    !build.is_empty() && build.len() <= MAX_BUILD && build.bytes().all(|b| b.is_ascii_graphic())
}

/// Identity of an executable file: the file object, its size and its
/// modification time. Replacing the file changes it even when the version
/// number does not.
pub fn executable_build(path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "exe-v1:{}:{}:{}:{modified}",
            metadata.dev(),
            metadata.ino(),
            metadata.len()
        ))
    }
    #[cfg(not(unix))]
    Ok(format!("exe-v1:{}:{modified}", metadata.len()))
}

/// This process's identity, captured at owner startup before the binary on
/// disk can be replaced.
pub(crate) fn own() -> &'static str {
    static OWN: OnceLock<String> = OnceLock::new();
    OWN.get_or_init(|| {
        std::env::var(ENV)
            .ok()
            .filter(|b| valid(b))
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| executable_build(&p).ok())
            })
            .unwrap_or_else(|| "unknown".into())
    })
}

/// Where this process's executable was at owner startup.
pub(crate) fn own_executable() -> Option<&'static str> {
    static OWN: OnceLock<Option<String>> = OnceLock::new();
    OWN.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.canonicalize().ok())
            .and_then(|p| p.to_str().map(str::to_owned))
    })
    .as_deref()
}

pub(crate) fn validate(build: &str) -> Result<()> {
    if valid(build) {
        Ok(())
    } else {
        Err(protocol_error("invalid build identity"))
    }
}

fn reported(ping: &Value) -> Option<&str> {
    ping.get("build").and_then(Value::as_str)
}

/// Last resort for owners from before `Retire` existed: they cannot be asked,
/// so stop the process named by their own endpoint. SQLite commits are
/// atomic, so an interrupted owner loses no committed state.
fn terminate(pid: u64) -> Result<()> {
    if pid <= 1 || pid == u64::from(std::process::id()) {
        return Err(protocol_error("previous reader service is unavailable"));
    }
    let pid = pid.to_string();
    #[cfg(unix)]
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()?;
    #[cfg(windows)]
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid])
        .status()?;
    if !status.success() {
        return Err(protocol_error("previous reader service did not stop"));
    }
    Ok(())
}

/// Ask the running owner of another build to finish its requests and exit,
/// then wait until it no longer answers. A concurrent client may already
/// have replaced it with an owner of `build`, which is kept.
pub(crate) fn retire(directory: &Path, build: &str, ping: &Value) -> Result<()> {
    let asked = call(
        directory,
        &Request::Retire {
            build: build.into(),
        },
    );
    if asked.is_err() {
        match call(directory, &Request::Ping) {
            Ok(current) if reported(&current) == Some(build) => return Ok(()),
            Ok(current) => terminate(
                current
                    .get("pid")
                    .or_else(|| ping.get("pid"))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| protocol_error("previous reader service is unavailable"))?,
            )?,
            Err(_) => return Ok(()),
        }
    }
    let deadline = Instant::now() + RETIRE_WAIT;
    while Instant::now() < deadline {
        match call(directory, &Request::Ping) {
            Ok(current) if reported(&current) == Some(build) => return Ok(()),
            Ok(_) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return Ok(()),
        }
    }
    Err(protocol_error("previous reader service did not stop"))
}

/// Whether a ping reply comes from an owner of `build`.
pub(crate) fn matches(ping: &Value, build: &str) -> bool {
    reported(ping) == Some(build)
}

/// Whether a client running `executable` (identity `build`) should replace
/// the answering owner: the owner predates build identities, runs from the
/// same place, or its executable was replaced or deleted since it started.
/// An owner of another installed copy that is still current is reused, so
/// two copies never keep retiring each other.
pub(crate) fn superseded(ping: &Value, executable: &Path, build: &str) -> bool {
    if matches(ping, build) {
        return false;
    }
    let (Some(theirs), Some(their_build)) = (
        ping.get("executable").and_then(Value::as_str),
        reported(ping),
    ) else {
        return true;
    };
    let same_place = executable.canonicalize().ok().as_deref() == Some(Path::new(theirs));
    let replaced = executable_build(Path::new(theirs)).ok().as_deref() != Some(their_build);
    same_place || replaced
}
