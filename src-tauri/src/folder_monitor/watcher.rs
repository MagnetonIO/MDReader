use crate::{
    library::{LibraryStore, LocalFolderSource},
    reader::ReaderBackend,
};
use mdreader_core::SourceRoot;
use mdreader_service::Request;
use notify_debouncer_mini::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    DebounceEventResult, Debouncer,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};
use tauri::Manager;
struct Watch {
    source: String,
    member: LocalFolderSource,
    root: SourceRoot,
    _watch: Debouncer<RecommendedWatcher>,
    overflow: Arc<AtomicBool>,
}
/// A bounded owner loop admits only registry-saved exact objects. OS callbacks
/// enqueue names, never read files, update SQLite or mint an active workspace.
pub(super) fn run(
    app: tauri::AppHandle,
    stop: Arc<AtomicBool>,
    unavailable: Arc<std::sync::Mutex<HashSet<String>>>,
) {
    let backend = app.state::<ReaderBackend>().inner().clone();
    let (tx, rx) = mpsc::sync_channel::<(String, Vec<String>)>(32);
    let mut watches = HashMap::<String, Watch>::new();
    let mut caught_up = HashSet::new();
    while !stop.load(Ordering::Acquire) {
        let members = crate::library::registry_path(&app)
            .ok()
            .and_then(|(p, _)| LibraryStore::new(p).read_query_snapshot().ok())
            .map(|s| s.local_sources);
        if let Some(members) = members {
            caught_up.retain(|id| members.iter().any(|m| m.id == *id));
            if let Ok(mut bad) = unavailable.lock() {
                bad.retain(|id| members.iter().any(|m| m.id == *id));
            }
            watches.retain(|id, watch| {
                members.iter().any(|m| m.id == *id && m == &watch.member)
                    && watch.root.verify().is_ok()
            });
            for member in members {
                let Ok(root) = member.root().and_then(|r| {
                    backend
                        .protect_local_root(r)
                        .map_err(|_| crate::library::LibraryError::SourceUnavailable)
                }) else {
                    continue;
                };
                let Ok(source) = watches
                    .get(&member.id)
                    .map(|w| Ok(w.source.clone()))
                    .unwrap_or_else(|| backend.admit(&root))
                else {
                    continue;
                };
                let Ok(status) =
                    backend.typed::<mdreader_core::FolderIndexStatus>(Request::FolderIndexStatus {
                        source_id: source.clone(),
                    })
                else {
                    continue;
                };
                if !status.monitor_enabled {
                    watches.remove(&member.id);
                    caught_up.remove(&member.id);
                    if let Ok(mut bad) = unavailable.lock() {
                        bad.remove(&member.id);
                    }
                    continue;
                }
                if caught_up.insert(member.id.clone()) {
                    let _ = backend.typed::<serde_json::Value>(Request::QueueFolderChanges {
                        source_id: source.clone(),
                        paths: vec![],
                    });
                }
                if let Some(watch) = watches.get(&member.id) {
                    if watch.overflow.swap(false, Ordering::AcqRel) {
                        let _ = backend.typed::<serde_json::Value>(Request::QueueFolderChanges {
                            source_id: source,
                            paths: vec![],
                        });
                    }
                    continue;
                }
                let overflow = Arc::new(AtomicBool::new(false));
                let failed = overflow.clone();
                let emit = tx.clone();
                let scope = source.clone();
                let authority = root.clone();
                let watcher = new_debouncer(
                    Duration::from_millis(400),
                    move |result: DebounceEventResult| {
                        if authority.verify().is_err() {
                            failed.store(true, Ordering::Release);
                            return;
                        }
                        let Ok(events) = result else {
                            failed.store(true, Ordering::Release);
                            return;
                        };
                        let mut paths = Vec::new();
                        for event in events {
                            let Ok(relative) = event.path.strip_prefix(&authority.path) else {
                                failed.store(true, Ordering::Release);
                                continue;
                            };
                            if !authority.allows_relative(relative) {
                                continue;
                            }
                            if relative.as_os_str().is_empty() {
                                failed.store(true, Ordering::Release);
                                continue;
                            }
                            let Some(path) = relative.to_str() else {
                                failed.store(true, Ordering::Release);
                                continue;
                            };
                            paths.push(path.replace(std::path::MAIN_SEPARATOR, "/"));
                            if paths.len() > 256 {
                                failed.store(true, Ordering::Release);
                                return;
                            }
                        }
                        paths.sort();
                        paths.dedup();
                        if !paths.is_empty() && emit.try_send((scope.clone(), paths)).is_err() {
                            failed.store(true, Ordering::Release);
                        }
                    },
                );
                let Ok(mut watcher) = watcher else {
                    if let Ok(mut bad) = unavailable.lock() {
                        bad.insert(member.id.clone());
                    }
                    continue;
                };
                if watcher
                    .watcher()
                    .watch(&root.path, RecursiveMode::Recursive)
                    .is_err()
                {
                    if let Ok(mut bad) = unavailable.lock() {
                        bad.insert(member.id.clone());
                    }
                    continue;
                }
                if root.verify().is_err() {
                    continue;
                }
                // Arm first, then full startup catchup; callbacks cover changes
                // occurring during the catchup and invalidate its generation.
                if let Ok(mut bad) = unavailable.lock() {
                    bad.remove(&member.id);
                }
                let _ = backend.typed::<serde_json::Value>(Request::QueueFolderChanges {
                    source_id: source.clone(),
                    paths: vec![],
                });
                watches.insert(
                    member.id.clone(),
                    Watch {
                        source,
                        member,
                        root,
                        _watch: watcher,
                        overflow,
                    },
                );
            }
        } else {
            watches.clear();
        }
        let until = std::time::Instant::now() + Duration::from_secs(3);
        while !stop.load(Ordering::Acquire) && std::time::Instant::now() < until {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok((source, paths)) => {
                    // Revalidate membership even if a removed watcher callback
                    // was already queued. FindSource alone cannot restore it.
                    if watches.values().any(|w| {
                        w.source == source
                            && w.root.verify().is_ok()
                            && crate::library::registry_path(&app)
                                .ok()
                                .and_then(|(p, _)| LibraryStore::new(p).read_query_snapshot().ok())
                                .is_some_and(|s| s.local_sources.contains(&w.member))
                    }) {
                        let _ = backend.typed::<serde_json::Value>(Request::QueueFolderChanges {
                            source_id: source,
                            paths,
                        });
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}
