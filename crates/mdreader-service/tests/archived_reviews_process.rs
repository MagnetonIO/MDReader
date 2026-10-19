#![cfg(unix)]
mod support {
    pub mod review_archive;
}
use mdreader_core::SourceRoot;
use mdreader_service::{call, reviews::ReviewRequest, Request};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(directory: &Path) -> Daemon {
    let child = Command::new(env!("CARGO_BIN_EXE_mdreader-service"))
        .arg("--no-model")
        .arg(directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut child = Daemon(child);
    for _ in 0..100 {
        if call(directory, &Request::Ping).is_ok() {
            return child;
        }
        assert!(child.0.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(20));
    }
    panic!("synthetic service did not start")
}
fn reviews(directory: &Path, request: ReviewRequest) -> serde_json::Value {
    call(directory, &Request::Reviews { request }).unwrap()
}
#[test]
fn explicit_archival_read_compare_and_restart_use_real_service_and_preserve_all_originals() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("profile");
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("selected.md"), "# Selected").unwrap();
    let daemon = start(&profile);
    let root = SourceRoot::capture(notes.canonicalize().unwrap()).unwrap();
    let source = call(
        &profile,
        &Request::Admit {
            name: "Selected".into(),
            path: root.path,
            identity: root.identity,
        },
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = call(
        &profile,
        &Request::Read {
            source_id: source.clone(),
            path: "selected.md".into(),
            revision: None,
        },
    )
    .unwrap()["revision"]
        .as_str()
        .unwrap()
        .to_string();
    let (stage, identity) = support::review_archive::archive(temp.path(), &revision);
    let before = std::fs::read(stage.join("reviews.sqlite")).unwrap();
    let selected = reviews(
        &profile,
        ReviewRequest::Select {
            stage: stage.clone(),
            identity: identity.clone(),
        },
    );
    assert!(selected["error"].is_null());
    let handle = selected["result"]["handle"].as_str().unwrap().to_string();
    let scope = selected["result"]["ownerScope"].clone();
    let page = reviews(
        &profile,
        ReviewRequest::List {
            handle: handle.clone(),
            offset: 0,
            limit: 20,
        },
    );
    assert_eq!(
        page["result"]["page"]["records"].as_array().unwrap().len(),
        3
    );
    let record = page["result"]["page"]["records"][1]["recordId"]
        .as_str()
        .unwrap()
        .to_string();
    let compare = || ReviewRequest::Compare {
        handle: handle.clone(),
        record_id: record.clone(),
        source_id: source.clone(),
        path: "selected.md".into(),
        revision: revision.clone(),
    };
    let compared = reviews(&profile, compare());
    assert_eq!(
        compared["result"]["comparison"]["review"]["state"],
        "rejected"
    );
    assert_eq!(
        compared["result"]["comparison"]["recordedCurrent"],
        "matches"
    );
    assert_eq!(
        compared["result"]["comparison"]["reviewedCurrent"],
        "differs"
    );
    assert_eq!(
        compared["result"]["comparison"]["currentPassage"],
        "# Selected"
    );
    let get = reviews(
        &profile,
        ReviewRequest::Get {
            handle: handle.clone(),
            record_id: record.clone(),
        },
    );
    assert_eq!(get["result"]["record"]["state"], "rejected");
    call(
        &profile,
        &Request::SetSourceEnabled {
            source_id: source.clone(),
            enabled: false,
        },
    )
    .unwrap();
    assert_eq!(reviews(&profile, compare())["error"]["kind"], "unavailable");
    drop(daemon);
    let _restarted = start(&profile);
    assert_eq!(
        reviews(
            &profile,
            ReviewRequest::List {
                handle,
                offset: 0,
                limit: 1
            }
        )["error"]["kind"],
        "unavailable"
    );
    let selected = reviews(
        &profile,
        ReviewRequest::Select {
            stage: stage.clone(),
            identity,
        },
    );
    assert_eq!(selected["result"]["ownerScope"], scope);
    assert_eq!(std::fs::read(stage.join("reviews.sqlite")).unwrap(), before);
    assert_eq!(
        std::fs::read(notes.join("selected.md")).unwrap(),
        b"# Selected"
    );
}
