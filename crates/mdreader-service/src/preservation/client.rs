//! Trusted native client. Never starts, restarts or discovers another daemon.
use super::{
    types::{MAX_CONTROL_BYTES, MAX_DURATION_MS},
    *,
};
use crate::{ENDPOINT, Endpoint, Envelope, PROTOCOL, Reply, Request};
use mdreader_core::SourceRoot;
use std::{
    fs::File,
    io::{BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    path::Path,
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;
use zeroize::Zeroizing;

struct BoundEndpoint {
    root: SourceRoot,
    file: File,
    original: Zeroizing<Vec<u8>>,
    port: u16,
    token: Zeroizing<String>,
}
impl BoundEndpoint {
    fn open(directory: &Path) -> Result<Self> {
        if !cfg!(unix) {
            return Err(Error::Unsupported);
        }
        let root = SourceRoot::capture(directory.to_owned()).map_err(|_| Error::StaleOwner)?;
        let file = root
            .open_optional_file_relative(Path::new(ENDPOINT))
            .map_err(|_| Error::StaleOwner)?
            .ok_or(Error::Unavailable)?;
        let mut original = Zeroizing::new(Vec::new());
        (&file)
            .take(4097)
            .read_to_end(&mut original)
            .map_err(|_| Error::Unavailable)?;
        if original.len() > 4096 {
            return Err(Error::Bounds);
        }
        let endpoint: Endpoint = serde_json::from_slice(&original).map_err(|_| Error::Invalid)?;
        if endpoint.protocol != PROTOCOL
            || endpoint.port == 0
            || endpoint.token.len() != 64
            || !endpoint.token.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::Invalid);
        }
        let bound = Self {
            root,
            file,
            original,
            port: endpoint.port,
            token: Zeroizing::new(endpoint.token),
        };
        bound.verify()?;
        Ok(bound)
    }
    fn selected(&self) -> SelectedRoot {
        SelectedRoot {
            directory: self.root.path.clone(),
            identity: self.root.identity.clone(),
        }
    }
    fn verify(&self) -> Result<()> {
        self.root.verify().map_err(|_| Error::StaleOwner)?;
        let named = self
            .root
            .open_optional_file_relative(Path::new(ENDPOINT))
            .map_err(|_| Error::StaleOwner)?
            .ok_or(Error::StaleOwner)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let first = self.file.metadata().map_err(|_| Error::StaleOwner)?;
            let current = named.metadata().map_err(|_| Error::StaleOwner)?;
            if current.nlink() != 1 || (first.dev(), first.ino()) != (current.dev(), current.ino())
            {
                return Err(Error::StaleOwner);
            }
        }
        let mut bytes = Zeroizing::new(Vec::new());
        (&named)
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::StaleOwner)?;
        if *bytes != *self.original {
            return Err(Error::StaleOwner);
        }
        self.root.verify().map_err(|_| Error::StaleOwner)
    }
    fn call(&self, request: ControlRequest) -> Result<ControlReply> {
        self.verify()?;
        let envelope = Envelope {
            protocol: PROTOCOL,
            token: self.token.to_string(),
            request: Request::Preservation { request },
        };
        let mut bytes = Zeroizing::new(crate::encode(&envelope).map_err(|_| Error::Invalid)?);
        if bytes.len() as u64 > MAX_CONTROL_BYTES {
            return Err(Error::Bounds);
        }
        bytes.push(b'\n');
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port);
        let mut stream = TcpStream::connect_timeout(&address.into(), Duration::from_secs(1))
            .map_err(|_| Error::Uncertain)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(31)))
            .map_err(|_| Error::Uncertain)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| Error::Uncertain)?;
        stream.write_all(&bytes).map_err(|_| Error::Uncertain)?;
        let bytes = crate::read_frame(&mut BufReader::new(stream), MAX_CONTROL_BYTES)
            .map_err(|_| Error::Uncertain)?;
        self.verify()?;
        let reply: Reply = serde_json::from_slice(&bytes).map_err(|_| Error::Uncertain)?;
        if reply.error.is_some() {
            return Err(Error::Uncertain);
        }
        match serde_json::from_value(reply.result).map_err(|_| Error::Uncertain)? {
            ControlReply::Failure { error } => Err(error),
            reply => Ok(reply),
        }
    }
}
/// Connected once to an explicitly selected native service root and boot.
/// No Debug/serialization exposes the authenticated endpoint.
pub struct Client {
    endpoint: Arc<BoundEndpoint>,
    boot: Uuid,
}
impl Client {
    pub fn connect(directory: &Path) -> Result<Self> {
        let endpoint = Arc::new(BoundEndpoint::open(directory)?);
        match endpoint.call(ControlRequest::Inspect {
            root: endpoint.selected(),
        })? {
            ControlReply::Hello { boot } if boot.get_version_num() == 4 => {
                Ok(Self { endpoint, boot })
            }
            _ => Err(Error::Uncertain),
        }
    }
    /// An attempt remains bound through uncertain replies. Dropping it does NOT
    /// release the server lease; callers must explicitly reconcile/release.
    pub fn attempt(&self, operation: Uuid, owner_epoch: Uuid, duration_ms: u32) -> Result<Attempt> {
        if duration_ms == 0 || duration_ms > MAX_DURATION_MS {
            return Err(Error::Bounds);
        }
        let scope = Scope {
            operation,
            owner_epoch,
            boot: self.boot,
            root: self.endpoint.selected(),
        };
        scope.validate()?;
        Ok(Attempt {
            endpoint: self.endpoint.clone(),
            scope,
            duration_ms,
            lease: None,
            backup: None,
        })
    }
}
pub struct Attempt {
    endpoint: Arc<BoundEndpoint>,
    scope: Scope,
    duration_ms: u32,
    lease: Option<Uuid>,
    // Retain issued target identity even when its dispatch reply is lost.
    backup: Option<mdreader_profile::database_backup::BackupRequest>,
}
impl Attempt {
    fn receive(&mut self, request: ControlRequest) -> Result<Status> {
        let ControlReply::Status { status } = self.endpoint.call(request)? else {
            return Err(Error::Uncertain);
        };
        if status.remaining_ms > self.duration_ms
            || (status.phase == Phase::Held
                && (status.remaining_ms == 0
                    || status.failure.is_some()
                    || status.lease.is_none_or(|id| id.get_version_num() != 4)))
            || (status.phase != Phase::Held && status.lease.is_some())
        {
            return Err(Error::Uncertain);
        }
        if let Some(lease) = status.lease {
            if self.lease.is_some_and(|old| old != lease) {
                return Err(Error::StaleOwner);
            }
            self.lease = Some(lease);
        }
        Ok(status)
    }
    pub fn begin(&mut self) -> Result<Status> {
        self.receive(ControlRequest::Begin {
            scope: self.scope.clone(),
            duration_ms: self.duration_ms,
        })
    }
    pub fn reconcile(&mut self) -> Result<Status> {
        self.receive(ControlRequest::Reconcile {
            scope: self.scope.clone(),
        })
    }
    pub fn revalidate(&mut self) -> Result<Status> {
        self.receive(ControlRequest::Revalidate {
            scope: self.scope.clone(),
            lease: self.lease.ok_or(Error::NotHeld)?,
        })
    }
    /// Requests real service teardown while retaining its already-drained
    /// guards. The offline caller must still acquire the exact writer lock;
    /// this acknowledgement (or a lost reply) is not closed-owner proof.
    pub fn shutdown(&mut self) -> Result<()> {
        match self.endpoint.call(ControlRequest::Shutdown {
            scope: self.scope.clone(),
            lease: self.lease.ok_or(Error::NotHeld)?,
        })? {
            ControlReply::ShutdownRequested { boot } if boot == self.scope.boot => Ok(()),
            _ => Err(Error::Uncertain),
        }
    }
    pub fn release(&mut self) -> Result<Status> {
        self.receive(ControlRequest::Release {
            scope: self.scope.clone(),
            lease: self.lease.ok_or(Error::NotHeld)?,
        })
    }
}

#[path = "backup_client.rs"]
mod backup_client;
#[cfg(all(test, unix, feature = "sync-native"))]
#[path = "tests.rs"]
mod tests;
