//! Transcript envelopes and GUI continuation helpers.
use std::collections::HashMap;
use std::fs::{create_dir_all, write};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use warp_core::safe_warn;

use super::json_utils::entries_to_jsonl;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ClaudeTranscriptEnvelope {
    /// The directory that the Claude Code session started in.
    pub(crate) cwd: PathBuf,
    /// Unique session identifier.
    pub(crate) uuid: Uuid,
    /// Claude Code version, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) claude_version: Option<String>,
    /// List of messages in the main agent conversation.
    pub(crate) entries: Vec<Value>,
    /// Messages in each subagent conversation, keyed by the agent filename (e.g. `"agent-aac0b7f3db6bccfaf"`).
    pub(crate) subagents: HashMap<String, Vec<Value>>,
    /// TODO lists for each agent, keyed on the session and agent (e.g. `"<session_uuid>-agent-<agent_id>"`).
    pub(crate) todos: HashMap<String, Value>,
}

#[derive(Debug)]
pub(crate) struct ClaudeLocalContinuation {
    pub(crate) command: String,
}

const SESSIONS_INDEX_FILENAME: &str = "sessions-index.json";

pub(crate) fn encode_cwd(cwd: &Path) -> String {
    let cwd = cwd.to_string_lossy();
    let mut encoded = String::with_capacity(cwd.len());
    let mut hash = 0i32;

    // Claude's JavaScript encoder treats a non-BMP character as two UTF-16 units.
    for unit in cwd.encode_utf16() {
        let character = match unit {
            0x30..=0x39 | 0x41..=0x5a | 0x61..=0x7a => char::from(unit as u8),
            _ => '-',
        };
        encoded.push(character);
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
    }

    if encoded.len() > 200 {
        encoded.truncate(200);
        encoded.push('-');
        let mut value = hash.unsigned_abs();
        let mut suffix = Vec::new();
        loop {
            suffix.push(char::from(
                b"0123456789abcdefghijklmnopqrstuvwxyz"[(value % 36) as usize],
            ));
            value /= 36;
            if value == 0 {
                break;
            }
        }
        encoded.extend(suffix.into_iter().rev());
    }

    encoded
}

pub(crate) fn claude_config_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    home_dir_for_claude_config()
        .map(|h| h.join(".claude"))
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}

pub(super) fn home_dir_for_claude_config() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home));
    }
    dirs::home_dir()
}

pub(crate) fn write_envelope(
    envelope: &ClaudeTranscriptEnvelope,
    config_root: &Path,
) -> Result<()> {
    let encoded = encode_cwd(&envelope.cwd);
    let projects_dir = config_root.join("projects").join(&encoded);
    create_dir_all(&projects_dir)
        .with_context(|| format!("Failed to create {}", projects_dir.display()))?;

    // Main session JSONL.
    let session_file = projects_dir.join(format!("{}.jsonl", envelope.uuid));
    write(&session_file, entries_to_jsonl(&envelope.entries)?)
        .with_context(|| format!("Failed to write {}", session_file.display()))?;

    // Subagent JSONLs.
    if !envelope.subagents.is_empty() {
        let subagents_dir = projects_dir
            .join(envelope.uuid.to_string())
            .join("subagents");
        create_dir_all(&subagents_dir)
            .with_context(|| format!("Failed to create {}", subagents_dir.display()))?;
        for (stem, entries) in &envelope.subagents {
            let path = subagents_dir.join(format!("{stem}.jsonl"));
            write(&path, entries_to_jsonl(entries)?)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }
    }

    // Per-agent todo lists.
    if !envelope.todos.is_empty() {
        let todos_dir = config_root.join("todos");
        create_dir_all(&todos_dir)
            .with_context(|| format!("Failed to create {}", todos_dir.display()))?;
        for (stem, value) in &envelope.todos {
            let path = todos_dir.join(format!("{stem}.json"));
            write(&path, serde_json::to_vec(value)?)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }
    }

    Ok(())
}

pub(crate) fn write_envelope_for_local_continuation(
    envelope: &ClaudeTranscriptEnvelope,
    storage_cwd: &Path,
    config_root: &Path,
) -> Result<()> {
    let projects_dir = config_root.join("projects").join(encode_cwd(storage_cwd));
    create_dir_all(&projects_dir)
        .with_context(|| format!("Failed to create {}", projects_dir.display()))?;

    // Main session JSONL.
    let session_file = projects_dir.join(format!("{}.jsonl", envelope.uuid));
    write(&session_file, entries_to_jsonl(&envelope.entries)?)
        .with_context(|| format!("Failed to write {}", session_file.display()))?;

    // Subagent JSONLs — same relative layout as write_envelope.
    if !envelope.subagents.is_empty() {
        let subagents_dir = projects_dir
            .join(envelope.uuid.to_string())
            .join("subagents");
        create_dir_all(&subagents_dir)
            .with_context(|| format!("Failed to create {}", subagents_dir.display()))?;
        for (stem, entries) in &envelope.subagents {
            let path = subagents_dir.join(format!("{stem}.jsonl"));
            write(&path, entries_to_jsonl(entries)?)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }
    }

    // Per-agent todo lists are written to the same global location as cloud resume.
    if !envelope.todos.is_empty() {
        let todos_dir = config_root.join("todos");
        create_dir_all(&todos_dir)
            .with_context(|| format!("Failed to create {}", todos_dir.display()))?;
        for (stem, value) in &envelope.todos {
            let path = todos_dir.join(format!("{stem}.json"));
            write(&path, serde_json::to_vec(value)?)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }
    }

    Ok(())
}

pub(crate) fn rehydrate_claude_transcript_from_reader(
    reader: impl Read,
) -> Result<ClaudeLocalContinuation> {
    let envelope: ClaudeTranscriptEnvelope =
        serde_json::from_reader(reader).context("Failed to parse Claude transcript envelope")?;
    let session_id = envelope.uuid;
    let config_root = claude_config_dir().context("Failed to resolve Claude config dir")?;
    let home_dir = home_dir_for_claude_config()
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    write_envelope_for_local_continuation(&envelope, &home_dir, &config_root)
        .context("Failed to rehydrate Claude transcript for local continuation")?;
    write_session_index_entry(session_id, &home_dir, &config_root)
        .context("Failed to update Claude sessions-index.json")?;
    Ok(ClaudeLocalContinuation {
        command: format!("claude --resume {session_id}"),
    })
}

pub(crate) fn write_session_index_entry(
    session_uuid: Uuid,
    cwd: &Path,
    config_root: &Path,
) -> Result<()> {
    let index_path = config_root.join(SESSIONS_INDEX_FILENAME);

    // Read the existing index if present. Missing or malformed files are treated as empty —
    // we'd rather clobber an unparsable file than fail the whole resume.
    let mut index: serde_json::Map<String, Value> = match std::fs::read_to_string(&index_path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                safe_warn!(
                    safe: ("sessions-index.json is not a JSON object; overwriting"),
                    full: ("sessions-index.json at {} is not a JSON object; overwriting", index_path.display())
                );
                serde_json::Map::new()
            }
            Err(e) => {
                safe_warn!(
                    safe: ("Failed to parse sessions-index.json; overwriting"),
                    full: ("Failed to parse sessions-index.json at {}: {e}; overwriting", index_path.display())
                );
                serde_json::Map::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => {
            return Err(
                anyhow::Error::from(e).context(format!("Failed to read {}", index_path.display()))
            );
        }
    };

    let encoded = encode_cwd(cwd);
    let transcript_path = format!("projects/{encoded}/{session_uuid}.jsonl");
    let entry = serde_json::json!({
        "sessionId": session_uuid.to_string(),
        "cwd": cwd.to_string_lossy(),
        "projectPath": encoded,
        "transcriptPath": transcript_path,
    });
    index.insert(session_uuid.to_string(), entry);

    if let Some(parent) = index_path.parent() {
        create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    write(
        &index_path,
        serde_json::to_vec_pretty(&Value::Object(index))
            .context("Failed to serialize sessions-index.json")?,
    )
    .with_context(|| format!("Failed to write {}", index_path.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "claude_transcript_tests.rs"]
mod tests;
