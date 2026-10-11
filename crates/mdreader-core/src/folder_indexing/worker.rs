//! Two bounded descriptor-owned readers/parsers. SQLite and owner mutex never
//! cross into this worker; channel backpressure bounds live bodies/handles.
use super::*;
use crate::source_root::file_stamp::{single_link_stamp, SourceFileStamp};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender},
    Arc, Mutex,
};
pub struct FolderScan {
    pub(super) source: String,
    pub(super) generation: u64,
    pub(super) token: String,
    pub(super) root: SourceRoot,
    pub(super) targets: Option<Vec<std::path::PathBuf>>,
    receiver: Receiver<Result<ScanEvent>>,
    cancelled: Arc<AtomicBool>,
    stats: Arc<Counters>,
}
#[derive(Default)]
struct Counters {
    body_reads: AtomicU64,
    unchanged: AtomicU64,
}
#[derive(Debug, Clone, Copy)]
pub struct FolderScanStatistics {
    pub body_reads: u64,
    pub unchanged: u64,
}
pub(super) enum ScanEvent {
    Metadata {
        path: String,
        stamp: SourceFileStamp,
        size: u64,
        modified: i64,
    },
    Body {
        path: String,
        stamp: SourceFileStamp,
        revision: String,
        modified: i64,
        parsed: Option<crate::ParsedDocument>,
    },
    Complete,
}
impl Drop for FolderScan {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
impl FolderScan {
    pub(super) fn start(
        source: String,
        generation: u64,
        root: SourceRoot,
        known: HashMap<String, (SourceFileStamp, String)>,
        targets: Option<Vec<std::path::PathBuf>>,
    ) -> Self {
        let token = uuid::Uuid::new_v4().to_string();
        let (tx, receiver) = mpsc::sync_channel(16);
        let cancelled = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Counters::default());
        let counts = stats.clone();
        let authority = root.clone();
        let stop = cancelled.clone();
        let selected = targets.clone();
        std::thread::spawn(move || {
            let (jobs, input) = mpsc::sync_channel::<crate::ScannedFile>(4);
            let input = Mutex::new(input);
            let failure = Mutex::new(None);
            let outcome = std::thread::scope(|scope| -> Result<()> {
                let mut workers = Vec::new();
                for _ in 0..2 {
                    let input = &input;
                    let tx = &tx;
                    let stop = &stop;
                    let known = &known;
                    let root = &authority;
                    let counts = &counts;
                    let failure = &failure;
                    workers.push(scope.spawn(move || -> Result<()> {
                        loop {
                            let file = input.lock().map_err(|_| ReaderError::Unavailable)?.recv();
                            let Ok(file) = file else {
                                return Ok(());
                            };
                            if stop.load(Ordering::Acquire) {
                                return Err(ReaderError::Cancelled);
                            }
                            if let Err(error) = prepare(file, root, known, tx, counts) {
                                if let Ok(mut first) = failure.lock() {
                                    if first.is_none() {
                                        *first = Some(error);
                                    }
                                }
                                stop.store(true, Ordering::Release);
                                return Err(ReaderError::Cancelled);
                            }
                        }
                    }));
                }
                let options = crate::ScanOptions {
                    include_mdx: false,
                    max_file_size: MAX_DOCUMENT_BYTES,
                    respect_gitignore: true,
                };
                let mut visit = |mut file| -> Result<()> {
                    loop {
                        if stop.load(Ordering::Acquire) {
                            return Err(ReaderError::Cancelled);
                        }
                        match jobs.try_send(file) {
                            Ok(()) => return Ok(()),
                            Err(mpsc::TrySendError::Full(pending)) => {
                                file = pending;
                                std::thread::sleep(std::time::Duration::from_millis(1));
                            }
                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                return Err(ReaderError::Cancelled)
                            }
                        }
                    }
                };
                let walk = match selected.as_ref() {
                    Some(paths) => {
                        crate::scan::visit_source_targets(&authority, &options, paths, &mut visit)
                    }
                    None => crate::visit_source_root(&authority, &options, &mut visit).map(|_| ()),
                };
                drop(jobs);
                let mut joined = Ok(());
                for worker in workers {
                    if let Err(error) = worker.join().unwrap_or(Err(ReaderError::Unavailable)) {
                        joined = Err(error);
                    }
                }
                if let Some(error) = failure.lock().map_err(|_| ReaderError::Unavailable)?.take() {
                    return Err(error);
                }
                joined?;
                walk?;
                authority.verify()?;
                Ok(())
            });
            let _ = tx.send(outcome.map(|()| ScanEvent::Complete));
        });
        Self {
            source,
            generation,
            token,
            root,
            targets,
            receiver,
            cancelled,
            stats,
        }
    }
    pub fn statistics(&self) -> FolderScanStatistics {
        FolderScanStatistics {
            body_reads: self.stats.body_reads.load(Ordering::Acquire),
            unchanged: self.stats.unchanged.load(Ordering::Acquire),
        }
    }
    pub(super) fn next(&self) -> Result<Option<ScanEvent>> {
        match self.receiver.try_recv() {
            Ok(event) => event.map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(ReaderError::Unavailable),
        }
    }
}
fn prepare(
    file: crate::ScannedFile,
    root: &SourceRoot,
    known: &HashMap<String, (SourceFileStamp, String)>,
    tx: &SyncSender<Result<ScanEvent>>,
    counts: &Counters,
) -> Result<()> {
    let stamp = file.stamp()?;
    let meta = file.metadata()?;
    let size = meta.len();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0);
    tx.send(Ok(ScanEvent::Metadata {
        path: file.rel.clone(),
        stamp: stamp.clone(),
        size,
        modified,
    }))
    .map_err(|_| ReaderError::Cancelled)?;
    let (revision, parsed) = if let Some((old, revision)) =
        known.get(&file.rel).filter(|(old, _)| old == &stamp)
    {
        let _ = old;
        counts.unchanged.fetch_add(1, Ordering::Relaxed);
        (revision.clone(), None)
    } else {
        counts.body_reads.fetch_add(1, Ordering::Relaxed);
        let bytes = file.read_bounded(MAX_DOCUMENT_BYTES)?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err(invalid("Markdown exceeds size limit"));
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid("Markdown must be UTF-8"))?;
        (
            super::super::revision(&bytes),
            Some(crate::parse_markdown(text, &file.rel)),
        )
    };
    if file.stamp()? != stamp
        || single_link_stamp(&root.open_file_relative(std::path::Path::new(&file.rel))?)? != stamp
    {
        return Err(ReaderError::Conflict);
    }
    root.verify()?;
    tx.send(Ok(ScanEvent::Body {
        path: file.rel,
        stamp,
        revision,
        modified,
        parsed,
    }))
    .map_err(|_| ReaderError::Cancelled)
}
