use super::claude_transcript::{claude_config_dir, home_dir_for_claude_config};
use super::json_utils::{read_json_file_or_default, write_json_file};
use super::{ThirdPartyHarness, install_docs_url};
use crate::terminal::CLIAgent;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use warp_cli::agent::Harness;
use warp_core::safe_info;
mod parent_bridge;
mod wake_driver;

pub(crate) struct ClaudeHarness;
impl ThirdPartyHarness for ClaudeHarness {
    fn harness(&self) -> Harness {
        Harness::Claude
    }

    fn cli_agent(&self) -> CLIAgent {
        CLIAgent::Claude
    }

    fn install_docs_url(&self) -> Option<&'static str> {
        install_docs_url(self.harness())
    }

    fn auth_check_command(&self) -> Option<String> {
        let cli = self.cli_agent().command_prefix();
        Some(format!("{cli} auth status --json"))
    }
}

fn claude_command(
    cli_name: &str,
    session_id: &Uuid,
    prompt_path: &str,
    system_prompt_path: Option<&str>,
    mcp_config_path: Option<&str>,
    resuming: bool,
) -> String {
    let flag = if resuming { "--resume" } else { "--session-id" };
    let mut cmd = format!("{cli_name} {flag} {session_id} --dangerously-skip-permissions");
    if let Some(sp_path) = system_prompt_path {
        let _ = write!(cmd, " --append-system-prompt-file '{sp_path}'");
    }
    if let Some(mcp_path) = mcp_config_path {
        let _ = write!(cmd, " --mcp-config '{mcp_path}'");
    }
    format!("{cmd} < '{prompt_path}'")
}

pub(crate) fn prepare_claude_environment_config(
    workspace_root: &Path,
    harness_working_dir: &Path,
    resolved_env_vars: &HashMap<OsString, OsString>,
    skill_dirs: &[PathBuf],
    factory_repository_contract: bool,
) -> Result<()> {
    let claude_json_path = claude_global_config_path()?;
    let claude_dir = claude_config_dir()?;
    let claude_settings_path = claude_dir.join(CLAUDE_SETTINGS_FILE_NAME);
    let api_key_suffix = resolve_anthropic_api_key_suffix(resolved_env_vars);
    prepare_claude_config(
        &claude_json_path,
        harness_working_dir,
        api_key_suffix.as_deref(),
    )?;
    prepare_claude_settings(&claude_settings_path)?;
    publish_skills_for_claude(
        workspace_root,
        harness_working_dir,
        skill_dirs,
        factory_repository_contract,
    )?;
    Ok(())
}

/// Publish `WARP_SKILL_DIRS` and eligible bundled skills under
/// `<harness_working_dir>/.claude/skills`.
///
/// Relative source directories are resolved from the workspace root, matching
/// Oz. The links are published into the harness working directory because
/// Claude Code discovers `.claude/skills` by walking up from its starting
/// directory to the repository root (or, absent a repository, just the
/// starting directory itself). This also keeps concurrent tasks from
/// publishing into a shared home directory. A published skill overrides any
/// existing entry with the same name (see
/// `skill_dirs_publish::publish_skill`), with the conflict-resolution behavior
/// depending on whether this run is sandboxed (see
/// `warp_isolation_platform::detect`).
fn publish_skills_for_claude(
    workspace_root: &Path,
    harness_working_dir: &Path,
    skill_dirs: &[PathBuf],
    factory_repository_contract: bool,
) -> Result<()> {
    let skill_root = harness_working_dir.join(".claude").join("skills");
    let is_sandbox = warp_isolation_platform::detect().is_some();
    let published = super::skill_dirs_publish::publish_skills_for_harness(
        &skill_root,
        workspace_root,
        is_sandbox,
        skill_dirs,
        factory_repository_contract,
    )?;
    super::skill_dirs_publish::exclude_published_skill_paths_from_git(
        harness_working_dir,
        &published,
    );
    if !published.is_empty() {
        let published = published.len();
        safe_info!(
            safe: ("Published {published} skill(s) to the Claude Code skill root"),
            full: (
                "Published {published} skill(s) to Claude Code skill root {}",
                skill_root.display()
            )
        );
    }
    Ok(())
}

// This function is used specifically for determining where to land `.claude.json`.
fn claude_global_config_path() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR")
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir).join(CLAUDE_JSON_FILE_NAME));
    }

    home_dir_for_claude_config()
        .map(|home| home.join(CLAUDE_JSON_FILE_NAME))
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}

fn prepare_claude_config(
    claude_json_path: &Path,
    working_dir: &Path,
    api_key_suffix: Option<&str>,
) -> Result<()> {
    let mut claude_config: ClaudeConfig = read_json_file_or_default(claude_json_path)?;
    claude_config.has_completed_onboarding = true;
    claude_config.lsp_recommendation_disabled = true;
    claude_config
        .projects
        .entry(working_dir.to_string_lossy().into_owned())
        .or_default()
        .has_trust_dialog_accepted = true;
    if let Some(suffix) = api_key_suffix {
        let responses = claude_config
            .custom_api_key_responses
            .get_or_insert_with(CustomApiKeyResponses::default);
        if !responses.approved.iter().any(|s| s == suffix) {
            responses.approved.push(suffix.to_owned());
        }
    }
    write_json_file(
        claude_json_path,
        &claude_config,
        "Failed to serialize Claude config",
    )?;
    Ok(())
}

fn prepare_claude_settings(claude_settings_path: &Path) -> Result<()> {
    let mut settings: ClaudeSettings = read_json_file_or_default(claude_settings_path)?;
    settings.skip_dangerous_mode_permission_prompt = true;
    write_json_file(
        claude_settings_path,
        &settings,
        "Failed to serialize Claude settings",
    )?;
    Ok(())
}

const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const CLAUDE_JSON_FILE_NAME: &str = ".claude.json";
const CLAUDE_SETTINGS_FILE_NAME: &str = "settings.json";
const ANTHROPIC_API_KEY_SUFFIX_LEN: usize = 20;

#[derive(Default, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ClaudeConfig {
    #[serde(default)]
    has_completed_onboarding: bool,
    #[serde(default)]
    lsp_recommendation_disabled: bool,
    #[serde(default)]
    projects: HashMap<String, ClaudeProjectConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom_api_key_responses: Option<CustomApiKeyResponses>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Default, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct CustomApiKeyResponses {
    #[serde(default)]
    approved: Vec<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Default, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ClaudeProjectConfig {
    #[serde(default)]
    has_trust_dialog_accepted: bool,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Default, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ClaudeSettings {
    #[serde(default)]
    skip_dangerous_mode_permission_prompt: bool,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// Try to get the last 20 chars of the ANTHROPIC_API_KEY, where 20 chars is the
/// suffix length that Claude Code truncates keys to.
fn resolve_anthropic_api_key_suffix(
    resolved_env_vars: &HashMap<OsString, OsString>,
) -> Option<String> {
    // Worker-injected process env wins.
    if let Ok(key) = std::env::var(ANTHROPIC_API_KEY_ENV)
        && !key.is_empty()
    {
        return suffix_of(&key).map(str::to_owned);
    }
    // Otherwise use the resolved value from the secrets map.
    resolved_env_vars
        .get(OsStr::new(ANTHROPIC_API_KEY_ENV))
        .and_then(|v| v.to_str())
        .and_then(suffix_of)
        .map(str::to_owned)
}

fn suffix_of(key: &str) -> Option<&str> {
    if key.len() >= ANTHROPIC_API_KEY_SUFFIX_LEN {
        key.get(key.len() - ANTHROPIC_API_KEY_SUFFIX_LEN..)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "claude_code_tests.rs"]
mod tests;
