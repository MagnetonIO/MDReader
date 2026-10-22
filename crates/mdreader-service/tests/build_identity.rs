//! A client of another build retires the running owner instead of reusing
//! it (the reinstall case), while two current copies never retire each other.
use mdreader_service::{call, ensure_running, ensure_running_as, executable_build, Request};
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};
use tempfile::tempdir;

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn executable() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_mdreader-service"))
}

fn no_model() -> Vec<std::ffi::OsString> {
    vec!["--no-model".into()]
}

fn ping(directory: &Path) -> serde_json::Value {
    call(directory, &Request::Ping).unwrap()
}

fn exits_within(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn a_different_build_retires_the_running_owner_and_starts_its_own() {
    let data = tempdir().unwrap();
    let mut old = Daemon(
        ensure_running_as(data.path(), executable(), &no_model(), "build-a")
            .unwrap()
            .expect("first client launches the owner"),
    );
    assert_eq!(ping(data.path())["build"], "build-a");
    // The same build reuses the owner and may not retire it.
    assert!(
        ensure_running_as(data.path(), executable(), &no_model(), "build-a")
            .unwrap()
            .is_none()
    );
    assert!(call(
        data.path(),
        &Request::Retire {
            build: "build-a".into()
        }
    )
    .is_err());
    assert!(!exits_within(&mut old.0, Duration::from_millis(200)));

    let new = Daemon(
        ensure_running_as(data.path(), executable(), &no_model(), "build-b")
            .unwrap()
            .expect("a new build replaces the owner"),
    );
    assert!(
        exits_within(&mut old.0, Duration::from_secs(10)),
        "old owner exited"
    );
    let current = ping(data.path());
    assert_eq!(current["build"], "build-b");
    assert_eq!(current["pid"], new.0.id());
}

#[test]
fn another_current_copy_reuses_the_owner_instead_of_retiring_it() {
    let data = tempdir().unwrap();
    let bin = tempdir().unwrap();
    let copy = bin.path().join("mdreader-service-copy");
    std::fs::copy(executable(), &copy).unwrap();
    let mut owner = Daemon(
        ensure_running(data.path(), executable(), &no_model())
            .unwrap()
            .expect("launch"),
    );
    assert_eq!(
        ping(data.path())["build"],
        executable_build(executable()).unwrap()
    );
    assert_ne!(
        executable_build(&copy).unwrap(),
        executable_build(executable()).unwrap()
    );
    assert!(ensure_running(data.path(), &copy, &no_model())
        .unwrap()
        .is_none());
    assert!(!exits_within(&mut owner.0, Duration::from_millis(200)));
}

#[test]
fn a_reinstalled_executable_replaces_the_owner_started_from_the_old_file() {
    let data = tempdir().unwrap();
    let bin = tempdir().unwrap();
    let installed = bin.path().join("mdreader-service");
    std::fs::copy(executable(), &installed).unwrap();
    let mut old = Daemon(
        ensure_running(data.path(), &installed, &no_model())
            .unwrap()
            .expect("launch"),
    );
    // Reinstall: the old file is deleted and a new one takes its path.
    std::fs::remove_file(&installed).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    std::fs::copy(executable(), &installed).unwrap();
    let new = Daemon(
        ensure_running(data.path(), &installed, &no_model())
            .unwrap()
            .expect("the reinstalled build starts its own owner"),
    );
    assert!(exits_within(&mut old.0, Duration::from_secs(10)));
    let current = ping(data.path());
    assert_eq!(current["build"], executable_build(&installed).unwrap());
    assert_eq!(current["pid"], new.0.id());
}
