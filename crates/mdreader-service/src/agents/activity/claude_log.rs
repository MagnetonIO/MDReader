//! Claude Code transcript lines (`~/.claude/projects/<dir>/<session>.jsonl`).
//! A light first pass reads only small metadata fields; tool inputs and the
//! first user prompt get a second, typed pass when the line can contain them.
//! Message bodies are skipped by the deserializer and never retained.
use super::titles::prompt_title;
use mdreader_core::agent_activity::{
    clock, Agent, FileObservation, FileOp, SessionObservation, TitleSource,
};
use serde::de::IgnoredAny;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
    #[serde(rename = "agentName")]
    agent_name: Option<String>,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    #[serde(rename = "isMeta", default)]
    is_meta: bool,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    #[serde(rename = "isCompactSummary", default)]
    is_compact_summary: bool,
    #[serde(default)]
    message: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct ToolInput {
    file_path: Option<String>,
    #[serde(rename = "planFilePath")]
    plan_file_path: Option<String>,
}
#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    name: Option<String>,
    input: Option<ToolInput>,
}
#[derive(Deserialize)]
struct ToolMessage {
    #[serde(default)]
    content: Vec<Block>,
}
#[derive(Deserialize)]
struct ToolEntry {
    message: ToolMessage,
}
#[derive(Deserialize)]
struct PromptMessage {
    content: serde_json::Value,
}
#[derive(Deserialize)]
struct PromptEntry {
    message: PromptMessage,
}

/// Tools whose input names a file the session created or edited.
const FILE_TOOLS: [&str; 4] = ["\"Write\"", "\"Edit\"", "\"MultiEdit\"", "\"ExitPlanMode\""];

pub(super) struct ClaudeLog {
    observation: SessionObservation,
    /// Subagent transcripts add files to their parent but never name it.
    subagent: bool,
    /// Log modification time, for entries without their own timestamp.
    fallback_at: i64,
}

impl ClaudeLog {
    pub(super) fn new(client_session_id: String, subagent: bool, fallback_at: i64) -> Self {
        Self {
            observation: SessionObservation::metadata(Agent::Claude, client_session_id),
            subagent,
            fallback_at,
        }
    }

    fn wants_prompt(&self) -> bool {
        !self.subagent
            && self
                .observation
                .title
                .as_ref()
                .is_none_or(|(_, source)| *source < TitleSource::Prompt)
    }

    fn resolve(&self, path: &str, cwd: Option<&str>) -> Option<PathBuf> {
        let path = Path::new(path);
        if path.is_absolute() {
            return Some(path.to_owned());
        }
        let cwd = cwd.or(self.observation.cwd.as_deref())?;
        Some(Path::new(cwd).join(path))
    }

    pub(super) fn line(&mut self, line: &[u8]) {
        let Ok(entry) = serde_json::from_slice::<Entry>(line) else {
            return;
        };
        let at = entry.timestamp.as_deref().and_then(clock::parse_rfc3339_ms);
        let o = &mut self.observation;
        if let Some(at) = at {
            o.saw(at);
        }
        if let Some(cwd) = entry.cwd.as_deref().filter(|c| Path::new(c).is_absolute()) {
            o.cwd.get_or_insert_with(|| cwd.to_owned());
        }
        if let Some(branch) = entry.git_branch.filter(|b| !b.is_empty() && b != "HEAD") {
            o.git_branch = Some(branch);
        }
        if !self.subagent {
            if let Some(name) = entry
                .agent_name
                .as_deref()
                .or(entry.custom_title.as_deref())
            {
                o.offer_title(name, TitleSource::Custom);
            }
            if let Some(title) = entry.ai_title.as_deref() {
                o.offer_title(title, TitleSource::Ai);
            }
        }
        if entry.message.is_none() {
            return;
        }
        match entry.kind.as_deref() {
            Some("assistant")
                if contains(line, b"\"tool_use\"")
                    && FILE_TOOLS.iter().any(|t| contains(line, t.as_bytes())) =>
            {
                self.tool_uses(line, entry.cwd.as_deref(), at);
            }
            Some("user")
                if self.wants_prompt()
                    && !entry.is_meta
                    && !entry.is_sidechain
                    && !entry.is_compact_summary
                    && !contains(line, b"\"tool_result\"") =>
            {
                self.prompt(line);
            }
            _ => {}
        }
    }

    fn tool_uses(&mut self, line: &[u8], cwd: Option<&str>, at: Option<i64>) {
        let Ok(entry) = serde_json::from_slice::<ToolEntry>(line) else {
            return;
        };
        let at = at.unwrap_or(self.fallback_at);
        for block in entry.message.content {
            if block.kind.as_deref() != Some("tool_use") {
                continue;
            }
            let Some(input) = block.input else { continue };
            let (path, op) = match block.name.as_deref() {
                Some("Write") => (input.file_path, FileOp::Create),
                Some("Edit" | "MultiEdit") => (input.file_path, FileOp::Edit),
                Some("ExitPlanMode") => (input.plan_file_path, FileOp::Create),
                _ => continue,
            };
            if let Some(abs_path) = path.as_deref().and_then(|p| self.resolve(p, cwd)) {
                if mdreader_core::agent_activity::is_markdown(&abs_path) {
                    self.observation.files.push(FileObservation {
                        abs_path,
                        op,
                        at_ms: at,
                    });
                }
            }
        }
    }

    fn prompt(&mut self, line: &[u8]) {
        let Ok(entry) = serde_json::from_slice::<PromptEntry>(line) else {
            return;
        };
        let text = match &entry.message.content {
            serde_json::Value::String(text) => Some(text.as_str()),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .and_then(|b| b.get("text"))
                .and_then(|t| t.as_str()),
            _ => None,
        };
        if let Some(title) = text.and_then(prompt_title) {
            self.observation.offer_title(&title, TitleSource::Prompt);
        }
    }

    pub(super) fn finish(mut self) -> SessionObservation {
        if !self.observation.files.is_empty() && !self.observation.has_activity() {
            self.observation.saw(self.fallback_at);
        }
        self.observation
    }
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
