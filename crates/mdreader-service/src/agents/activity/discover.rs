//! Find logs that need reading: inside an enabled agent's history window and
//! changed since their cursor. Metadata only; no log is opened here.
use super::{ingest, Homes};
use mdreader_core::agent_activity::Agent;
use mdreader_core::{ReaderStore, Result};
use std::ops::DerefMut;
use std::path::{Path, PathBuf};

/// Bounds the walk so a huge history cannot stall one request.
const MAX_ENTRIES: usize = 100_000;
const MAX_RESULTS: usize = 5_000;

fn children(dir: &Path, budget: &mut usize) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .filter_map(|e| e.ok())
        .take_while(|_| {
            *budget = budget.saturating_sub(1);
            *budget > 0
        })
        .map(|e| e.path())
        .collect()
}

/// Candidate log files at the depths each agent writes them.
fn candidates(homes: &Homes, agent: Agent, budget: &mut usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match agent {
        Agent::Claude => {
            for project in children(&homes.claude_projects, budget) {
                for entry in children(&project, budget) {
                    if entry.is_dir() {
                        out.extend(children(&entry.join("subagents"), budget));
                    } else {
                        out.push(entry);
                    }
                }
            }
        }
        Agent::Codex => {
            for year in children(&homes.codex_sessions, budget) {
                for month in children(&year, budget) {
                    for day in children(&month, budget) {
                        out.extend(children(&day, budget));
                    }
                }
            }
            out.push(homes.codex_index.clone());
        }
    }
    out
}

pub(super) fn changed_logs<G: DerefMut<Target = ReaderStore>>(
    lock: &dyn Fn() -> Result<G>,
    homes: &Homes,
) -> Result<Vec<PathBuf>> {
    let windows: Vec<(Agent, i64)> = lock()?
        .agent_tracking()?
        .into_iter()
        .filter(|t| t.enabled)
        .map(|t| (t.agent, t.since_ms.unwrap_or(0)))
        .collect();
    let mut budget = MAX_ENTRIES;
    let mut found: Vec<(i64, u64, PathBuf)> = Vec::new();
    for (agent, since) in windows {
        for path in candidates(homes, agent, &mut budget) {
            if ingest::classify(homes, &path).is_none() {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            let mtime = ingest::mtime_ms(&metadata);
            if metadata.is_file() && mtime >= since {
                found.push((mtime, metadata.len(), path));
            }
        }
    }
    // Newest first; the index sorts last so thread names land on sessions
    // already read in this pass.
    found.sort_by(|a, b| {
        let index = |p: &Path| p == homes.codex_index;
        index(&a.2).cmp(&index(&b.2)).then(b.0.cmp(&a.0))
    });
    let mut store = lock()?;
    let mut out = Vec::new();
    for (mtime, size, path) in found {
        let Some(key) = path.to_str() else { continue };
        let unchanged = store
            .agent_ingest_cursor(key)?
            .is_some_and(|c| c.size == size && c.mtime_ms == mtime);
        if !unchanged {
            out.push(path);
            if out.len() >= MAX_RESULTS {
                break;
            }
        }
    }
    Ok(out)
}
