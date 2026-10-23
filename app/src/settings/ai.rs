//! Settings for Blocklist AI.
//!
//! These settings are currently used to configure the underlying model/API used to power the AI
//! UX, as well as small UX configurations.

use std::collections::HashMap;
use std::path::PathBuf;

use ::ai::api_keys::CustomEndpointDefinitions;
use chrono::{DateTime, Utc};
pub use cloud_object_models::{
    AgentModeCommandExecutionPredicate, DEFAULT_COMMAND_EXECUTION_ALLOWLIST,
    DEFAULT_COMMAND_EXECUTION_DENYLIST,
};
use serde::{Deserialize, Serialize};
use settings::{
    RespectUserSyncSetting, Setting, SupportedPlatforms, SyncToCloud, define_settings_group,
};
use strum_macros::EnumIter;
use warp_core::features::FeatureFlag;
use warp_errors::report_if_error;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, UpdateModel, WeakViewHandle};

use crate::ai::execution_profiles::ExecutionProfilesConfig;
use crate::ai::request_usage_model::RequestLimitInfo;
use crate::auth::AuthStateProvider;
use crate::settings::PrivacySettings;
use crate::terminal::TerminalView;
use crate::workspaces::user_workspaces::{TeamScope, UserWorkspaces};

pub enum FocusedTerminalInfoEvent {
    TerminalInfoUpdated,
}

/// Singleton model that is used to track the remote sessions in the terminal.
/// Useful for organizations that have restrictions on using AI in sessions in
/// remote sessions.
#[derive(Default, Clone, Debug)]
pub struct FocusedTerminalInfo {
    terminal: Option<WeakViewHandle<TerminalView>>,
    contains_any_remote_blocks: bool,
    contains_any_restored_remote_blocks: bool,
}

impl FocusedTerminalInfo {
    pub fn new(_: &mut ModelContext<Self>) -> Self {
        Self::default()
    }

    pub fn terminal(&self) -> Option<&WeakViewHandle<TerminalView>> {
        self.terminal.as_ref()
    }

    pub fn contains_any_remote_blocks(&self) -> bool {
        self.contains_any_remote_blocks
    }

    pub fn contains_any_restored_remote_blocks(&self) -> bool {
        self.contains_any_restored_remote_blocks
    }

    /// Records what the focused `terminal` contains, in a single atomic operation.
    /// Only emits a TerminalInfoUpdated event if anything changes.
    /// Returns true if the event was emitted.
    ///
    /// The surface is written together with its flags rather than tracked separately so a
    /// reader cannot resolve one terminal's team for another terminal's content.
    pub fn update(
        &mut self,
        terminal: WeakViewHandle<TerminalView>,
        contains_any_remote_blocks: bool,
        contains_any_restored_remote_blocks: bool,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let unchanged = self.terminal.as_ref().map(|held| held.id()) == Some(terminal.id())
            && self.contains_any_remote_blocks == contains_any_remote_blocks
            && self.contains_any_restored_remote_blocks == contains_any_restored_remote_blocks;
        if unchanged {
            return false;
        }

        self.terminal = Some(terminal);
        self.contains_any_remote_blocks = contains_any_remote_blocks;
        self.contains_any_restored_remote_blocks = contains_any_restored_remote_blocks;
        ctx.emit(FocusedTerminalInfoEvent::TerminalInfoUpdated);
        true
    }
}

impl Entity for FocusedTerminalInfo {
    type Event = FocusedTerminalInfoEvent;
}

impl SingletonEntity for FocusedTerminalInfo {}

/// Controls how agent thinking/reasoning traces are displayed after streaming.
#[derive(
    Default,
    Debug,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Copy,
    Clone,
    EnumIter,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "Controls how agent thinking is displayed after streaming.",
    rename_all = "snake_case"
)]
pub enum ThinkingDisplayMode {
    /// Show reasoning blocks while streaming, then collapse them when complete (default).
    #[default]
    ShowAndCollapse,
    /// Always keep reasoning blocks expanded, even after streaming finishes.
    AlwaysShow,
    /// Never show reasoning blocks.
    NeverShow,
}

settings::macros::implement_setting_for_enum!(
    ThinkingDisplayMode,
    AISettings,
    SupportedPlatforms::ALL,
    SyncToCloud::Globally(RespectUserSyncSetting::Yes),
    private: false,
    toml_path: "agents.warp_agent.other.thinking_display_mode",
    description: "Controls how agent thinking traces are displayed after streaming.",
);

impl ThinkingDisplayMode {
    /// Display name for the settings dropdown.
    pub fn display_name(&self) -> &'static str {
        match self {
            ThinkingDisplayMode::ShowAndCollapse => "Show & collapse",
            ThinkingDisplayMode::AlwaysShow => "Always show",
            ThinkingDisplayMode::NeverShow => "Never show",
        }
    }

    pub fn command_palette_description(&self) -> &'static str {
        match self {
            ThinkingDisplayMode::ShowAndCollapse => "Set agent thinking display: show & collapse",
            ThinkingDisplayMode::AlwaysShow => "Set agent thinking display: always show",
            ThinkingDisplayMode::NeverShow => "Set agent thinking display: never show",
        }
    }

    pub fn should_render(&self) -> bool {
        !matches!(self, ThinkingDisplayMode::NeverShow)
    }

    pub fn should_keep_expanded(&self) -> bool {
        matches!(self, ThinkingDisplayMode::AlwaysShow)
    }
}

/// Controls how child-agent message bodies are displayed.
#[derive(
    Default,
    Debug,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Copy,
    Clone,
    EnumIter,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "Controls how child-agent messages are displayed.",
    rename_all = "snake_case"
)]
pub enum OrchestrationMessageDisplayMode {
    /// Show child-agent messages while streaming, then collapse them.
    ShowAndCollapse,
    /// Keep child-agent message bodies expanded.
    AlwaysShow,
    /// Keep child-agent message bodies collapsed.
    #[default]
    AlwaysCollapse,
}

settings::macros::implement_setting_for_enum!(
    OrchestrationMessageDisplayMode,
    AISettings,
    SupportedPlatforms::ALL,
    SyncToCloud::Globally(RespectUserSyncSetting::Yes),
    private: false,
    toml_path: "agents.warp_agent.other.orchestration_message_display_mode",
    description: "Controls how child-agent messages are displayed.",
);

/// Unit for GUI usage and spend displays.
#[derive(
    Default,
    Debug,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Copy,
    Clone,
    EnumIter,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "Which unit the GUI's usage/spend displays show: credits or dollars.",
    rename_all = "snake_case"
)]
pub enum UsageDisplayUnit {
    #[default]
    Credits,
    Dollars,
}

settings::macros::implement_setting_for_enum!(
    UsageDisplayUnit,
    AISettings,
    SupportedPlatforms::ALL,
    SyncToCloud::Globally(RespectUserSyncSetting::Yes),
    private: false,
    toml_path: "agents.warp_agent.other.usage_display_unit",
    description: "Which unit the GUI's usage/spend displays show: credits or dollars.",
);

impl UsageDisplayUnit {
    pub fn display_name(&self) -> &'static str {
        match self {
            UsageDisplayUnit::Credits => "Credits",
            UsageDisplayUnit::Dollars => "Dollars",
        }
    }
}

impl OrchestrationMessageDisplayMode {
    /// Display name for the settings dropdown.
    pub fn display_name(&self) -> &'static str {
        match self {
            OrchestrationMessageDisplayMode::ShowAndCollapse => "Show & collapse",
            OrchestrationMessageDisplayMode::AlwaysShow => "Always show",
            OrchestrationMessageDisplayMode::AlwaysCollapse => "Always collapse",
        }
    }

    pub fn command_palette_description(&self) -> &'static str {
        match self {
            OrchestrationMessageDisplayMode::ShowAndCollapse => {
                "Set child-agent message display: show & collapse"
            }
            OrchestrationMessageDisplayMode::AlwaysShow => {
                "Set child-agent message display: always show"
            }
            OrchestrationMessageDisplayMode::AlwaysCollapse => {
                "Set child-agent message display: always collapse"
            }
        }
    }

    /// Whether child-agent message bodies should expand while streaming.
    pub fn should_expand_agent_message_body(&self) -> bool {
        matches!(
            self,
            OrchestrationMessageDisplayMode::ShowAndCollapse
                | OrchestrationMessageDisplayMode::AlwaysShow
        )
    }

    /// Whether child-agent message bodies should collapse after streaming.
    pub fn should_collapse_agent_message_body_on_finish(&self) -> bool {
        matches!(self, OrchestrationMessageDisplayMode::ShowAndCollapse)
    }
}

/// Controls what happens when a user submits a new prompt while the agent is
/// still responding to an earlier prompt.
///
/// This is the *default* used when a conversation has no explicit auto-queue
/// override. Per-conversation overrides live on `QueuedQueryModel` and take
/// precedence over this setting.
#[derive(
    Default,
    Debug,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Copy,
    Clone,
    EnumIter,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "Default behavior when submitting a new prompt while the agent is still responding.",
    rename_all = "snake_case"
)]
pub enum PromptSubmissionMode {
    /// Cancel the in-flight response and submit the new prompt immediately
    /// (default).
    #[default]
    Interrupt,
    /// Hold the new prompt until the in-flight response finishes, then submit.
    Queue,
}

settings::macros::implement_setting_for_enum!(
    PromptSubmissionMode,
    AISettings,
    SupportedPlatforms::ALL,
    SyncToCloud::Globally(RespectUserSyncSetting::Yes),
    private: false,
    toml_path: "agents.warp_agent.other.default_prompt_submission_mode",
    description: "Default behavior when submitting a new prompt while the agent is still responding.",
    feature_flag: FeatureFlag::QueueSlashCommand,
);

impl PromptSubmissionMode {
    /// Display name for the settings dropdown.
    pub fn display_name(&self) -> &'static str {
        match self {
            PromptSubmissionMode::Interrupt => "Interrupt response",
            PromptSubmissionMode::Queue => "Queue until response finishes",
        }
    }

    pub fn command_palette_description(&self) -> &'static str {
        match self {
            PromptSubmissionMode::Interrupt => "Set default prompt submission: interrupt response",
            PromptSubmissionMode::Queue => {
                "Set default prompt submission: queue until response finishes"
            }
        }
    }
}

/// What happens when a prompt is submitted while an agent controls an agent-requested
/// long-running command (LRC).
///
/// Only consulted when [`PromptSubmissionMode`] is `Interrupt`: in `Queue` mode
/// prompts always queue until the full response finishes, so this setting is
/// hidden and ignored.
#[derive(
    Default,
    Debug,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Copy,
    Clone,
    EnumIter,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "What happens when a prompt is submitted while an agent controls an agent-requested long-running command.",
    rename_all = "snake_case"
)]
pub enum LongRunningCommandSubmissionMode {
    /// Send the prompt to the agent immediately, steering it mid-command.
    SendImmediately,
    /// Queue the prompt and send it to the agent when the command finishes
    /// (default).
    #[default]
    QueueUntilCommandCompletes,
}

settings::macros::implement_setting_for_enum!(
    LongRunningCommandSubmissionMode,
    AISettings,
    SupportedPlatforms::ALL,
    SyncToCloud::Globally(RespectUserSyncSetting::Yes),
    private: false,
    toml_path: "agents.warp_agent.other.long_running_command_submission_mode",
    description: "What happens when a prompt is submitted while an agent controls an agent-requested long-running command.",
    feature_flag: FeatureFlag::QueueSlashCommand,
);

impl LongRunningCommandSubmissionMode {
    /// Display name for the settings dropdown.
    pub fn display_name(&self) -> &'static str {
        match self {
            LongRunningCommandSubmissionMode::SendImmediately => "Send immediately",
            LongRunningCommandSubmissionMode::QueueUntilCommandCompletes => {
                "Queue until command finishes"
            }
        }
    }

    pub fn command_palette_description(&self) -> &'static str {
        match self {
            LongRunningCommandSubmissionMode::SendImmediately => {
                "Set long-running command submission: send immediately"
            }
            LongRunningCommandSubmissionMode::QueueUntilCommandCompletes => {
                "Set long-running command submission: queue until command finishes"
            }
        }
    }
}

/// Tracks the state of the quota reset banner
#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    PartialEq,
    Default,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(description = "State of the quota reset banner.")]
pub struct BannerState {
    #[serde(default)]
    #[schemars(description = "Whether the banner has been dismissed.")]
    pub dismissed: bool,
}

/// Tracks information about a single billing cycle for AI request usage
#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    PartialEq,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(description = "Information about a single billing cycle.")]
pub struct CycleInfo {
    /// End date of the billing cycle
    #[schemars(description = "End date of the billing cycle.")]
    pub end_date: DateTime<Utc>,
    /// Whether the quota was exceeded in this cycle
    #[schemars(description = "Whether the usage quota was exceeded in this cycle.")]
    pub was_quota_exceeded: bool,
    /// State of the quota reset banner
    #[schemars(description = "State of the quota reset banner for this cycle.")]
    pub banner_state: BannerState,
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    Default,
    PartialEq,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(description = "AI usage quota information across billing cycles.")]
pub struct AIRequestQuotaInfo {
    /// History of billing cycles and their usage.
    ///
    /// Note that these are only populated going forward from when this setting
    /// was introduced.
    #[schemars(description = "History of billing cycles and their quota usage.")]
    pub cycle_history: Vec<CycleInfo>,
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    Default,
    PartialEq,
    EnumIter,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "File read permission level for the agent.",
    rename_all = "snake_case"
)]
pub enum AgentModeCodingPermissionsType {
    /// Agent Mode must ask for explicit permission for any type of file read.
    #[default]
    AlwaysAskBeforeReading,
    /// Agent Mode can always read files without explicit consent.
    AlwaysAllowReading,
    /// Agent Mode can only read certain files without explicit consent.
    ///
    /// The specific filepaths are backed by the
    /// [`AISettings::agent_mode_coding_file_read_allowlist`] setting.
    AllowReadingSpecificFiles,
}

define_settings_group!(AISettings, settings: [
    // If `false`, all AI features are disabled.
    is_any_ai_enabled: IsAnyAIEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: false,
        toml_path: "agents.warp_agent.is_any_ai_enabled",
        description: "Controls whether all AI features are enabled.",
    },
    // This field should not be referenced directly to lookup autodetection enablement -- use the
    // `is_ai_autodetection_enabled()` getter.
    ai_autodetection_enabled_internal: AIAutoDetectionEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.input.ai_auto_detection_enabled",
        description: "Controls whether AI automatically detects natural language input.",
    },
    // This field should not be referenced directly -- use the
    // `is_nld_in_terminal_enabled()` getter.
    // Controls whether natural language detection is enabled in the terminal input.
    //
    // This is only used when `FeatureFlag::AgentView` is enabled.
    nld_in_terminal_enabled_internal: NLDInTerminalEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.input.nld_in_terminal_enabled",
        description: "Controls whether natural language detection is enabled in the terminal input.",
    },
    autodetection_command_denylist: AICommandDenylist {
        type: String,
        default: String::new(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.input.ai_command_denylist",
        description: "Commands to exclude from AI natural language autodetection.",
    },
    // Predicates that Agent Mode can use to decide if it can execute
    // a command without explicit user consent.
    //
    // Prefer [`BlocklistAIPermissions::can_autoexecute_command`] to
    // interpret this allowlist.
    agent_mode_command_execution_allowlist: AgentModeCommandExecutionAllowlist {
        type: Vec<AgentModeCommandExecutionPredicate>,
        default: DEFAULT_COMMAND_EXECUTION_ALLOWLIST.clone(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.profiles.agent_mode_command_execution_allowlist",
        description: "Commands that the agent can execute without explicit permission.",
    },
    // Predicates that Agent Mode can use to decide if a command must
    // be executed by the user.
    //
    // Prefer [`BlocklistAIPermissions::can_autoexecute_command`] to
    // interpret this denylist.
    agent_mode_command_execution_denylist: AgentModeCommandExecutionDenylist {
        type: Vec<AgentModeCommandExecutionPredicate>,
        default: DEFAULT_COMMAND_EXECUTION_DENYLIST.clone(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.profiles.agent_mode_command_execution_denylist",
        description: "Commands that the agent must always ask before executing.",
    },
    // Enabled iff Agent Mode can execute readonly commands without explicit user consent.
    //
    // Prefer [`BlocklistAIPermissions::can_autoexecute_command`] to
    // interpret this setting.
    agent_mode_execute_read_only_commands: AgentModeExecuteReadonlyCommands {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.profiles.agent_mode_execute_readonly_commands",
        description: "Whether the agent can auto-execute read-only commands without asking.",
    },
    // Determines coding permissions that Agent Mode has.
    // Note that if Agent Mode has permissions to execute readonly commands,
    // that automatically gives Agent Mode the ability to also _read_ files for coding
    // tasks, including codebase search.
    //
    // Prefer [`BlocklistAIPermissions::can_read_file`] to interpret this setting.
    agent_mode_coding_permissions: AgentModeCodingPermissions {
        type: AgentModeCodingPermissionsType,
        default: AgentModeCodingPermissionsType::default(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.profiles.agent_mode_coding_permissions",
        description: "The file read permission level for the agent.",
    }
    // Specific filepaths that Agent Mode can read without asking for additional permissions.
    // These should be persisted as absolute filepaths to avoid ambiguity.
    //
    // This is used in conjunction with [`AgentModeCodingPermissionsType::AllowReadingSpecificFiles`]
    // but modelled as a separate setting because it is not cloud-synced.
    //
    // Prefer [`BlocklistAIPermissions::can_read_file`] to interpret this setting.
    agent_mode_coding_file_read_allowlist: AgentModeCodingFileReadAllowlist {
        type: Vec<PathBuf>,
        default: vec![],
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        toml_path: "agents.profiles.agent_mode_coding_file_read_allowlist",
        description: "File paths the agent can read without asking for permission.",
    }
    // The complete execution-profile collection shared by GUI and TUI.
    // GUI cloud synchronization respects the user's settings-sync preference;
    // TUI settings mode keeps this value local.
    execution_profiles: ExecutionProfiles {
        type: ExecutionProfilesConfig,
        default: ExecutionProfilesConfig::default(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.execution_profiles",
        max_table_depth: 2,
        description: "AI execution profiles and their permissions.",
    }
    // Non-secret custom inference endpoint definitions shared by GUI and TUI.
    // Credentials remain in each surface's secure-storage namespace.
    custom_endpoints: CustomEndpoints {
        type: CustomEndpointDefinitions,
        default: CustomEndpointDefinitions::default(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.custom_endpoints",
        max_table_depth: 2,
        description: "Custom inference endpoint definitions.",
    }
    usage_display_unit: UsageDisplayUnit,
    // Whether or not the profile-level command autoexecution speedbump has been shown.
    //
    // Not a user-visible setting - we model it as a setting so we can track how often
    // it's shown across devices.
    has_shown_agent_mode_profile_command_autoexecution_speedbump: HasShownAgentModeProfileCommandAutoexecutionSpeedbump {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    }
    // Whether or not we should show the speedbump for auto-executing readonly cmds.
    //
    // Not a user-visible settings - we model it as a setting so we can track how often
    // it's shown across devices.
    should_show_agent_mode_autoexecute_readonly_commands_speedbump: ShouldShowAgentModeModelExecuteReadonlyCommandsSpeedbump {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    }
    // Whether or not we should show the speedbump for auto-writing to the PTY.
    //
    // Not a user-visible settings - we model it as a setting so we can track how often
    // it's shown across devices.
    should_show_agent_mode_write_to_pty_speedbump: ShouldShowAgentModeWriteToPtySpeedbump {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    }
    // Whether or not we should show the speedbump for auto-reading files.
    //
    // Not a user-visible settings - we model it as a setting so we can track how often
    // it's shown across devices.
    should_show_agent_mode_autoread_files_speedbump: ShouldShowAgentModeCodingReadPermissionsNudge {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    }
    // Whether or not we should show the one-shot speedbump on Ask-User-Question cards.
    //
    // Not a user-visible setting - we model it as a setting so we can track state.
    // Intentionally NOT cloud-synced: we want users to see the first-time nudge on
    // each fresh device, and we avoid a cloud-sync race that would make the flag
    // silently stay `false` on new devices after being consumed once elsewhere.
    should_show_agent_mode_ask_user_question_speedbump: ShouldShowAgentModeAskUserQuestionSpeedbump {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: true,
    }
    // Whether to use locally loaded AWS credentials for Bedrock-enabled requests.
    aws_bedrock_credentials_enabled: AwsBedrockCredentialsEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "cloud_platform.third_party_api_keys.aws_bedrock_credentials_enabled",
        description: "Whether Warp should use your local AWS credentials for Bedrock-enabled requests.",
    }
    // Whether to automatically run the AWS login command when Bedrock credentials are expired.
    //
    // When true, the configured login command will be run automatically without asking.
    // When false (default), a prompt will be shown asking for permission.
    aws_bedrock_auto_login: AwsBedrockAutoLogin {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "cloud_platform.third_party_api_keys.aws_bedrock_auto_login",
        description: "Whether to automatically run the AWS login command when Bedrock credentials expire.",
    }
    // Command to run to refresh AWS credentials when using Bedrock auto-login.
    aws_bedrock_auth_refresh_command: AwsBedrockAuthRefreshCommand {
        type: String,
        default: "aws login".to_string(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "cloud_platform.third_party_api_keys.aws_bedrock_auth_refresh_command",
        description: "The command to run to refresh AWS credentials for Bedrock.",
    }
    // AWS profile name to use when loading credentials from the local AWS credential/config chain.
    aws_bedrock_profile: AwsBedrockProfile {
        type: String,
        default: "default".to_string(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "cloud_platform.third_party_api_keys.aws_bedrock_profile",
        description: "The AWS profile name to use for Bedrock credentials.",
    }
    // Whether the AWS Bedrock login banner has been permanently dismissed.
    //
    // Not a user-visible setting - we model it as a setting so we can track state.
    aws_bedrock_login_banner_dismissed: AwsBedrockLoginBannerDismissed {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    }
    // Whether to mint and attach Gemini Enterprise (GEAP) credentials to eligible agent
    // requests, routing them through the workspace's Google Cloud project. Only consulted
    // when the admin sets the GEAP host to RESPECT_USER_SETTING; ENFORCE bypasses it.
    // Prefer [`UserWorkspaces::is_gemini_enterprise_credentials_enabled`] to interpret
    // this setting.
    gemini_enterprise_credentials_enabled: GeminiEnterpriseCredentialsEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "cloud_platform.third_party_api_keys.gemini_enterprise_credentials_enabled",
        description: "Whether Warp should route eligible requests through your workspace's Gemini Enterprise Google Cloud project.",
    }
    // Whether or not the user wants agent mode requests to use their saved rules.
    memory_enabled: MemoryEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.knowledge.rules_enabled",
        description: "Whether the agent uses your saved rules during requests.",
    }
    // Whether warp drive context should be included in AI requests
    warp_drive_context_enabled: WarpDriveContextEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.knowledge.warp_drive_context_enabled",
        description: "Whether Warp Drive context is included in AI requests.",
    }

    // Whether the codebase speedbump banner has been permanently dismissed for a given repo path.
    //
    // Not a user-visible settings - we model it as a setting so we can track state.
    codebase_index_speedbump_banner_dismissed_for_repo_paths: CodebaseIndexSpeedbumpBannerDismissedForRepoPaths {
        type: Vec<PathBuf>,
        default: vec![],
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: true,
    }

    // Whether the agent mode setup banner has been shown for a given repo path.
    // Once shown, it will not be shown again for that repo.
    //
    // Not a user-visible settings - we model it as a setting so we can track state.
    agent_mode_setup_banner_shown_for_repo_paths: AgentModeSetupBannerShownForRepoPaths {
        type: Vec<PathBuf>,
        default: vec![],
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: true,
    }

    // Whether the codebase speedbump banner has been globally dismissed ("Don't show again").
    //
    // Not a user-visible settings - we model it as a setting so we can track state.
    codebase_index_speedbump_banner_globally_dismissed: CodebaseIndexSpeedbumpBannerGloballyDismissed {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    }

    // Information about AI request quotas and usage across billing cycles
    ai_request_quota_info: AIRequestQuotaInfoSetting {
        type: AIRequestQuotaInfo,
        default: AIRequestQuotaInfo::default(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    },

    mcp_execution_path: MCPExecutionPath {
        type: Option<String>,
        default: None,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: true,
    },

    // This is not a user-visible setting - its merely a one-time flag to track if the agents 3 launch modal
    // has been shown to the user.
    //
    // We model it as a setting so it's only shown once to a given user regardless of the number of
    // devices they use.
    did_check_to_trigger_agents_3_launch_modal: DidShowAgents3LaunchModal {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: true,
    }

    // This is not a user-visible setting - it's merely a one-time flag to track if the Oz launch modal
    // has been shown to the user.
    //
    // We model it as a setting so it's only shown once to a given user regardless of the number of
    // devices they use.
    did_check_to_trigger_oz_launch_modal: DidShowOzLaunchModal {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: true,
    }

    // This is not a user-visible setting - it's merely a one-time flag to track if the
    // orchestration launch modal has been shown to the user.
    //
    // We model it as a setting so it's only shown once to a given user regardless of the number of
    // devices they use.
    did_check_to_trigger_orchestration_launch_modal: DidShowOrchestrationLaunchModal {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: true,
    }

    // This is not a user-visible setting - it's merely a one-time flag to track if the
    // Warp Agent CLI launch modal has been shown to the user.
    //
    // We model it as a setting so it's only shown once to a given user regardless of the number of
    // devices they use.
    did_check_to_trigger_agent_cli_launch_modal: DidShowAgentCliLaunchModal {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: true,
    }

    // This is not a user-visible setting - it's merely a one-time flag to track if the
    // free-AI-removal notice modal has been shown to (or silently marked as seen for) the user.
    //
    // We model it as a setting so it's only shown once to a given user regardless of the number of
    // devices they use.
    did_check_to_trigger_free_ai_removal_modal: DidShowFreeAiRemovalModal {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: true,
    }

    // This is not a user-visible setting - it's merely a one-time flag to track if the
    // "You're using your ChatGPT plan" modal has been shown for the user's current ChatGPT
    // connection. Reset when the user disconnects so a later reconnect shows it again.
    //
    // We model it as a setting so it's only shown once to a given user regardless of the number of
    // devices they use.
    did_show_chatgpt_plan_modal: DidShowChatGPTPlanModal {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: true,
    }

    // Used to determine whether the "Latest updates" section of the agent view
    // zero state is expanded or collapsed by default.
    should_expand_oz_updates: ShouldExpandOzUpdates {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: true,
    }

    // Used to determine whether the "Latest updates" section of the agent view
    // zero state is shown or hidden.
    should_show_oz_updates_in_zero_state: ShouldShowOzUpdatesInZeroState {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.should_show_oz_updates_in_zero_state",
        description: "Whether the \"What's new\" section is shown in the agent view.",
    }

    // Whether or not the user has enabled fallback to Warp credits for user-provided models.
    can_use_warp_credits_for_fallback: CanUseWarpCreditsForFallback {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        storage_key: "CanUseWarpCreditsWithByok",
        toml_path: "cloud_platform.third_party_api_keys.can_use_warp_credits_with_byok",
        description: "Whether Warp credits can be used as a fallback for user-provided models.",
    }

    should_render_use_agent_footer_for_user_commands: ShouldRenderUseAgentToolbarForUserCommands {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.should_render_use_agent_toolbar_for_user_commands",
        description: "Whether to show the \"Use Agent\" footer for terminal commands.",
    }

    // This is not a user-visible setting - it tracks whether a paid user has dismissed the
    // agent management help page by clicking "View Agents".
    //
    // When false and user is on a paid plan, the help page is shown.
    // When true, the help page is hidden (user dismissed it).
    // Free users never see the help page by default regardless of this setting.
    did_dismiss_cloud_setup_guide: DidDismissAgentManagementHelpPage {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    }

    // Whether the ambient agent trial widget has been dismissed by the user.
    //
    // Not a user-visible setting - we model it as a setting so we can track state.
    ambient_agent_trial_widget_dismissed: AmbientAgentTrialWidgetDismissed {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
    }

    // Whether computer use is enabled for cloud agent conversations started from the Warp app.
    // This setting is only used when the AI autonomy setting is AlwaysAsk or not set.
    cloud_agent_computer_use_enabled: CloudAgentComputerUseEnabled {
        type: bool,
        default: warp_core::channel::ChannelState::channel().is_dogfood(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.cloud_agent_computer_use_enabled",
        description: "Whether computer use is enabled for cloud agent conversations.",
    }


    // Whether file-based MCP servers from third-party AI tools (e.g. Claude, Codex) should
    // be automatically detected and spawned. Warp-native config files (.warp/.mcp.json) are
    // always detected and spawned, regardless of this setting.
    file_based_mcp_enabled: FileBasedMcpEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.mcp_servers.file_based_mcp_enabled",
        description: "Whether third-party file-based MCP servers are automatically detected.",
    }

    // Controls how agent thinking/reasoning traces are displayed.
    thinking_display_mode: ThinkingDisplayMode,

    // Controls how orchestration message bodies are expanded by default.
    orchestration_message_display_mode: OrchestrationMessageDisplayMode,

    // Default behavior when the user submits a new prompt while the agent is still
    // responding. Per-conversation overrides live on `QueuedQueryModel`; this
    // setting is the fallback used when a conversation has no explicit override.
    default_prompt_submission_mode: PromptSubmissionMode,

    // What happens when a prompt is submitted while an agent controls an agent-requested
    // long-running command. Only consulted when `default_prompt_submission_mode` is `Interrupt`;
    // per-LRC manual overrides live on `QueuedQueryModel`.
    long_running_command_submission_mode: LongRunningCommandSubmissionMode,

    // Whether agent-executed shell commands should be included in command history
    // (up-arrow, Ctrl-R search, inline history menu).
    // When false, commands run by the AI agent are excluded from history.
    include_agent_commands_in_history: IncludeAgentCommandsInHistory {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.input.include_agent_commands_in_history",
        description: "Whether agent-executed commands are included in command history.",
    }

    // Whether fast forward / auto-approve can run commands that match the command denylist.
    auto_approve_bypasses_command_denylist: AutoApproveBypassesCommandDenylist {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.auto_approve_bypasses_command_denylist",
        description: "Whether auto-approve bypasses the command denylist.",
    }

    // Controls whether the conversation history view appears in the tools panel.
    show_conversation_history: ShowConversationHistory {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.show_conversation_history",
        description: "Whether conversation history appears in the tools panel.",
    }


    // Controls whether agent notifications (mailbox button, toasts, notification items) are shown.
    show_agent_notifications: ShowAgentNotifications {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.show_agent_notifications",
        description: "Whether agent notifications are shown.",
    }

    // Whether Oz should add attribution (co-author line) to commit messages and PRs.
    // This is the user-level preference; it may be overridden by the window's team's
    // `enable_warp_attribution` AdminEnablementSetting (see
    // `UserWorkspaces::get_agent_attribution_setting`).
    agent_attribution_enabled: AgentAttributionEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: false,
        toml_path: "agents.warp_agent.other.agent_attribution_enabled",
        description: "Whether the Warp Agent adds an attribution co-author line to commit messages and pull requests it creates.",
    }

    should_force_disable_cloud_handoff: ShouldForceDisableCloudHandoff {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.should_force_disable_cloud_handoff",
        description: "Whether to force-disable local-to-cloud handoff.",
    }

    should_force_disable_ampersand_handoff: ShouldForceDisableAmpersandHandoff {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.should_force_disable_ampersand_handoff",
        description: "Whether to force-disable the & prefix for cloud handoff compose mode.",
    }

    auto_handoff_on_sleep_enabled: AutoHandoffOnSleepEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::MAC,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: false,
        toml_path: "agents.warp_agent.other.auto_handoff_on_sleep_enabled",
        description: "Whether Warp automatically hands off local agent conversations to cloud when the computer is about to sleep.",
    }

    // This is not a user-visible setting - it's merely a one-time flag to track if the
    // auto-handoff sleep modal has been shown to the user.
    //
    // We model it as a setting so it's only shown once to a given user regardless of the number of
    // devices they use.
    did_show_auto_handoff_sleep_modal: DidShowAutoHandoffSleepModal {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: true,
    }

    // Not a user-visible setting - it tracks which one-time feature-intro popups the
    // user has already seen, keyed by the feature-intro id (see `FEATURE_INTROS`).
    //
    // We model it as a globally-synced setting (not respecting the user's sync setting)
    // so each feature is announced at most once per user, regardless of how many devices
    // they use. A feature is considered seen when its id is present and mapped to `true`.
    seen_feature_intro_ids: SeenFeatureIntroIds {
        type: HashMap<String, bool>,
        default: HashMap::default(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        private: true,
    }
]);

impl AISettings {
    pub fn register_and_subscribe_to_events(app: &mut AppContext) {
        Self::register(app);
        app.add_singleton_model(FocusedTerminalInfo::new);

        app.update_model(&Self::handle(app), |_me, ctx| {
            ctx.subscribe_to_model(&FocusedTerminalInfo::handle(ctx), |_me, _, event, ctx| {
                if matches!(event, FocusedTerminalInfoEvent::TerminalInfoUpdated) {
                    // Pipe the event so that any view that listens for settings changes will be notified.
                    ctx.emit(AISettingsChangedEvent::IsAnyAIEnabled {
                        change_event_reason: ChangeEventReason::LocalChange,
                    });
                }
            });
        });
    }

    pub fn is_ai_disabled_due_to_remote_session_org_policy(&self, app: &AppContext) -> bool {
        let focused_terminal = FocusedTerminalInfo::as_ref(app);
        let Some(terminal) = focused_terminal.terminal() else {
            return false;
        };
        if !focused_terminal.contains_any_remote_blocks()
            && !focused_terminal.contains_any_restored_remote_blocks()
        {
            return false;
        }

        let user_workspaces = UserWorkspaces::as_ref(app);
        let scope = user_workspaces.team_context(terminal, app);
        !user_workspaces.is_ai_allowed_in_remote_sessions(&scope)
    }

    pub fn is_any_ai_enabled(&self, app: &AppContext) -> bool {
        // Disable AI for anonymous and logged-out users.
        let is_anonymous_or_logged_out = AuthStateProvider::as_ref(app)
            .get()
            .is_anonymous_or_logged_out();

        *self.is_any_ai_enabled
            && !is_anonymous_or_logged_out
            && !self.is_ai_disabled_due_to_remote_session_org_policy(app)
    }

    /// Returns whether conversation history is available for the current
    /// account and AI state.
    ///
    /// The stored `show_conversation_history` preference is kept separately so
    /// an onboarding choice can take effect automatically after signup and AI
    /// enablement without asking the user to toggle the setting again.
    pub fn is_conversation_history_available(&self, app: &AppContext) -> bool {
        self.is_any_ai_enabled(app)
    }

    /// Returns whether conversation history should currently appear in the
    /// tools panel.
    pub fn is_conversation_history_enabled(&self, app: &AppContext) -> bool {
        *self.show_conversation_history && self.is_conversation_history_available(app)
    }

    /// Returns `true` if input autodetection is enabled.
    ///
    /// If `FeatureFlag::AgentView` is enabled, this specifically gates NLD enablement in the agent
    /// view only.
    pub fn is_ai_autodetection_enabled(&self, app: &warpui::AppContext) -> bool {
        self.is_any_ai_enabled(app) && *self.ai_autodetection_enabled_internal
    }

    /// Returns `true` if NLD is enabled in the terminal.
    ///
    /// This is only used when `FeatureFlag::AgentView` is enabled.
    /// If the user has not explicitly set this setting, it defaults to the value of
    /// `ai_autodetection_enabled_internal`.
    pub fn is_nld_in_terminal_enabled(&self, app: &warpui::AppContext) -> bool {
        self.is_any_ai_enabled(app) && *self.nld_in_terminal_enabled_internal
    }

    pub fn is_memory_enabled(&self, app: &warpui::AppContext) -> bool {
        self.is_any_ai_enabled(app) && *self.memory_enabled
    }

    pub fn is_warp_drive_context_enabled(&self, app: &warpui::AppContext) -> bool {
        self.is_any_ai_enabled(app) && *self.warp_drive_context_enabled
    }

    pub fn is_file_based_mcp_enabled(&self, app: &warpui::AppContext) -> bool {
        if !FeatureFlag::FileBasedMcp.is_enabled() || !self.is_any_ai_enabled(app) {
            return false;
        }
        // NOTE: we intentionally do not force-enable this in Cloud Mode. Previously
        // we auto-spawned file-based MCPs in autonomous execution, but that bypassed
        // the user's explicit opt-in and let any MCP config checked into a repo run
        // arbitrary commands as part of a cloud agent run. Respecting the toggle
        // closes that attack surface; cloud agents that need project-scoped MCP
        // servers should surface an explicit, auditable opt-in. A more robust
        // solution (e.g. per-environment allowlisting, signed configs) should be
        // explored in the future.
        *self.file_based_mcp_enabled
    }

    pub fn is_orchestration_enabled(&self, app: &warpui::AppContext) -> bool {
        self.is_any_ai_enabled(app)
    }

    /// Returns true when local-to-cloud handoff is effectively enabled.
    /// False when the user/org has disabled it, cloud conversations are off,
    /// or AI is globally off.
    pub fn is_cloud_handoff_enabled(&self, app: &warpui::AppContext) -> bool {
        if !self.is_any_ai_enabled(app) || *self.should_force_disable_cloud_handoff {
            return false;
        }
        if !FeatureFlag::OzHandoff.is_enabled()
            || !FeatureFlag::HandoffLocalCloud.is_enabled()
            || !cfg!(all(feature = "local_fs", not(target_family = "wasm")))
        {
            return false;
        }
        let privacy = PrivacySettings::as_ref(app);
        if !privacy.is_cloud_conversation_storage_enabled {
            return false;
        }
        !matches!(
            UserWorkspaces::as_ref(app).get_cloud_conversation_storage_enablement_setting(),
            crate::workspaces::workspace::AdminEnablementSetting::Disable
        )
    }
    pub fn is_ampersand_handoff_enabled(&self, app: &warpui::AppContext) -> bool {
        self.is_cloud_handoff_enabled(app) && !*self.should_force_disable_ampersand_handoff
    }

    pub fn is_auto_handoff_on_sleep_enabled(&self, app: &warpui::AppContext) -> bool {
        self.is_cloud_handoff_enabled(app)
            && self
                .auto_handoff_on_sleep_enabled
                .is_supported_on_current_platform()
            && *self.auto_handoff_on_sleep_enabled
    }

    /// Determines whether a quota reset banner should be displayed to the user.
    ///
    /// The banner should be shown if the most recent completed billing cycle had
    /// quota exceeded and the banner was not manually dismissed.
    pub fn should_display_quota_reset_banner(&self) -> bool {
        let quota_info = &self.ai_request_quota_info;

        let most_recent_completed_cycle = quota_info
            .cycle_history
            .iter()
            .rev()
            .find(|cycle| cycle.end_date < Utc::now());

        if let Some(cycle) = most_recent_completed_cycle
            && cycle.was_quota_exceeded
            && !cycle.banner_state.dismissed
        {
            return true;
        }

        false
    }

    /// Marks the banner as dismissed for all completed cycles.
    pub fn mark_quota_banner_as_dismissed(&mut self, ctx: &mut ModelContext<Self>) {
        let mut cycle_history = self.ai_request_quota_info.cycle_history.clone();

        for cycle in cycle_history.iter_mut() {
            if cycle.end_date < Utc::now() {
                cycle.banner_state.dismissed = true;
            }
        }

        report_if_error!(
            self.ai_request_quota_info
                .set_value(AIRequestQuotaInfo { cycle_history }, ctx)
        );
    }

    /// Updates the quota info based on the latest RequestLimitInfo.
    ///
    /// This method finds or creates the appropriate CycleInfo based on the
    /// request_limit_info's next refresh time and updates its fields accordingly.
    pub fn update_quota_info(
        &mut self,
        request_limit_info: &RequestLimitInfo,
        ctx: &mut ModelContext<Self>,
    ) {
        // Convert ServerTimestamp to DateTime<Utc>
        let next_refresh_time = request_limit_info.next_refresh_time.utc();
        let now = Utc::now();

        // Check if request_limit_info has unlimited requests
        let is_quota_exceeded = !request_limit_info.is_unlimited
            && request_limit_info.num_requests_used_since_refresh >= request_limit_info.limit;

        let mut cycle_history = self.ai_request_quota_info.cycle_history.clone();

        // Track if we updated an existing cycle
        let mut updated_existing_cycle = false;

        // Find or create a cycle that matches the current period
        if let Some(current_cycle) = cycle_history.last_mut()
            && now <= current_cycle.end_date
        {
            // Update existing cycle
            current_cycle.was_quota_exceeded = is_quota_exceeded;
            updated_existing_cycle = true;
        }

        // Only create a new cycle if we didn't update an existing one
        if !updated_existing_cycle {
            // Create a new cycle
            let new_cycle = CycleInfo {
                end_date: next_refresh_time,
                was_quota_exceeded: is_quota_exceeded,
                banner_state: BannerState::default(),
            };

            cycle_history.push(new_cycle);
        }

        report_if_error!(
            self.ai_request_quota_info
                .set_value(AIRequestQuotaInfo { cycle_history }, ctx)
        );
    }

    pub fn is_command_denylist_editable(&self, app: &AppContext) -> bool {
        self.is_any_ai_enabled(app)
    }

    pub(crate) fn is_command_allowlist_editable(
        &self,
        scope: &impl TeamScope,
        app: &AppContext,
    ) -> bool {
        let set_by_workspace = UserWorkspaces::as_ref(app)
            .ai_autonomy_settings(scope)
            .has_override_for_execute_commands_allowlist();

        self.is_any_ai_enabled(app) && !set_by_workspace
    }

    pub(crate) fn is_directory_allowlist_editable(
        &self,
        scope: &impl TeamScope,
        app: &AppContext,
    ) -> bool {
        let set_by_workspace = UserWorkspaces::as_ref(app)
            .ai_autonomy_settings(scope)
            .has_override_for_read_files_allowlist();

        self.is_any_ai_enabled(app) && !set_by_workspace
    }

    pub(crate) fn is_execute_commands_permissions_editable(
        &self,
        scope: &impl TeamScope,
        app: &AppContext,
    ) -> bool {
        let set_by_workspace = UserWorkspaces::as_ref(app)
            .ai_autonomy_settings(scope)
            .has_override_for_execute_commands();

        self.is_any_ai_enabled(app) && !set_by_workspace
    }

    pub(crate) fn is_write_to_pty_permissions_editable(
        &self,
        scope: &impl TeamScope,
        app: &AppContext,
    ) -> bool {
        let set_by_workspace = UserWorkspaces::as_ref(app)
            .ai_autonomy_settings(scope)
            .has_override_for_write_to_pty();
        self.is_any_ai_enabled(app) && !set_by_workspace
    }

    pub(crate) fn is_computer_use_permissions_editable(
        &self,
        scope: &impl TeamScope,
        app: &AppContext,
    ) -> bool {
        let set_by_workspace = UserWorkspaces::as_ref(app)
            .ai_autonomy_settings(scope)
            .has_override_for_computer_use();
        self.is_any_ai_enabled(app) && !set_by_workspace
    }

    pub(crate) fn is_read_files_permissions_editable(
        &self,
        scope: &impl TeamScope,
        app: &AppContext,
    ) -> bool {
        let set_by_workspace = UserWorkspaces::as_ref(app)
            .ai_autonomy_settings(scope)
            .has_override_for_read_files();

        self.is_any_ai_enabled(app) && !set_by_workspace
    }

    pub(crate) fn is_code_diffs_permissions_editable(
        &self,
        scope: &impl TeamScope,
        app: &AppContext,
    ) -> bool {
        let set_by_workspace = UserWorkspaces::as_ref(app)
            .ai_autonomy_settings(scope)
            .has_override_for_code_diffs();

        self.is_any_ai_enabled(app) && !set_by_workspace
    }

    pub fn is_ask_user_question_permissions_editable(&self, app: &AppContext) -> bool {
        self.is_any_ai_enabled(app)
    }

    pub fn is_mcp_permission_editable(&self, app: &AppContext) -> bool {
        // TODO: Allow workspace overrides on MCP permissions.
        self.is_any_ai_enabled(app)
    }

    pub fn is_run_agents_permissions_editable(&self, app: &AppContext) -> bool {
        self.is_orchestration_enabled(app)
    }

    /// Whether the feature-intro popover with the given id key has been seen.
    pub fn is_feature_intro_seen(&self, key: &str) -> bool {
        self.seen_feature_intro_ids
            .get(key)
            .copied()
            .unwrap_or(false)
    }

    /// Records that the feature-intro popover with the given id key has been seen,
    /// so it is never shown again. No-op if already recorded.
    pub fn mark_feature_intro_seen(&mut self, key: &str, ctx: &mut ModelContext<Self>) {
        if self.is_feature_intro_seen(key) {
            return;
        }
        let mut map = self.seen_feature_intro_ids.clone();
        map.insert(key.to_owned(), true);
        report_if_error!(self.seen_feature_intro_ids.set_value(map, ctx));
    }
}

#[cfg(test)]
#[path = "ai_tests.rs"]
mod tests;
