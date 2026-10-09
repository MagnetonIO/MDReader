//! Retain the original OS/process/file capability in one live native controller.
//! TCP carries commands, never handles or a reconstructed frozen-owner assertion.
use super::*;
use mdreader_profile::legacy_windows::{ClosedOriginal, RetainedLaunch};
use std::{
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tauri::AppHandle;
mod wire;
use wire::{Action, Reply, Request, Status};
pub(in crate::profile_transition::offline) struct LiveController {
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
    root: SourceRoot,
}
impl LiveController {
    pub(in crate::profile_transition::offline) fn verify_live(&self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        if self.worker.is_finished() {
            return Err(Error::ReconciliationRequired);
        }
        Ok(())
    }
}
impl Drop for LiveController {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
struct Worker {
    original: Option<ClosedOriginal>,
    launch: RetainedLaunch,
    child: Option<std::process::Child>,
    prepared: Prepared,
    record: super::controller_record::Record,
    // The exact private byte lock remains held for the whole endpoint lifetime.
    _lock: std::fs::File,
    app: AppHandle,
}
/// Transfers the actual frozen capability by Rust ownership before launching the
/// candidate. Failure remains explicit; a future controller must reobserve the
/// exact original app, not infer closure from a stale discovery row or PID.
pub(in crate::profile_transition::offline) fn start(
    app: AppHandle,
    mut original: ClosedOriginal,
    mut prepared: Prepared,
) -> Result<LiveController> {
    original.binding().validate()?;
    if prepared.state()? != "launch_requested" {
        return Err(Error::Conflict);
    }
    let receipt = prepared
        .control
        .with_library_connection(|db| marker::read(db).map(|r| r.1))?;
    if original.binding() != &receipt.original {
        return Err(Error::Conflict);
    }
    original.revalidate()?;
    let snapshot = mdreader_profile::verify(
        &prepared.root.path.join("original"),
        &receipt.snapshot,
        &Limits::default(),
    )?;
    original_fence::verify(&original.inventory()?, snapshot.manifest())?;
    let private = mdreader_capture::PrivateDirectory::open(&prepared.root.path)
        .map_err(|_| Error::Conflict)?;
    let lock = private
        .open_lock(Path::new("controller.lock"))
        .map_err(|_| Error::Storage)?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::NotQuiescent)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| Error::Storage)?;
    listener.set_nonblocking(true).map_err(|_| Error::Storage)?;
    let generation = prepared
        .control
        .with_library_connection(|db| Journal::attach(db)?.get(prepared.operation))?
        .activation_generation
        .ok_or(Error::Conflict)?;
    let record = super::controller_record::Record {
        operation: prepared.operation,
        generation,
        epoch: Uuid::new_v4(),
        root: mdreader_profile::RootBinding::capture_private(&prepared.root.path)?,
        port: listener.local_addr().map_err(|_| Error::Storage)?.port(),
        token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
    };
    prepared
        .control
        .with_library_connection(|db| super::controller_record::publish(db, &record))?;
    private.verify().map_err(|_| Error::Conflict)?;
    let launch = original.launch_record()?.retain()?;
    let root = prepared.root.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let mut worker = Worker {
        original: Some(original),
        launch,
        child: None,
        prepared,
        record,
        _lock: lock,
        app,
    };
    let worker = thread::Builder::new()
        .name("mdreader-migration-controller".into())
        .spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                        let _ = worker.connection(&mut stream);
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25))
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|_| Error::ReconciliationRequired)?;
    Ok(LiveController { stop, worker, root })
}
impl Worker {
    fn return_original(&mut self) -> Result<()> {
        if self.child.is_some() {
            return Ok(());
        }
        if self.prepared.state()? != "rolled_back" {
            self.prepared.rollback(
                self.original
                    .as_mut()
                    .ok_or(Error::ReconciliationRequired)?,
            )?;
        }
        crate::reader_profile::returned_to_original(
            &self.app,
            &self.prepared.root.path.join("candidate"),
        )?;
        self.launch.verify()?;
        // Unknown dispatch from another lifetime is never retried. A definite
        // CreateProcess failure is recorded by this exact lock-holding worker.
        self.prepared.control.with_library_connection(|db| {
            super::controller_dispatch::begin(db, self.record.operation)
        })?;
        drop(self.original.take());
        match self.launch.launch() {
            Ok(child) => {
                self.child = Some(child);
                self.prepared.control.with_library_connection(|db| {
                    super::controller_dispatch::finish(db, self.record.operation, true)
                })?;
                Ok(())
            }
            Err(error) => {
                self.prepared.control.with_library_connection(|db| {
                    super::controller_dispatch::finish(db, self.record.operation, false)
                })?;
                Err(error)
            }
        }
    }
    fn connection(&mut self, stream: &mut TcpStream) -> Result<()> {
        wire::configure(stream)?;
        let request: Request = wire::read(stream)?;
        if request.version != 1
            || request.operation != self.record.operation
            || !wire::token_matches(&request.token, &self.record.token)
        {
            return Err(Error::Conflict);
        }
        self.prepared.verify()?;
        if let Some(original) = self.original.as_mut() {
            original.revalidate()?;
        }
        let status = match request.action {
            Action::Status => match self.prepared.state()? {
                "launch_requested" => Status::Active,
                "rolled_back" if self.child.is_some() => Status::Returned,
                _ => Status::Unconfirmed,
            },
            Action::Rollback => match self.return_original() {
                Ok(()) => Status::Returned,
                Err(Error::NotQuiescent | Error::ReconciliationRequired) => {
                    Status::CloseCandidateRequired
                }
                Err(_) => Status::Unconfirmed,
            },
        };
        wire::write(
            stream,
            &Reply {
                version: 1,
                operation: self.record.operation,
                status,
            },
        )
    }
}
/// Exact private selector discovery followed by a live authenticated command.
/// The stale endpoint case is unconfirmed and never reacquires old owner guards.
pub(in crate::profile_transition::offline) fn request(
    path: &Path,
    rollback: bool,
) -> Result<&'static str> {
    let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
    let binding = mdreader_profile::RootBinding::capture_private(path)?;
    let mut store = ReaderStore::open_durable_readonly(&root.path.join("control"))
        .map_err(|_| Error::Conflict)?;
    let record = store.with_library_connection(|db| -> Result<_> {
        let tx = db.transaction().map_err(|_| Error::Storage)?;
        let record = super::controller_record::read(&tx)?.ok_or(Error::ReconciliationRequired)?;
        let receipt = marker::read(&tx)?.1;
        let transition = observe(&tx, receipt.operation)?.0;
        if record.root != binding
            || record.operation != receipt.operation
            || transition.activation_generation != Some(record.generation)
            || !matches!(transition.state, State::Active | State::RolledBack)
            || super::legacy_record::read(&tx)?.is_none_or(|legacy| {
                legacy.operation != record.operation || legacy.launch.profile() != &receipt.original
            })
        {
            return Err(Error::Conflict);
        }
        tx.commit().map_err(|_| Error::Storage)?;
        Ok(record)
    })?;
    root.verify().map_err(|_| Error::Conflict)?;
    let mut stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, record.port)),
        Duration::from_secs(2),
    )
    .map_err(|_| Error::ReconciliationRequired)?;
    wire::configure(&stream)?;
    wire::write(
        &mut stream,
        &Request {
            version: 1,
            operation: record.operation,
            token: record.token,
            action: if rollback {
                Action::Rollback
            } else {
                Action::Status
            },
        },
    )?;
    let reply: Reply = wire::read(&mut stream)?;
    root.verify().map_err(|_| Error::Conflict)?;
    if reply.version != 1 || reply.operation != record.operation {
        return Err(Error::Conflict);
    }
    match reply.status {
        Status::Active => Ok("launch_requested"),
        Status::CloseCandidateRequired => Ok("rollback_handoff_pending"),
        Status::Returned => Ok("rolled_back"),
        Status::Unconfirmed => Err(Error::ReconciliationRequired),
    }
}

/// Native discovery for the selected candidate's live original controller.
/// A failed live response requires explicit original reobservation, never PID
/// inference, owner-lock bypass or an automatically reconstructed capability.
pub(in crate::profile_transition::offline) fn resume(
    path: &Path,
) -> Result<(Uuid, SourceRoot, u32)> {
    request(path, false)?;
    let (native, original, imported, _) = observation(path)?;
    Ok((native, original, imported))
}

/// Metadata for an explicit reobserve flow, not a reconstructed owner. Only
/// Active candidates can regain a controller; unknown return dispatch remains
/// fail closed and cannot be replayed from a fresh lifetime.
pub(in crate::profile_transition::offline) fn observation(
    path: &Path,
) -> Result<(
    Uuid,
    SourceRoot,
    u32,
    mdreader_profile::legacy_windows::LaunchRecord,
)> {
    let root = selection::restore(&mdreader_profile::RootBinding::capture_private(path)?)?;
    let mut control = ReaderStore::open_durable_readonly(&root.path.join("control"))
        .map_err(|_| Error::Conflict)?;
    let result = control.with_library_connection(|db| -> Result<_> {
        let (native, receipt) = marker::read(db)?;
        let legacy = super::legacy_record::read(db)?.ok_or(Error::Unsupported)?;
        if legacy.operation != receipt.operation || legacy.launch.profile() != &receipt.original {
            return Err(Error::Conflict);
        }
        if observe(db, receipt.operation)?.0.state != State::Active {
            return Err(Error::ReconciliationRequired);
        }
        let original = selection::restore_legacy(&receipt.original)?;
        let imported = u32::try_from(receipt.imported_records).map_err(|_| Error::Bounds)?;
        Ok((native, original, imported, legacy.launch))
    })?;
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(result)
}
