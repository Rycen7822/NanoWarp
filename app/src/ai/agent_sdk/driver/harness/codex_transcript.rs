//! Transcript envelopes and GUI continuation helpers.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::json_utils::entries_to_jsonl;

/// Env var codex honors to override `~/.codex` (see codex `core/src/config/mod.rs`).
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_HOME_DIRNAME: &str = ".codex";
/// Subdirectory under `$CODEX_HOME` where rollouts live.
const CODEX_SESSIONS_SUBDIR: &str = "sessions";

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct CodexTranscriptEnvelope {
    /// The directory the codex session started in (recovered from the `SessionMeta` line).
    pub(crate) cwd: PathBuf,
    /// Codex session/thread UUID. Matches the trailing `-<uuid>` in the rollout filename.
    pub(crate) session_id: Uuid,
    /// `cli_version` from `SessionMeta`, surfaced separately for the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) codex_version: Option<String>,
    /// Timestamp from the `SessionMeta` line, used to derive the YYYY/MM/DD directory
    /// path when writing the rollout file back to disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_start_timestamp: Option<DateTime<Utc>>,
    /// Parsed JSONL entries.
    pub(crate) entries: Vec<Value>,
}

pub(crate) fn codex_sessions_root() -> anyhow::Result<PathBuf> {
    let home = if let Ok(dir) = std::env::var(CODEX_HOME_ENV) {
        PathBuf::from(dir)
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?
            .join(CODEX_HOME_DIRNAME)
    };
    Ok(home.join(CODEX_SESSIONS_SUBDIR))
}

pub(crate) fn write_envelope(
    envelope: &CodexTranscriptEnvelope,
    sessions_root: &Path,
) -> Result<PathBuf> {
    let timestamp = envelope.session_start_timestamp.unwrap_or_else(Utc::now);
    let day_dir = sessions_root
        .join(format!("{:04}", timestamp.year()))
        .join(format!("{:02}", timestamp.month()))
        .join(format!("{:02}", timestamp.day()));
    fs::create_dir_all(&day_dir)
        .with_context(|| format!("Failed to create {}", day_dir.display()))?;
    // Codex's filename format: `[year]-[month]-[day]T[hour]-[minute]-[second]`
    // (codex `rollout/src/recorder.rs::precompute_log_file_info`).
    let date_str = timestamp.format("%Y-%m-%dT%H-%M-%S").to_string();
    let file_path = day_dir.join(format!(
        "rollout-{date_str}-{session_id}.jsonl",
        session_id = envelope.session_id
    ));
    fs::write(&file_path, entries_to_jsonl(&envelope.entries)?)
        .with_context(|| format!("Failed to write {}", file_path.display()))?;
    Ok(file_path)
}

pub(crate) fn rehydrate_codex_transcript_from_reader(reader: impl Read) -> Result<String> {
    let envelope: CodexTranscriptEnvelope =
        serde_json::from_reader(reader).context("Failed to parse codex transcript envelope")?;
    let session_id = envelope.session_id;
    let sessions_root = codex_sessions_root().context("Failed to resolve codex sessions root")?;
    // Write as-is: no cwd mutation, no session_meta patch.
    write_envelope(&envelope, &sessions_root).context("Failed to rehydrate codex transcript")?;
    Ok(format!("codex resume {session_id}"))
}

#[cfg(test)]
#[path = "codex_transcript_tests.rs"]
mod tests;
