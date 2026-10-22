use super::{
    AgentDriverError, LEGACY_OZ_PARENT_LISTENER_MANAGED_EXTERNALLY_ENV,
    LEGACY_OZ_PARENT_STATE_ROOT_ENV, OZ_MESSAGE_LISTENER_MANAGED_EXTERNALLY_ENV,
    OZ_MESSAGE_LISTENER_STATE_ROOT_ENV, WARP_MESSAGE_LISTENER_MANAGED_EXTERNALLY_ENV,
    WARP_MESSAGE_LISTENER_STATE_ROOT_ENV,
};
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::ambient_agents::task::HarnessModelConfig;
use crate::terminal::CLIAgent;
use crate::util::path::resolve_executable;
use anyhow::Result;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use warp_cli::agent::Harness;
use warp_cli::{
    OZ_CLI_ENV, OZ_HARNESS_ENV, OZ_PARENT_RUN_ID_ENV, OZ_RUN_ID_ENV, SERVER_ROOT_URL_OVERRIDE_ENV,
    SESSION_SHARING_SERVER_URL_OVERRIDE_ENV, WARP_CLI_ENV, WARP_HARNESS_ENV,
    WARP_PARENT_RUN_ID_ENV, WARP_RUN_ID_ENV, WS_SERVER_URL_OVERRIDE_ENV,
};
use warp_core::channel::ChannelState;

pub(crate) mod claude_code;
pub(crate) mod claude_transcript;
mod codex;
pub(crate) mod codex_transcript;
mod gemini;
mod json_utils;
mod skill_dirs_publish;
pub(crate) use claude_code::ClaudeHarness;
use codex::CodexHarness;
use gemini::GeminiHarness;

pub(crate) trait ThirdPartyHarness: Send + Sync {
    /// Returns the [`Harness`] variant this implementation corresponds to.
    fn harness(&self) -> Harness;

    /// Returns the CLIAgent type associated with this harness.
    fn cli_agent(&self) -> CLIAgent;

    /// URL to install instructions for this harness's CLI, surfaced in the
    /// default [`validate`] impl when the CLI is not on `PATH`.
    fn install_docs_url(&self) -> Option<&'static str> {
        None
    }

    /// Validate that the harness is ready to run. Default impl checks that the
    /// CLI is installed on `PATH`; override for additional checks.
    fn validate(&self) -> Result<(), AgentDriverError> {
        validate_cli_installed(self.cli_agent().command_prefix(), self.install_docs_url())
    }

    /// Shell command to verify authentication credentials are valid.
    /// Exit code 0 = pass; non-zero = fail.
    fn auth_check_command(&self) -> Option<String> {
        None
    }
}

/// Harness support available to local GUI panes.
pub(crate) enum HarnessKind {
    Oz,
    /// Third-party CLI-backed harness (e.g. Claude, Gemini).
    ThirdParty(Box<dyn ThirdPartyHarness>),
    /// Harnesses without local CLI validation support.
    Unsupported(Harness),
}

impl HarnessKind {
    /// Corresponding [`Harness`] enum value.
    pub(crate) fn harness(&self) -> Harness {
        match self {
            HarnessKind::Oz => Harness::Oz,
            HarnessKind::ThirdParty(h) => h.harness(),
            HarnessKind::Unsupported(harness) => *harness,
        }
    }
}

impl fmt::Debug for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use the `Display` method on the [`Harness`] enum.
        write!(f, "{}", self.harness())
    }
}

/// Build a [`HarnessKind`] for the given [`Harness`] and transport.
///
/// We shouldn't ever get a `--harness unknown` here because clap should handle
/// it.
pub(crate) fn harness_kind(harness: Harness) -> Result<HarnessKind, AgentDriverError> {
    match harness {
        Harness::Oz => Ok(HarnessKind::Oz),
        Harness::Claude => Ok(HarnessKind::ThirdParty(Box::new(ClaudeHarness))),
        Harness::Codex => Ok(HarnessKind::ThirdParty(Box::new(CodexHarness))),
        Harness::Gemini => Ok(HarnessKind::ThirdParty(Box::new(GeminiHarness))),
        Harness::Pi | Harness::OpenCode => Ok(HarnessKind::Unsupported(harness)),
        Harness::Unknown => Err(AgentDriverError::InvalidRuntimeState),
    }
}

/// URL to install instructions for the harness's CLI.
fn install_docs_url(harness: Harness) -> Option<&'static str> {
    match harness {
        Harness::Claude => Some("https://code.claude.com/docs/en/quickstart"),
        Harness::Codex => Some("https://developers.openai.com/codex/cli"),
        Harness::Gemini => Some("https://geminicli.com/"),
        Harness::Pi => Some("https://github.com/svkozak/pi-acp#install"),
        Harness::Oz | Harness::OpenCode | Harness::Unknown => None,
    }
}

/// Returns the harness's auth-check preflight command, if any.
///
/// The viewer uses this to recognize preflight blocks via exact string
/// equality (so they stay grouped under "Set up environment commands"
/// rather than being mistaken for the main harness invocation, which
/// shares the same CLI prefix).
///
/// Returns `None` for [`Harness::Oz`], for unsupported harnesses, and
/// for any third-party harness whose `auth_check_command` returns `None`
/// (e.g. Gemini today).
pub(crate) fn auth_check_command_for(harness: Harness) -> Option<String> {
    let HarnessKind::ThirdParty(third_party) = harness_kind(harness).ok()? else {
        return None;
    };
    third_party.auth_check_command()
}

/// Check that `cli` is installed and on PATH, returning a `HarnessSetupFailed`
/// error with an optional install-docs link when it isn't.
pub(crate) fn validate_cli_installed(
    cli: &str,
    install_docs_url: Option<&str>,
) -> Result<(), AgentDriverError> {
    if resolve_executable(cli).is_none() {
        let mut reason = format!("'{cli}' CLI not found on your machine.");
        if let Some(url) = install_docs_url {
            reason.push_str(&format!(" Install it first: {url}"));
        }
        return Err(AgentDriverError::HarnessSetupFailed {
            harness: cli.into(),
            reason,
        });
    }
    Ok(())
}

fn insert_non_empty_task_env_var(
    env_vars: &mut HashMap<OsString, OsString>,
    key: &'static str,
    value: String,
) {
    if value.is_empty() {
        return;
    }

    env_vars.insert(OsString::from(key), OsString::from(value));
}

/// Writes `value` under every name in `keys`.
///
/// Gives a variable both its `OZ_` and its `WARP_` name from a single value, so the two cannot
/// carry different ones. `task_env_vars_mirror_every_oz_var_to_a_warp_name` fails if a name is
/// listed here under only one of the two spellings.
///
/// Takes `AsRef<OsStr>` rather than `&str` so a path-valued variable stays byte-exact: `OZ_CLI`
/// holds an executable path that agents exec, and a lossy conversion would replace non-UTF-8
/// bytes and leave them unable to launch it.
fn insert_task_env_var_names(
    env_vars: &mut HashMap<OsString, OsString>,
    keys: &[&'static str],
    value: impl AsRef<OsStr>,
) {
    let value = value.as_ref();
    for key in keys {
        env_vars.insert(OsString::from(key), value.to_os_string());
    }
}

fn message_listener_state_root() -> Option<String> {
    [
        OZ_MESSAGE_LISTENER_STATE_ROOT_ENV,
        LEGACY_OZ_PARENT_STATE_ROOT_ENV,
    ]
    .into_iter()
    .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
}

pub(crate) fn task_env_vars(
    task_id: Option<&AmbientAgentTaskId>,
    parent_run_id: Option<&str>,
    selected_harness: Harness,
) -> HashMap<OsString, OsString> {
    // Sized for the OZ_/WARP_ pairs written below.
    let mut env_vars = HashMap::with_capacity(14);

    if let Some(id) = task_id {
        insert_task_env_var_names(
            &mut env_vars,
            &[OZ_RUN_ID_ENV, WARP_RUN_ID_ENV],
            id.to_string(),
        );
    }

    if let Some(parent_run_id) = parent_run_id.filter(|id| !id.is_empty()) {
        insert_task_env_var_names(
            &mut env_vars,
            &[OZ_PARENT_RUN_ID_ENV, WARP_PARENT_RUN_ID_ENV],
            parent_run_id,
        );
    }

    let cli_path = std::env::current_exe()
        .unwrap_or_else(|_| ChannelState::channel().cli_command_name().into());
    insert_task_env_var_names(&mut env_vars, &[OZ_CLI_ENV, WARP_CLI_ENV], &cli_path);
    // The harness name is only consumed by child orchestration telemetry when the child
    // CLI emits `run message *` events.
    insert_task_env_var_names(
        &mut env_vars,
        &[OZ_HARNESS_ENV, WARP_HARNESS_ENV],
        selected_harness.to_string(),
    );
    if selected_harness == Harness::Claude && task_id.is_some() {
        insert_task_env_var_names(
            &mut env_vars,
            &[
                OZ_MESSAGE_LISTENER_MANAGED_EXTERNALLY_ENV,
                WARP_MESSAGE_LISTENER_MANAGED_EXTERNALLY_ENV,
                LEGACY_OZ_PARENT_LISTENER_MANAGED_EXTERNALLY_ENV,
            ],
            "1",
        );
        if let Some(state_root) = message_listener_state_root() {
            insert_task_env_var_names(
                &mut env_vars,
                &[
                    OZ_MESSAGE_LISTENER_STATE_ROOT_ENV,
                    WARP_MESSAGE_LISTENER_STATE_ROOT_ENV,
                    LEGACY_OZ_PARENT_STATE_ROOT_ENV,
                ],
                &state_root,
            );
        }
    }
    // Server URL overrides are disabled on release channels, so there's no
    // override to propagate to child processes there.
    if ChannelState::channel().allows_server_url_overrides() {
        insert_non_empty_task_env_var(
            &mut env_vars,
            SERVER_ROOT_URL_OVERRIDE_ENV,
            ChannelState::server_root_url().into_owned(),
        );
        insert_non_empty_task_env_var(
            &mut env_vars,
            WS_SERVER_URL_OVERRIDE_ENV,
            ChannelState::ws_server_url().into_owned(),
        );
        if let Some(url) = ChannelState::session_sharing_server_url()
            .map(Cow::into_owned)
            .filter(|url| !url.is_empty())
        {
            env_vars.insert(
                OsString::from(SESSION_SHARING_SERVER_URL_OVERRIDE_ENV),
                OsString::from(url),
            );
        }
    }

    env_vars
}

/// Drops every name under which the externally-managed-listener signal is injected.
///
/// The list must stay in step with the one `task_env_vars` writes: leaving
/// one name behind would tell the Claude plugin that Warp owns the listener when it does not.
/// `prepare_local_wake_command_rehydrates_transcript_with_self_managed_listener` asserts none
/// of them survive.
pub(crate) fn remove_claude_externally_managed_listener_env_vars(
    env_vars: &mut HashMap<OsString, OsString>,
) {
    for env_name in [
        OZ_MESSAGE_LISTENER_MANAGED_EXTERNALLY_ENV,
        WARP_MESSAGE_LISTENER_MANAGED_EXTERNALLY_ENV,
        LEGACY_OZ_PARENT_LISTENER_MANAGED_EXTERNALLY_ENV,
    ] {
        env_vars.remove(OsStr::new(env_name));
    }
}

/// Returns model environment variables for a local child harness.
pub(crate) fn harness_env_vars(
    selected_harness: Harness,
    third_party_harness_model_config: Option<&HarnessModelConfig>,
) -> Result<HashMap<OsString, OsString>> {
    let mut env_vars = HashMap::new();

    if selected_harness == Harness::Claude
        && let Some(model_id) = third_party_harness_model_config
            .map(|config| config.model_id.as_str())
            .filter(|id| !id.is_empty())
    {
        env_vars.insert(OsString::from("ANTHROPIC_MODEL"), OsString::from(model_id));
    }

    Ok(env_vars)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
