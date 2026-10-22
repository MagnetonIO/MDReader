//! Incremental, bounded log reading. Each log is read from its cursor to the
//! last complete line; the parsed observation and the new cursor commit
//! together. Size or mtime moving backwards (rotation, rewrite) rescans.
use super::{claude_log::ClaudeLog, codex_log, codex_log::CodexLog, Homes};
use mdreader_core::agent_activity::{Agent, Cursor, SessionObservation, TitleSource};
use mdreader_core::{ReaderError, ReaderStore, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::ops::DerefMut;
use std::path::{Component, Path};

/// Lines longer than this are skipped whole, never buffered.
pub(super) const MAX_LINE: usize = 8 * 1024 * 1024;
const MAX_INDEX: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Log {
    Claude { client: String, subagent: bool },
    Codex,
    CodexIndex,
}
impl Log {
    pub(super) fn agent(&self) -> Agent {
        match self {
            Log::Claude { .. } => Agent::Claude,
            Log::Codex | Log::CodexIndex => Agent::Codex,
        }
    }
}

fn names(path: &Path, root: &Path) -> Option<Vec<String>> {
    let relative = path.strip_prefix(root).ok()?;
    relative
        .components()
        .map(|c| match c {
            Component::Normal(n) => n.to_str().map(str::to_owned),
            _ => None,
        })
        .collect()
}

fn client_id(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

/// Which known log a path is. Anything else is refused, so callers cannot
/// make the owner read arbitrary files.
pub(super) fn classify(homes: &Homes, path: &Path) -> Option<Log> {
    if !path.is_absolute() {
        return None;
    }
    if path == homes.codex_index {
        return Some(Log::CodexIndex);
    }
    let jsonl = |name: &str| name.len() > 6 && name.ends_with(".jsonl");
    if let Some(parts) = names(path, &homes.claude_projects) {
        return match parts.as_slice() {
            [_, file] if jsonl(file) => Some(Log::Claude {
                client: client_id(file.trim_end_matches(".jsonl"))?,
                subagent: false,
            }),
            [_, session, sub, file] if sub == "subagents" && jsonl(file) => Some(Log::Claude {
                client: client_id(session)?,
                subagent: true,
            }),
            _ => None,
        };
    }
    match names(path, &homes.codex_sessions)?.as_slice() {
        [y, m, d, file]
            if [y, m, d]
                .iter()
                .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
                && file.starts_with("rollout-")
                && jsonl(file) =>
        {
            Some(Log::Codex)
        }
        _ => None,
    }
}

pub(super) fn mtime_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

enum LineRead {
    Line(usize),
    Oversized(usize),
    End,
}

fn next_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<LineRead> {
    buf.clear();
    let (mut consumed, mut oversized) = (0usize, false);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            // A trailing line without its newline may still be being written.
            return Ok(LineRead::End);
        }
        let (take, done) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (i, true),
            None => (available.len(), false),
        };
        if !oversized && buf.len() + take <= MAX_LINE {
            buf.extend_from_slice(&available[..take]);
        } else {
            oversized = true;
            buf.clear();
        }
        let step = take + usize::from(done);
        reader.consume(step);
        consumed += step;
        if done {
            return Ok(if oversized {
                LineRead::Oversized(consumed)
            } else {
                LineRead::Line(consumed)
            });
        }
    }
}

/// Read complete new lines after `cursor`. `None` when nothing changed.
fn read_new(
    path: &Path,
    agent: Agent,
    cursor: Option<&Cursor>,
    each: &mut dyn FnMut(&[u8]),
) -> Result<Option<Cursor>> {
    // Never block a handler on a FIFO or device named like a log.
    let file = mdreader_core::source_root::file_stamp::open_regular(path)?;
    let metadata = file.metadata()?;
    let (size, mtime) = (metadata.len(), mtime_ms(&metadata));
    let resume = cursor.filter(|c| c.size <= size && c.mtime_ms <= mtime && c.byte_offset <= size);
    if resume.is_some_and(|c| c.size == size && c.mtime_ms == mtime) {
        return Ok(None);
    }
    let mut offset = resume.map_or(0, |c| c.byte_offset);
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    reader.seek(SeekFrom::Start(offset))?;
    let mut line = Vec::new();
    loop {
        match next_line(&mut reader, &mut line)? {
            LineRead::Line(n) => {
                each(&line);
                offset += n as u64;
            }
            LineRead::Oversized(n) => offset += n as u64,
            LineRead::End => break,
        }
    }
    Ok(Some(Cursor {
        log_path: path.to_str().ok_or(ReaderError::Unavailable)?.to_owned(),
        agent,
        client_session_id: resume.and_then(|c| c.client_session_id.clone()),
        byte_offset: offset,
        mtime_ms: mtime,
        size,
    }))
}

/// Lazily loaded Codex thread names, shared across one ingest batch.
pub(super) struct ThreadNames<'a> {
    homes: &'a Homes,
    names: Option<HashMap<String, String>>,
}
impl<'a> ThreadNames<'a> {
    pub(super) fn new(homes: &'a Homes) -> Self {
        Self { homes, names: None }
    }
    fn get(&mut self, id: &str) -> Option<&String> {
        self.names
            .get_or_insert_with(|| read_index(&self.homes.codex_index).unwrap_or_default())
            .get(id)
    }
}

fn read_index(path: &Path) -> Option<HashMap<String, String>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    mdreader_core::source_root::file_stamp::open_regular(path)
        .ok()?
        .take(MAX_INDEX)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(codex_log::thread_names(&bytes))
}

/// Ingest one log. Parsing runs without the owner lock; only the cursor read
/// and the final commit take it. Returns changed session ids and bytes read.
pub(super) fn ingest<G: DerefMut<Target = ReaderStore>>(
    lock: &dyn Fn() -> Result<G>,
    log: &Log,
    path: &Path,
    names: &mut ThreadNames<'_>,
) -> Result<(Vec<String>, u64)> {
    let agent = log.agent();
    let key = path.to_str().ok_or(ReaderError::Unavailable)?;
    let cursor = lock()?.agent_ingest_cursor(key)?;
    let fallback = std::fs::metadata(path).map(|m| mtime_ms(&m))?;
    let start = cursor.as_ref().map_or(0, |c| c.byte_offset);
    let (observations, cursor): (Vec<SessionObservation>, Option<Cursor>) = match log {
        Log::Claude { client, subagent } => {
            let mut parser = ClaudeLog::new(client.clone(), *subagent, fallback);
            let read = read_new(path, agent, cursor.as_ref(), &mut |l| parser.line(l))?;
            (vec![parser.finish()], read)
        }
        Log::Codex => {
            let client = cursor.as_ref().and_then(|c| c.client_session_id.clone());
            let cwd = match &client {
                Some(client) => {
                    let mut store = lock()?;
                    match store.agent_session_id(Agent::Codex, client)? {
                        Some(id) => store.agent_session_ref(Agent::Codex, &id)?.1,
                        None => None,
                    }
                }
                None => None,
            };
            let mut parser = CodexLog::new(client, cwd, fallback);
            let read = read_new(path, agent, cursor.as_ref(), &mut |l| parser.line(l))?;
            let read = read.map(|mut c| {
                c.client_session_id = parser.client_session_id().map(str::to_owned);
                c
            });
            let mut observation = parser.finish();
            if let Some(o) = observation.as_mut() {
                if let Some(name) = names.get(&o.client_session_id) {
                    o.offer_title(name, TitleSource::Custom);
                }
            }
            (observation.into_iter().collect(), read)
        }
        Log::CodexIndex => {
            let metadata = std::fs::metadata(path)?;
            let (size, mtime) = (metadata.len(), mtime_ms(&metadata));
            if cursor
                .as_ref()
                .is_some_and(|c| c.size == size && c.mtime_ms == mtime)
            {
                return Ok((vec![], 0));
            }
            let all = read_index(path).unwrap_or_default();
            let observations = all
                .iter()
                .map(|(id, name)| {
                    let mut o = SessionObservation::metadata(Agent::Codex, id.clone());
                    o.offer_title(name, TitleSource::Custom);
                    o
                })
                .collect();
            let cursor = Cursor {
                log_path: key.to_owned(),
                agent,
                client_session_id: None,
                byte_offset: size,
                mtime_ms: mtime,
                size,
            };
            (observations, Some(cursor))
        }
    };
    let Some(cursor) = cursor else {
        return Ok((vec![], 0));
    };
    let read = cursor.byte_offset.saturating_sub(start);
    let changed = lock()?.record_agent_activity(&observations, Some(&cursor))?;
    Ok((changed, read))
}
