//! Codex rollout lines (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`) and
//! thread names (`~/.codex/session_index.jsonl`). The session id appears only
//! in the first entry, so later chunks get it from the ingest cursor.
use super::claude_log::contains;
use super::titles::prompt_title;
use mdreader_core::agent_activity::{
    clock, Agent, FileObservation, FileOp, SessionObservation, TitleSource,
};
use serde::de::IgnoredAny;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    /// Pre-`session_meta` rollouts start with `{id, timestamp, git, …}`.
    id: Option<String>,
    payload: Option<IgnoredAny>,
}
#[derive(Deserialize)]
struct Git {
    branch: Option<String>,
}
#[derive(Deserialize)]
struct MetaPayload {
    id: String,
    cwd: Option<String>,
    git: Option<Git>,
}
#[derive(Deserialize)]
struct Meta {
    payload: MetaPayload,
}
#[derive(Deserialize)]
struct LegacyMeta {
    id: String,
    git: Option<Git>,
}
#[derive(Deserialize)]
struct Change {
    #[serde(rename = "type")]
    kind: Option<String>,
    move_path: Option<String>,
}
#[derive(Deserialize)]
struct Text {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}
#[derive(Deserialize)]
struct Item {
    #[serde(rename = "type")]
    kind: Option<String>,
    status: Option<String>,
    changes: Option<BTreeMap<String, Change>>,
    #[serde(default)]
    content: Vec<Text>,
}
/// A tool call: `custom_tool_call` carries `input`, `function_call`
/// carries JSON `arguments`.
#[derive(Deserialize)]
struct Call {
    #[serde(rename = "type")]
    kind: Option<String>,
    name: Option<String>,
    input: Option<String>,
    arguments: Option<String>,
}
#[derive(Deserialize)]
struct CallEntry {
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<Call>,
    name: Option<String>,
    arguments: Option<String>,
}
#[derive(Deserialize)]
struct ItemPayload {
    item: Option<Item>,
}
#[derive(Deserialize)]
struct ItemEntry {
    payload: ItemPayload,
}

const PATCH_HEADERS: [&str; 3] = ["*** Add File: ", "*** Update File: ", "*** Move to: "];
const MAX_PATCH_FILES: usize = 128;

pub(super) struct CodexLog {
    observation: Option<SessionObservation>,
    cwd: Option<String>,
    fallback_at: i64,
}

impl CodexLog {
    /// `client_session_id`/`cwd` come from the cursor and the stored session
    /// when resuming mid-file.
    pub(super) fn new(
        client_session_id: Option<String>,
        cwd: Option<String>,
        fallback_at: i64,
    ) -> Self {
        Self {
            observation: client_session_id.map(|id| SessionObservation::metadata(Agent::Codex, id)),
            cwd,
            fallback_at,
        }
    }

    pub(super) fn client_session_id(&self) -> Option<&str> {
        self.observation
            .as_ref()
            .map(|o| o.client_session_id.as_str())
    }

    fn start(&mut self, id: String, cwd: Option<String>, branch: Option<String>) {
        let observation = self
            .observation
            .get_or_insert_with(|| SessionObservation::metadata(Agent::Codex, id));
        if let Some(cwd) = cwd.filter(|c| Path::new(c).is_absolute()) {
            self.cwd.get_or_insert_with(|| cwd.clone());
            observation.cwd.get_or_insert(cwd);
        }
        if let Some(branch) = branch.filter(|b| !b.is_empty()) {
            observation.git_branch = Some(branch);
        }
    }

    fn resolve(&self, path: &str) -> Option<PathBuf> {
        let path = Path::new(path.trim());
        if path.is_absolute() {
            Some(path.to_owned())
        } else {
            Some(Path::new(self.cwd.as_deref()?).join(path))
        }
    }

    fn file(&mut self, path: &str, op: FileOp, at: i64) {
        let Some(abs_path) = self.resolve(path) else {
            return;
        };
        if !mdreader_core::agent_activity::is_markdown(&abs_path) {
            return;
        }
        if let Some(o) = self.observation.as_mut() {
            if o.files.len() < 4096 {
                o.files.push(FileObservation {
                    abs_path,
                    op,
                    at_ms: at,
                });
            }
        }
    }

    pub(super) fn line(&mut self, line: &[u8]) {
        let Ok(entry) = serde_json::from_slice::<Entry>(line) else {
            return;
        };
        let at = entry.timestamp.as_deref().and_then(clock::parse_rfc3339_ms);
        match (
            entry.kind.as_deref(),
            entry.payload.is_some(),
            entry.id.is_some(),
        ) {
            (Some("session_meta"), true, _) => {
                if let Ok(meta) = serde_json::from_slice::<Meta>(line) {
                    let branch = meta.payload.git.and_then(|g| g.branch);
                    self.start(meta.payload.id, meta.payload.cwd, branch);
                }
            }
            (None, false, true) => {
                if let Ok(meta) = serde_json::from_slice::<LegacyMeta>(line) {
                    self.start(meta.id, None, meta.git.and_then(|g| g.branch));
                }
            }
            _ => {}
        }
        let Some(o) = self.observation.as_mut() else {
            return;
        };
        if let Some(at) = at {
            o.saw(at);
        }
        let at = at.unwrap_or(self.fallback_at);
        if contains(line, b"\"FileChange\"") || contains(line, b"\"UserMessage\"") {
            self.item(line, at);
        } else if contains(line, b"*** Begin Patch") {
            self.patch(line, at);
        }
    }

    fn item(&mut self, line: &[u8], at: i64) {
        let Ok(entry) = serde_json::from_slice::<ItemEntry>(line) else {
            return;
        };
        let Some(item) = entry.payload.item else {
            return;
        };
        match item.kind.as_deref() {
            Some("FileChange")
                if !matches!(item.status.as_deref(), Some("failed" | "declined")) =>
            {
                for (path, change) in item.changes.unwrap_or_default() {
                    match change.kind.as_deref() {
                        Some("add") => self.file(&path, FileOp::Create, at),
                        Some("update") => match change.move_path {
                            Some(to) => self.file(&to, FileOp::Edit, at),
                            None => self.file(&path, FileOp::Edit, at),
                        },
                        _ => {}
                    }
                }
            }
            Some("UserMessage") => {
                let Some(o) = self.observation.as_mut() else {
                    return;
                };
                if o.title
                    .as_ref()
                    .is_some_and(|(_, s)| *s >= TitleSource::Prompt)
                {
                    return;
                }
                let text = item
                    .content
                    .iter()
                    .find(|t| matches!(t.kind.as_deref(), Some("Text" | "text")))
                    .and_then(|t| t.text.as_deref());
                if let Some(title) = text.and_then(prompt_title) {
                    o.offer_title(&title, TitleSource::Prompt);
                }
            }
            _ => {}
        }
    }

    /// Patch text from real tool calls only: `apply_patch` (custom tool
    /// input or function arguments) and `shell` commands that run it. Patch
    /// text quoted in user messages, instructions or tool output is ignored.
    /// Only the file header lines are read.
    fn patch(&mut self, line: &[u8], at: i64) {
        let Ok(entry) = serde_json::from_slice::<CallEntry>(line) else {
            return;
        };
        let call = match entry.kind.as_deref() {
            Some("response_item") => entry.payload,
            // Pre-`response_item` rollouts log calls at the top level.
            Some("function_call") => Some(Call {
                kind: entry.kind,
                name: entry.name,
                input: None,
                arguments: entry.arguments,
            }),
            _ => None,
        };
        let Some(call) = call else { return };
        let mut patches = Vec::new();
        match (call.kind.as_deref(), call.name.as_deref()) {
            (Some("custom_tool_call"), Some("apply_patch")) => patches.extend(call.input),
            (Some("function_call"), Some("apply_patch" | "shell")) => {
                let Some(arguments) = call.arguments else {
                    return;
                };
                match serde_json::from_str::<serde_json::Value>(&arguments) {
                    Ok(value) => collect_patches(&value, 0, &mut patches),
                    Err(_) => patches.push(arguments),
                }
            }
            _ => return,
        }
        let mut found = Vec::new();
        for patch in patches {
            for line in patch.lines() {
                for header in PATCH_HEADERS {
                    if let Some(path) = line.strip_prefix(header) {
                        let op = if header == PATCH_HEADERS[0] {
                            FileOp::Create
                        } else {
                            FileOp::Edit
                        };
                        found.push((path.to_owned(), op));
                    }
                }
                if found.len() >= MAX_PATCH_FILES {
                    break;
                }
            }
        }
        for (path, op) in found {
            self.file(&path, op, at);
        }
    }

    pub(super) fn finish(mut self) -> Option<SessionObservation> {
        let fallback = self.fallback_at;
        self.observation.as_mut().map(|o| {
            if !o.files.is_empty() && !o.has_activity() {
                o.saw(fallback);
            }
            o.clone()
        })
    }
}

fn collect_patches(value: &serde_json::Value, depth: usize, out: &mut Vec<String>) {
    if depth > 8 || out.len() > 16 {
        return;
    }
    match value {
        serde_json::Value::String(text) if text.contains("*** Begin Patch") => {
            match serde_json::from_str::<serde_json::Value>(text) {
                Ok(inner) if depth < 4 => collect_patches(&inner, depth + 1, out),
                _ => out.push(text.clone()),
            }
        }
        serde_json::Value::Array(items) => items
            .iter()
            .for_each(|v| collect_patches(v, depth + 1, out)),
        serde_json::Value::Object(map) => map
            .values()
            .for_each(|v| collect_patches(v, depth + 1, out)),
        _ => {}
    }
}

#[derive(Deserialize)]
struct IndexRow {
    id: String,
    thread_name: Option<String>,
}

/// Thread names by session id; the last row for an id wins.
pub(super) fn thread_names(bytes: &[u8]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for line in bytes.split(|b| *b == b'\n') {
        if let Ok(row) = serde_json::from_slice::<IndexRow>(line) {
            if let Some(name) = row.thread_name.filter(|n| !n.trim().is_empty()) {
                names.insert(row.id, name);
            }
        }
    }
    names
}
