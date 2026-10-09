//! One native selection retained before modern owner handoff. UI Retry never
//! repicks the folder or regenerates a dispatched service operation.
use super::super::shutdown_handoff::OwnerHandoffs;
use super::*;

#[derive(Clone, Copy)]
pub(super) enum Mode {
    Reader,
    Workspace,
}
pub(super) struct Opening {
    root: SourceRoot,
    mode: Mode,
    handoffs: OwnerHandoffs,
}
impl Opening {
    pub(super) fn issued(&self) -> bool {
        self.handoffs.issued()
    }
    pub(super) fn failure_kind(&self) -> Option<&'static str> {
        self.handoffs.failure_kind()
    }
}
pub(super) fn pending(root: SourceRoot, mode: Mode) -> Pending {
    Pending {
        id: Uuid::new_v4(),
        rollback_pending: false,
        original: None,
        legacy: None,
        archive: None,
        workspace: None,
        opening: Some(Opening {
            root,
            mode,
            handoffs: Default::default(),
        }),
    }
}
fn open(active: &mut Pending) -> Result<MigrationReply> {
    let opening = active.opening.as_mut().ok_or(Error::Conflict)?;
    opening.root.verify().map_err(|_| Error::Conflict)?;
    let mut result = match opening.mode {
        Mode::Reader => {
            let owner = opening.handoffs.acquire(&opening.root.path)?;
            super::super::super::backup_inventory::selections(owner.path(), &Default::default())?;
            let store = ReaderStore::open_durable_readonly(&owner.path().join("reader-v1"))
                .map_err(|_| Error::Unsupported)?;
            let documents = conversion::documents(&store)?
                .into_iter()
                .map(|document| Document {
                    document_id: document.id,
                    name: document.name,
                    revision: document.revision,
                    bytes: document.bytes,
                    pinned: document.pinned,
                    line: document.line,
                })
                .collect();
            opening.root.verify().map_err(|_| Error::Conflict)?;
            active.original = Some(owner);
            let mut result = reply("preview", Some(active.id));
            result.documents = documents;
            result
        }
        Mode::Workspace => {
            let mut workspace =
                Workspace::resume_retaining(&opening.root.path, &mut opening.handoffs)?;
            let state = workspace.status()?;
            let imported = workspace.imported()?;
            opening.root.verify().map_err(|_| Error::Conflict)?;
            active.workspace = Some(workspace);
            let mut result = reply(state, Some(active.id));
            result.imported = imported;
            result
        }
    };
    result.origin = Some("reader");
    active.opening = None;
    Ok(result)
}
pub(super) fn finish(active: &mut Pending) -> Result<MigrationReply> {
    match open(active) {
        Ok(result) => Ok(result),
        Err(Error::NotQuiescent | Error::ReconciliationRequired) => Ok(active.waiting()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_handoff_retains_lost_begin_identity_and_never_shutdowns_quarantined_owner() {
        use std::io::Write;
        use std::time::Instant;
        use std::{
            io::{BufRead, BufReader},
            net::TcpListener,
            time::Duration,
        };
        let temporary = tempfile::tempdir().unwrap();
        let root = SourceRoot::capture(temporary.path().canonicalize().unwrap()).unwrap();
        drop(ReaderStore::open(&root.path.join("reader-v1")).unwrap());
        root.create_new_relative(Path::new(".mdreader-json-owner.lock"))
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let boot = Uuid::new_v4();
        let endpoint = serde_json::json!({"protocol":1,"port":port,"token":"a".repeat(64),"pid":1});
        root.create_new_relative(Path::new("reader-v1/reader-endpoint.json"))
            .unwrap()
            .write_all(&serde_json::to_vec(&endpoint).unwrap())
            .unwrap();
        let retained = root
            .open_lock_relative(Path::new("reader-v1/reader.lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&retained).unwrap();
        let contender = root
            .open_lock_relative(Path::new("reader-v1/reader.lock"))
            .unwrap();
        let server = std::thread::spawn(move || {
            let mut first_scope = None;
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut input = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut input)
                    .unwrap();
                let envelope: serde_json::Value = serde_json::from_str(&input).unwrap();
                let request = &envelope["request"]["params"]["request"];
                assert_eq!(envelope["request"]["method"], "preservation");
                let result = if index == 0 {
                    assert_eq!(request["operation"], "inspect");
                    serde_json::json!({"state":"hello","boot":boot})
                } else {
                    assert_eq!(request["operation"], "begin");
                    if index == 1 {
                        first_scope = Some(request["scope"].clone());
                        // The first dispatched Begin loses its reply. It must not
                        // be replaced by a fresh operation on the next UI Retry.
                        continue;
                    }
                    assert_eq!(Some(&request["scope"]), first_scope.as_ref());
                    serde_json::json!({"state":"status","status":{
                    "phase":"quarantined","remaining_ms":0,"lease":null,"failure":"deadline"}})
                };
                writeln!(
                    stream,
                    "{}",
                    serde_json::json!({"result":result,"error":null})
                )
                .unwrap();
            }
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_millis(300);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok(_) => {
                        panic!("failed/quarantined Begin must not issue Revalidate or Shutdown")
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("unexpected synthetic listener error: {error}"),
                }
            }
        });
        let mut active = pending(root, Mode::Reader);
        let operation = active.id;
        for _ in 0..2 {
            let reply = finish(&mut active).unwrap();
            assert_eq!(reply.state, "service_handoff_pending");
            assert_eq!(reply.operation_id, Some(operation.to_string()));
            assert!(active.handoff_issued());
            assert_eq!(
                active.handoff_failure(),
                Some("service_handoff_unconfirmed")
            );
            // The production Done guard cannot drop this issued operation. A later
            // explicit Preview/Resume reattaches this waiting identity, without IO.
            assert!(matches!(
                active.ensure_closeable(),
                Err(Error::ReconciliationRequired)
            ));
            let reattached = active.waiting();
            assert_eq!(reattached.operation_id, Some(operation.to_string()));
            assert_eq!(reattached.state, "service_handoff_pending");
            assert!(fs2::FileExt::try_lock_exclusive(&contender).is_err());
        }
        // An archive rollback has its own continuation; reattachment must
        // never turn an issued return operation into ordinary preview.
        active.rollback_pending = true;
        assert_eq!(active.waiting().state, "rollback_handoff_pending");
        assert_eq!(active.waiting().operation_id, Some(operation.to_string()));
        assert!(matches!(
            active.ensure_closeable(),
            Err(Error::ReconciliationRequired)
        ));
        server.join().unwrap();
        fs2::FileExt::unlock(&retained).unwrap();
    }
}
