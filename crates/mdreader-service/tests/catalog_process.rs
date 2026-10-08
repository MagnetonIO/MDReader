#![cfg(unix)]
use mdreader_core::SourceRoot;
use mdreader_service::{Request, call, catalog::CatalogRequest};
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
fn start(profile: &Path) -> Daemon {
    let mut child = Daemon(
        Command::new(env!("CARGO_BIN_EXE_mdreader-service"))
            .arg("--no-model")
            .arg(profile)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        if call(profile, &Request::Ping).is_ok() {
            return child;
        }
        assert!(child.0.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(20));
    }
    panic!("synthetic service unavailable")
}
#[test]
fn actual_service_catalog_opens_exact_observation_and_restart_fences_volatile_cursor() {
    let t = tempfile::tempdir().unwrap();
    let profile = t.path().join("profile");
    let notes = t.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    for n in 0..3 {
        std::fs::write(notes.join(format!("{n}.md")), format!("# Exact {n}")).unwrap();
    }
    let daemon = start(&profile);
    let root = SourceRoot::capture(notes).unwrap();
    let id = call(
        &profile,
        &Request::Admit {
            name: "Synthetic".into(),
            path: root.path,
            identity: root.identity,
        },
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &profile,
        &Request::Index {
            source_id: id.clone(),
        },
    )
    .unwrap();
    let page = call(
        &profile,
        &Request::Catalog {
            request: CatalogRequest::Page {
                catalog_id: format!("local:{id}"),
                cursor: None,
                limit: 2,
            },
        },
    )
    .unwrap();
    assert!(page["error"].is_null());
    let documents = page["result"]["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 2);
    let evidence = &documents[0]["open"];
    let document = call(
        &profile,
        &Request::Read {
            source_id: evidence["sourceId"].as_str().unwrap().into(),
            path: evidence["path"].as_str().unwrap().into(),
            revision: Some(evidence["revision"].as_str().unwrap().into()),
        },
    )
    .unwrap();
    assert_eq!(document["markdown"], "# Exact 0");
    let cursor = page["result"]["nextCursor"].as_str().unwrap().to_owned();
    drop(daemon);
    let _restart = start(&profile);
    let stale = call(
        &profile,
        &Request::Catalog {
            request: CatalogRequest::Page {
                catalog_id: format!("local:{id}"),
                cursor: Some(cursor),
                limit: 2,
            },
        },
    )
    .unwrap();
    assert_eq!(stale["error"], "stale");
    let sources = call(&profile, &Request::ListSources).unwrap();
    assert_eq!(sources.as_array().unwrap().len(), 1);
}
