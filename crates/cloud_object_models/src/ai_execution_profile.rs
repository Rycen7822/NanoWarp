use std::path::PathBuf;

use ai::LLMId;
use cloud_objects::cloud_object::{
    GenericCloudObject, GenericServerObject, GenericStringModel, JsonObjectType,
};
use cloud_objects::ids::GenericStringObjectId;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use warp_cli::agent::Harness;

use crate::{JsonModel, JsonSerializer};

pub const PROFILE_NAME_MAX_LENGTH: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionPermission {
    AgentDecides,
    AlwaysAllow,
    AlwaysAsk,
    // This is intended to catch deserialization errors whenever we add new variants to this enum. Say we
    // want to add a "Never" variant. Without this catch-all, old clients wouldn't be able to deserialize
    // a "Never" into one of the existing options.
    #[serde(other)]
    Unknown,
}

impl ActionPermission {
    pub fn description(&self) -> &'static str {
        match self {
            ActionPermission::AgentDecides | ActionPermission::Unknown => {
                "The Agent chooses the safest path: acting on its own when confident, and asking for approval when uncertain."
            }
            ActionPermission::AlwaysAllow => {
                "Give the Agent full autonomy  — no manual approval ever required."
            }
            ActionPermission::AlwaysAsk => {
                "Require explicit approval before the Agent takes any action."
            }
        }
    }

    pub fn is_always_ask(&self) -> bool {
        matches!(self, Self::AlwaysAsk)
    }

    pub fn is_always_allow(&self) -> bool {
        matches!(self, Self::AlwaysAllow)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteToPtyPermission {
    // This is for backwards compatibility with the old "Never" value.
    #[serde(alias = "Never")]
    AlwaysAllow,
    #[default]
    AlwaysAsk,
    AskOnFirstWrite,
    // This is intended to catch deserialization errors whenever we add new variants to this enum.
    #[serde(other)]
    Unknown,
}

impl WriteToPtyPermission {
    pub fn description(&self) -> &'static str {
        match self {
            WriteToPtyPermission::AlwaysAllow => ActionPermission::AlwaysAllow.description(),
            WriteToPtyPermission::AskOnFirstWrite => {
                "The agent will ask for permission the first time it needs to interact with a running command. After that, it will continue automatically for the rest of that command."
            }
            WriteToPtyPermission::AlwaysAsk => {
                "The agent will always ask for permission to interact with a running command."
            }
            WriteToPtyPermission::Unknown => ActionPermission::Unknown.description(),
        }
    }

    pub fn is_always_allow(&self) -> bool {
        matches!(self, Self::AlwaysAllow)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AskUserQuestionPermission {
    /// Never pause; skip questions and continue with best judgment.
    Never,
    /// Pause and wait for the user, unless auto-approve mode is enabled.
    AskExceptInAutoApprove,
    /// Always pause and wait for the user to answer before continuing, even in auto-approve mode.
    #[default]
    AlwaysAsk,

    // This is intended to catch deserialization errors whenever we add new variants to this enum.
    #[serde(other)]
    Unknown,
}

impl AskUserQuestionPermission {
    pub fn label(&self) -> &'static str {
        match self {
            AskUserQuestionPermission::Never => "Never ask",
            AskUserQuestionPermission::AskExceptInAutoApprove => "Ask unless auto-approve",
            AskUserQuestionPermission::AlwaysAsk | AskUserQuestionPermission::Unknown => {
                "Always ask"
            }
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            AskUserQuestionPermission::AskExceptInAutoApprove
            | AskUserQuestionPermission::Unknown => {
                "The Agent may ask a question and pause for your response, but will continue automatically when auto-approve is on."
            }
            AskUserQuestionPermission::Never => {
                "The Agent will not ask questions and will continue with its best judgment."
            }
            AskUserQuestionPermission::AlwaysAsk => {
                "The Agent may ask a question and will pause for your response even when auto-approve is on."
            }
        }
    }
}

/// Predicate types to match commands that can be executed by Agent Mode.
#[derive(Debug, Serialize, Deserialize, Clone)]
enum AgentModeCommandExecutionPredicateType {
    /// A regex with start (`^`) and end (`$`) anchors.
    ///
    /// We want regex rules to apply to the entire cmd string so we anchor them
    /// (there isn't any efficient way to apply to the entire cmd string at match-time).
    #[serde(with = "serde_regex")]
    AnchoredRegex(Regex),
}

impl AgentModeCommandExecutionPredicateType {
    fn new_regex(regex: &str) -> Result<Self, regex::Error> {
        // Redundant anchors aren't a problem so we can unconditionally add them.
        let anchored_regex = Regex::new(&format!("^{regex}$"))?;
        Ok(Self::AnchoredRegex(anchored_regex))
    }

    fn matches(&self, cmd: &str) -> bool {
        match self {
            Self::AnchoredRegex(regex) => regex.is_match(cmd),
        }
    }
}

impl PartialEq for AgentModeCommandExecutionPredicateType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnchoredRegex(a), Self::AnchoredRegex(b)) => {
                // Indexing should be safe since they're guaranteed to have at least
                // the anchors around them.
                let a_unanchored = &a.as_str()[1..a.as_str().len() - 1];
                let b_unanchored = &b.as_str()[1..b.as_str().len() - 1];
                a_unanchored == b_unanchored
            }
        }
    }
}

impl std::fmt::Display for AgentModeCommandExecutionPredicateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnchoredRegex(regex) => {
                write!(f, "{}", &regex.as_str()[1..regex.as_str().len() - 1])
            }
        }
    }
}

/// A wrapper around [`AgentModeCommandExecutionPredicateType`] to enforce
/// the use of the provided constructors rather than direct construction of the variants.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(transparent)]
pub struct AgentModeCommandExecutionPredicate(AgentModeCommandExecutionPredicateType);

impl schemars::JsonSchema for AgentModeCommandExecutionPredicate {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("AgentModeCommandExecutionPredicate")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<String>()
    }
}

impl AgentModeCommandExecutionPredicate {
    pub fn new_regex(regex: &str) -> Result<Self, regex::Error> {
        Ok(Self(AgentModeCommandExecutionPredicateType::new_regex(
            regex,
        )?))
    }

    pub fn matches(&self, cmd: &str) -> bool {
        self.0.matches(cmd)
    }
}

impl std::fmt::Display for AgentModeCommandExecutionPredicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl settings_value::SettingsValue for AgentModeCommandExecutionPredicate {
    fn to_file_value(&self) -> serde_json::Value {
        serde_json::Value::String(self.to_string())
    }

    fn from_file_value(value: &serde_json::Value) -> Option<Self> {
        value.as_str().and_then(|s| Self::new_regex(s).ok())
    }
}

lazy_static! {
    static ref OPTIONAL_ARGS_REGEX: Regex =
        Regex::new(r"(\s.*)?").expect("Can parse optional args regex");
}

cfg_if::cfg_if! {
    if #[cfg(test)] {
        lazy_static! {
            // Compiling the regexes for the default command execution allowlist/denylist can be slow
            // in an unoptimized build, so we use empty lists in unit tests.
            pub static ref DEFAULT_COMMAND_EXECUTION_ALLOWLIST: Vec<AgentModeCommandExecutionPredicate> = vec![];
            pub static ref DEFAULT_COMMAND_EXECUTION_DENYLIST: Vec<AgentModeCommandExecutionPredicate> = vec![];
        }
    } else {
        lazy_static! {
            pub static ref DEFAULT_COMMAND_EXECUTION_ALLOWLIST: Vec<AgentModeCommandExecutionPredicate> = vec![
                AgentModeCommandExecutionPredicate::new_regex(&format!("cat{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default cat rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("echo{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default echo rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex("find .*").expect("Can parse default find rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("grep{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default grep rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("ls{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default ls rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex("which .*").expect("Can parse default which rule into regex"),
            ];

            pub static ref DEFAULT_COMMAND_EXECUTION_DENYLIST: Vec<AgentModeCommandExecutionPredicate> = vec![
                AgentModeCommandExecutionPredicate::new_regex(&format!("bash{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default bash rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("fish{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default fish rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("pwsh{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default pwsh rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("sh{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default sh rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("zsh{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default zsh rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("curl{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default curl rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("eval{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default eval rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("exec{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default exec rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("source{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default source rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("wget{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default wget rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("dig{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default dig rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("nslookup{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default nslookup rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("host{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default host rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("ssh{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default ssh rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("scp{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default scp rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("rsync{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default rsync rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("telnet{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default telnet rule into regex"),
                AgentModeCommandExecutionPredicate::new_regex(&format!("rm{}", OPTIONAL_ARGS_REGEX.as_str())).expect("Can parse default rm rule into regex"),
            ];
        }
    }
}

/// Core data structure representing an AI execution profile, which includes model configuration,
/// behavior settings, and permissions.
///
/// NOTE: `planning_model` was removed after planning via subagent was deprecated; serialized legacy
/// profiles may include a `planning_model` field and this field name should remain reserved
/// indefinitely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AIExecutionProfile {
    pub name: String,
    pub is_default_profile: bool,
    pub apply_code_diffs: ActionPermission,
    pub read_files: ActionPermission,

    pub execute_commands: ActionPermission,
    pub write_to_pty: WriteToPtyPermission,
    pub ask_user_question: AskUserQuestionPermission,

    /// Always ask for permission for these commands
    pub command_denylist: Vec<AgentModeCommandExecutionPredicate>,

    /// When the execute_commands is set to AlwaysAsk, autoexecute these commands
    pub command_allowlist: Vec<AgentModeCommandExecutionPredicate>,

    /// When the read_files is set to AlwaysAsk, autoread from these directories
    pub directory_allowlist: Vec<PathBuf>,

    pub base_model: Option<LLMId>,
    pub coding_model: Option<LLMId>,
    pub cli_agent_model: Option<LLMId>,

    pub context_window_limit: Option<u32>,

    /// Whether the agent may use web search when helpful for completing tasks
    pub web_search_enabled: bool,
}

impl Default for AIExecutionProfile {
    fn default() -> Self {
        Self {
            name: Default::default(),
            is_default_profile: false,
            apply_code_diffs: ActionPermission::AgentDecides,
            read_files: ActionPermission::AgentDecides,
            execute_commands: ActionPermission::AlwaysAsk,
            write_to_pty: WriteToPtyPermission::AlwaysAsk,
            ask_user_question: AskUserQuestionPermission::AlwaysAsk,
            command_denylist: DEFAULT_COMMAND_EXECUTION_DENYLIST.clone(),
            command_allowlist: Vec::new(),
            directory_allowlist: Vec::new(),
            base_model: None,
            coding_model: None,
            cli_agent_model: None,
            context_window_limit: None,
            web_search_enabled: true,
        }
    }
}

impl AIExecutionProfile {
    /// Creates the TUI default while retaining every shared profile default
    /// except command execution approval.
    pub fn default_profile_for_tui() -> Self {
        Self {
            execute_commands: ActionPermission::AgentDecides,
            ..Default::default()
        }
    }

    #[cfg(feature = "agent_mode_evals")]
    pub fn create_agent_mode_eval_profile() -> Self {
        Self {
            name: "Agent Mode Eval".to_string(),
            is_default_profile: false,
            apply_code_diffs: ActionPermission::AlwaysAllow,
            read_files: ActionPermission::AlwaysAllow,
            execute_commands: ActionPermission::AlwaysAllow,
            write_to_pty: WriteToPtyPermission::AlwaysAllow,
            ask_user_question: AskUserQuestionPermission::Never,
            command_denylist: Vec::new(),
            command_allowlist: Vec::new(),
            directory_allowlist: Vec::new(),
            base_model: None,
            coding_model: None,
            cli_agent_model: None,
            context_window_limit: None,
            web_search_enabled: true,
        }
    }

    /// This creates a CLI-specific profile that will never ask the user for permission,
    /// since we cannot do so in a non-interactive setting.
    pub fn create_default_cli_profile(is_sandboxed: bool) -> Self {
        let command_denylist = if is_sandboxed {
            Vec::new()
        } else {
            DEFAULT_COMMAND_EXECUTION_DENYLIST.to_vec()
        };

        Self {
            name: "Default (CLI)".to_owned(),
            is_default_profile: true,
            apply_code_diffs: ActionPermission::AlwaysAllow,
            read_files: ActionPermission::AlwaysAllow,
            execute_commands: ActionPermission::AlwaysAllow,
            write_to_pty: WriteToPtyPermission::AlwaysAllow,
            ask_user_question: AskUserQuestionPermission::Never,
            command_denylist,
            command_allowlist: DEFAULT_COMMAND_EXECUTION_ALLOWLIST.to_vec(),
            directory_allowlist: Vec::new(),
            base_model: None,
            coding_model: None,
            cli_agent_model: None,
            context_window_limit: None,
            web_search_enabled: true,
        }
    }
}

impl JsonModel for AIExecutionProfile {
    fn json_object_type() -> JsonObjectType {
        JsonObjectType::AIExecutionProfile
    }
}

pub type CloudAIExecutionProfile =
    GenericCloudObject<GenericStringObjectId, CloudAIExecutionProfileModel>;
pub type CloudAIExecutionProfileModel = GenericStringModel<AIExecutionProfile, JsonSerializer>;
pub type ServerAIExecutionProfile =
    GenericServerObject<GenericStringObjectId, CloudAIExecutionProfileModel>;

/// Runtime configuration snapshot for agent execution.
///
/// This is the merged/resolved config used when spawning or running an agent.
/// It combines settings from config files and CLI args.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentConfigSnapshot {
    /// Config name for searchability/traceability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_prompt: Option<String>,
    /// Profile ID for local agent runs. This configures the terminal session
    /// with the specified execution profile. Only used for local runs, not cloud runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// Skill spec to use as the base prompt for the agent.
    /// Format: "skill_name", "repo:skill_name", or "org/repo:skill_name".
    /// The skill is resolved at runtime in the agent environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_spec: Option<String>,
    /// Execution harness for the agent run.
    /// If None, we use Warp's default ("oz").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessConfig>,
    /// Extra repositories the worker should clone in addition to the environment's repos.
    /// This is server-populated for per-task sources such as a GitHub webhook's origin repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_source_repos: Option<Vec<SourceRepo>>,
}

/// Configuration for a third-party execution harness.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HarnessConfig {
    /// The harness type, e.g. [`Harness::Claude`].
    #[serde(
        rename = "type",
        serialize_with = "serialize_harness",
        deserialize_with = "deserialize_harness"
    )]
    pub harness_type: Harness,
    /// The model to use with this harness. None means use the harness default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Optional reasoning level for harnesses that support it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_level: Option<String>,
}

impl HarnessConfig {
    /// Builds a harness config from just the harness type.
    pub fn from_harness_type(harness_type: Harness) -> Self {
        Self {
            harness_type,
            model_id: None,
            reasoning_level: None,
        }
    }
}

fn serialize_harness<S: Serializer>(harness: &Harness, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(harness.config_name())
}

fn deserialize_harness<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Harness, D::Error> {
    let name = String::deserialize(deserializer)?;
    Ok(Harness::from_config_name(&name).unwrap_or_else(|| {
        log::warn!("Unknown harness config name: {name:?}; treating as Unknown");
        Harness::Unknown
    }))
}

/// Source-control provider hosting an environment's repositories.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CodeForge {
    #[default]
    #[serde(rename = "GITHUB")]
    GitHub,
    #[serde(rename = "GITLAB")]
    GitLab,
    /// Azure DevOps (Azure Repos). `owner` carries the
    /// `organization/project` pair, mirroring GitLab's nested namespaces.
    #[serde(rename = "AZURE_DEVOPS")]
    AzureDevOps,
    /// Explicit "no code forge" container value: a repo-less environment
    /// that clones nothing and relies entirely on `setup_commands`.
    #[serde(rename = "NONE")]
    None,
    // Catches a forge value this client build doesn't recognize yet (e.g. the
    // server adds one before this client updates), so the rest of the
    // environment still deserializes instead of the whole object failing.
    #[serde(other)]
    Unknown,
}

/// Identifies a repository and the source-control provider that hosts it.
///
/// For GitLab, `owner` contains the full, potentially nested namespace.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceRepo {
    /// The repository's explicit source-control provider.
    ///
    /// When absent, a single-forge environment fills this from the container
    /// primary. A mixed environment leaves it unset so clone fails closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_forge: Option<CodeForge>,
    pub owner: String,
    pub repo: String,
    /// Ref to check out after cloning this repository (commit SHA, branch, or
    /// tag). Absent leaves the clone on the default branch. Benchmark trials
    /// use it to start from a pinned base commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_ref: Option<String>,
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[test]
    fn additional_source_repos_round_trip_and_is_optional() {
        let snapshot = AgentConfigSnapshot {
            additional_source_repos: Some(vec![SourceRepo {
                code_forge: Some(CodeForge::GitHub),
                owner: "warpdotdev".to_string(),
                repo: "warp".to_string(),
                checkout_ref: None,
            }]),
            ..Default::default()
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            json["additional_source_repos"][0],
            serde_json::json!({
                "code_forge": "GITHUB",
                "owner": "warpdotdev",
                "repo": "warp"
            })
        );
        let decoded: AgentConfigSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, snapshot);
        let legacy: AgentConfigSnapshot = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(legacy.additional_source_repos.is_none());
    }
}
