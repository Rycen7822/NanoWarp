mod agent;
pub mod buffer_model;
mod classic;
mod common;
pub mod decorations;
pub(crate) mod handoff_compose;
pub mod message_bar;
mod suggestions_mode_menu;
pub mod suggestions_mode_model;
mod terminal;
mod universal;

use std::any::Any;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use async_channel::Sender;
use base64::Engine as _;
#[cfg(feature = "local_fs")]
use diesel::SqliteConnection;
use futures::FutureExt as _;
use futures::stream::AbortHandle;
use itertools::Itertools;
use lazy_static::lazy_static;
use ordered_float::Float;
use parking_lot::FairMutex;
#[cfg(feature = "local_fs")]
use parking_lot::Mutex;
use regex::Regex;
use serde::Serialize;
use session_sharing_protocol::common::{AgentAttachment, ParticipantId, ServerConversationToken};
use settings::{Setting as _, ToggleableSetting};
use string_offset::{ByteOffset, CharOffset};
use vim::vim::VimMode;
use warp_cli::agent::Harness;
use warp_completer::completer::{
    self, CompleterOptions, CompletionContext, CompletionsFallbackStrategy, Description,
    ExplicitTabCompletion, MatchStrategy, MatchType, PathSeparators, PreparedSuggestion,
    SuggestionResults,
};
use warp_completer::meta::{HasSpan, Span, Spanned};
use warp_completer::parsers::LiteCommand;
use warp_completer::parsers::simple::command_at_cursor_position;
use warp_completer::signatures::CommandRegistry;
use warp_completer::util::parse_current_commands_and_tokens;
use warp_core::r#async::debounce;
use warp_core::context_flag::ContextFlag;
use warp_core::ui::theme::AnsiColorIdentifier;
use warp_core::ui::theme::color::internal_colors;
use warp_editor::editor::NavigationKey;
use warp_errors::{report_error, report_if_error};
use warp_util::path::ShellFamily;
pub use warpui::WindowId;
use warpui::accessibility::{AccessibilityContent, ActionAccessibilityContent, WarpA11yRole};
#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
use warpui::r#async::FutureExt as _;
use warpui::r#async::SpawnedFutureHandle;
use warpui::clipboard::{ClipboardContent, ImageData};
use warpui::clipboard_utils::CLIPBOARD_IMAGE_MIME_TYPES;
use warpui::color::ColorU;
use warpui::elements::{
    Align, AnchorPair, Clipped, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    DispatchEventResult, DropTargetData, Element, EventHandler, Flex, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, OffsetType, ParentElement, Radius, ResizableStateHandle,
    SavePosition, SelectionHandle, Text, Wrap, YAxisAnchor, resizable_state_handle,
};
pub use warpui::elements::{ParentElement as _, Stack};
pub use warpui::geometry::vector::{Vector2F, vec2f};
use warpui::keymap::{BindingDescription, EditableBinding, FixedBinding, Keystroke};
use warpui::platform::OperatingSystem;
use warpui::presenter::ChildView;
use warpui::text_layout::TextStyle;
use warpui::ui_components::chip::Chip;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::units::IntoPixels;
use warpui::{
    AppContext, Entity, EntityId, FocusContext, ModelAsRef, ModelHandle, SingletonEntity,
    TypedActionView, View, ViewContext, ViewHandle, WeakViewHandle, end_trace, start_trace,
};

use self::decorations::InputBackgroundJobOptions;
pub use self::handoff_compose::{HandoffComposeState, HandoffComposeStateEvent};
use super::alias::is_expandable_alias;
use super::block_list_viewport::InputMode;
use super::event::{BlockCompletedEvent, BlockType, UserBlockCompleted};
use super::ligature_settings::LigatureSettings;
use super::model::block::{
    AgentInteractionMetadata, BlockId, BlockMetadata, BlocklistEnvVarMetadata,
};
use super::model::completions::ShellCompletion;
use super::model::session::{Session, SessionId, Sessions};
use super::prompt_render_helper::{
    PromptRenderHelper, SameLinePromptElements, should_render_prompt_on_same_line,
    should_render_prompt_using_editor_decorator_elements,
};
use super::safe_mode_settings::{
    SafeModeSettings, SafeModeSettingsChangedEvent, get_secret_obfuscation_mode,
};
use super::session_settings::{SessionSettings, SessionSettingsChangedEvent};
use super::settings::{SpacingMode, TerminalSettings, TerminalSettingsChangedEvent};
use super::shared_session::SharedSessionStatus;
use super::shared_session::presence_manager::PresenceManager;
use super::shared_session::viewer::history_model::SharedSessionHistoryModel;
use super::shell::ShellType;
use super::universal_developer_input::{
    UniversalDeveloperInputButtonBar, UniversalDeveloperInputButtonBarEvent,
};
use super::view::ambient_agent::{
    AmbientAgentViewModel, AmbientAgentViewModelEvent, is_cloud_agent_pre_first_exchange,
};
use super::view::queued_prompts_panel::{QueuedPromptsPanelEvent, QueuedPromptsPanelView};
use super::view::{
    ExecuteCommandEvent, PADDING_LEFT as TERMINAL_VIEW_PADDING_LEFT, SyncInputType, TerminalAction,
};
use super::warpify::SubshellSource;
use super::{History, HistoryEntry, SizeInfo, TerminalModel, prompt, should_right_click_paste};
#[allow(unused_imports)]
use crate::ASSETS;
use crate::ai::AIRequestUsageModel;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::{AIAgentContext, CancellationReason, EntrypointType, ImageContext};
use crate::ai::agent_conversations_model::AgentConversationsModel;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::ambient_agents::telemetry::HandoffEntryPoint;
use crate::ai::attachment_utils::MAX_ATTACHMENT_SIZE_BYTES;
use crate::ai::blocklist::agent_view::shortcuts::AgentShortcutViewModel;
use crate::ai::blocklist::agent_view::{
    AgentInputFooter, AgentInputFooterEvent, AgentViewController, AgentViewEntryOrigin,
    EphemeralMessageModel, is_in_cloud_context,
};
use crate::ai::blocklist::block::cli_controller::{CLISubagentController, CLISubagentEvent};
use crate::ai::blocklist::block::status_bar::BlocklistAIStatusBar;
#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
use crate::ai::blocklist::handoff::{
    HandoffLaunchAttachments, PendingCloudLaunch, suggest_handoff_environment,
};
use crate::ai::blocklist::prompt::prompt_alert::{PromptAlertEvent, PromptAlertView};
use crate::ai::blocklist::telemetry_banner::should_collect_ai_ugc_telemetry;
use crate::ai::blocklist::{
    AttachmentType, BLOCK_CONTEXT_ATTACHMENT_REGEX, BlocklistAIActionModel,
    BlocklistAIContextEvent, BlocklistAIContextModel, BlocklistAIController,
    BlocklistAIControllerEvent, BlocklistAIHistoryEvent, BlocklistAIHistoryModel,
    BlocklistAIInputModel, DIFF_HUNK_ATTACHMENT_REGEX, DRIVE_OBJECT_ATTACHMENT_REGEX, InputConfig,
    InputType, InputTypeAutoDetectionSource, PendingAttachment, PendingFile, QueuedQuery,
    QueuedQueryEvent, QueuedQueryId, QueuedQueryModel, QueuedQueryOrigin, SlashCommandRequest,
    ai_brand_color, ai_indicator_height, render_ai_agent_mode_icon, render_ai_follow_up_icon,
};
use crate::ai::cloud_agent_settings::{AuthSecretPreference, CloudAgentSettings};
use crate::ai::cloud_environments::CloudAmbientAgentEnvironment;
use crate::ai::connected_self_hosted_workers::{
    ConnectedSelfHostedWorkersEvent, ConnectedSelfHostedWorkersModel,
};
#[cfg(not(target_family = "wasm"))]
use crate::ai::conversation_export::export_conversation_markdown;
use crate::ai::document::ai_document_model::{AIDocumentId, AIDocumentVersion};
use crate::ai::harness_availability::{
    CloudAgentStartBlocker, HarnessAvailabilityModel, cloud_agent_start_blocker,
};
use crate::ai::llms::{LLMPreferences, LLMPreferencesEvent};
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::appearance::{Appearance, AppearanceEvent};
use crate::channel::ChannelState;
use crate::cloud_object::model::actions::ObjectActionType;
use crate::cloud_object::model::generic_string_model::StringModel;
use crate::cloud_object::model::persistence::CloudModel;
use crate::cloud_object::{CloudObject, CloudObjectLookup as _, Space};
#[cfg(feature = "local_fs")]
use crate::code::editor_management::CodeSource;
use crate::completer::SessionContext;
use crate::completer::is_command_valid;
use crate::context_chips::display::{PromptDisplay, PromptDisplayEvent};
use crate::context_chips::display_chip::{DisplayChipConfig, PromptChipShellCommand};
use crate::context_chips::prompt_type::PromptType;
use crate::context_chips::spacing;
use crate::editor::{
    AttachedImage as AttachedImageRawData, AutosuggestionLocation, AutosuggestionType,
    BaselinePositionComputationMethod, CommandXRayAnchor, CrdtOperation, CursorColors,
    DisplayPoint, EditOrigin, EditorAction, EditorDecoratorElements, EditorOptions, EditorSnapshot,
    EditorView, Event as EditorEvent, ImageContextOptions, InteractionState,
    MAX_IMAGES_PER_CONVERSATION, PathTransformerFn, PlainTextEditorViewAction,
    Point as BufferPoint, PropagateAndNoOpEscapeKey, PropagateAndNoOpNavigationKeys,
    PropagateHorizontalNavigationKeys, ReplicaId, TextColors, TextRun, default_cursor_colors,
    position_id_for_cached_point, position_id_for_cursor, position_id_for_first_cursor,
};
use crate::env_vars::EnvVarCollectionExt;
use crate::features::FeatureFlag;
use crate::input_suggestions::{
    Event as InputSuggestionsEvent, InputSuggestions, TabCompletionsPreselectOption,
};
use crate::pane_group::PaneGroupAction;
use crate::pane_group::focus_state::PaneFocusHandle;
#[cfg(feature = "local_fs")]
use crate::persistence::{database_file_path_for_current_scope, establish_ro_connection};
use crate::prefix::longest_common_prefix;
use crate::prompt::editor_modal::OpenSource as PromptEditorOpenSource;
use crate::resource_center::{
    Tip, TipHint, TipsCompleted, mark_feature_used_and_write_to_user_defaults,
};
use crate::search::QueryFilter;
use crate::server::cloud_objects::update_manager::UpdateManager;
use crate::server::ids::SyncId;
use crate::server::server_api::ServerApi;
#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
use crate::server::server_api::ai::AttachmentInput;
use crate::server::server_api::ai::{AIClient, AttachmentFileInfo};
use crate::server::server_api::presigned_upload::upload_to_target;
use crate::server::telemetry::{
    AgentModeAutoDetectionFalsePositivePayload, AgentModeAutoDetectionSettingOrigin,
    AnonymousUserSignupEntrypoint, CommandXRayTrigger, EnvVarTelemetryMetadata, PaletteSource,
    QueuedPromptSendNowTrigger, TelemetryEvent, WorkflowTelemetryMetadata,
};
use crate::session_management::SessionNavigationPromptElements;
use crate::settings::{
    AISettings, AISettingsChangedEvent, AliasExpansionSettings, AppEditorSettings,
    AppEditorSettingsChangedEvent, InputModeSettings, InputSettings, InputSettingsChangedEvent,
    MAX_TIMES_TO_SHOW_AUTOSUGGESTION_HINT, PrivacySettings,
};
use crate::settings_view::{SettingsSection, flags};
use crate::suggestions::ignored_suggestions_model::{
    IgnoredSuggestionsModel, IgnoredSuggestionsModelEvent, SuggestionType,
};
#[cfg(feature = "local_fs")]
use crate::terminal::history::find_next_commands_from_similar_history;
use crate::terminal::history::find_potential_autosuggestions_from_history;
use crate::terminal::input::buffer_model::InputBufferModel;
use crate::terminal::input::suggestions_mode_model::InputSuggestionsModeModel;
use crate::terminal::model::session::shell_quote_arg;
use crate::terminal::prompt_render_helper::should_render_ps1_prompt;
use crate::terminal::view::ambient_agent::{
    AuthSecretFtuxView, AuthSecretFtuxViewEvent, AuthSecretSelector, AuthSecretSelectorEvent,
    CLOUD_AGENT_TEAM_REQUIRED_MESSAGE, HarnessSelector, HarnessSelectorEvent, HostSelector,
    HostSelectorEvent, NakedHeaderButtonTheme,
};
use crate::terminal::view::init::CAN_ATTACH_FILE_KEY;
use crate::terminal::view::{
    AIQueryRouting, file_attach_allowed_for_shared_session, resolve_ai_query_routing,
    resolve_ambient_agent_task_id,
};
use crate::ui_components::blended_colors;
use crate::ui_components::icons::Icon;
use crate::util::bindings::{self, CustomAction};
#[cfg(feature = "local_fs")]
use crate::util::file::external_editor;
use crate::util::image::MAX_IMAGE_COUNT_FOR_QUERY;
use crate::util::truncation::truncate_from_end;
use crate::view_components::{DismissibleToast, ToastFlavor};
use crate::workflows::command_parser::{
    WorkflowArgumentIndex, WorkflowDisplayData, compute_workflow_display_data,
    compute_workflow_display_data_with_overrides,
};
use crate::workflows::info_box::{
    WORKFLOW_PARAMETER_HIGHLIGHT_COLOR, WorkflowsInfoBoxViewEvent, WorkflowsMoreInfoView,
};
use crate::workflows::workflow_enum::EnumVariants;
use crate::workflows::{WorkflowSelectionSource, WorkflowSource, WorkflowType};
use crate::workspace::sync_inputs::SyncedInputState;
use crate::workspace::{CommandSearchOptions, InitContent, ToastStack, WorkspaceAction};
use crate::workspaces::user_workspaces::{
    ResolvedTeamScope, TeamContext, UserWorkspaces, UserWorkspacesEvent,
};
#[allow(unused_imports)]
use crate::{AgentModeEntrypoint, ServerApiProvider, cmd_or_ctrl_shift, send_telemetry_from_ctx};

/// Drop target data for dropping content on the [`Input`].
#[derive(Debug, Clone)]
pub struct InputDropTargetData {
    pub input_view: WeakViewHandle<Input>,
}

impl InputDropTargetData {
    fn new(input_view: WeakViewHandle<Input>) -> Self {
        Self { input_view }
    }

    pub fn weak_view_handle(&self) -> WeakViewHandle<Input> {
        self.input_view.clone()
    }
}

impl DropTargetData for InputDropTargetData {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub const DEBOUNCE_INPUT_DECORATION_PERIOD: Duration = Duration::from_millis(10);

const CLOUD_MODE_V2_HINT_TEXT: &str = "Kick off a cloud agent";
const SHORT_CIRCUIT_HIGHLIGHTING_ACTIONS: [Option<PlainTextEditorViewAction>; 7] = [
    Some(PlainTextEditorViewAction::Space),
    Some(PlainTextEditorViewAction::NonExpandingSpace),
    Some(PlainTextEditorViewAction::Paste),
    Some(PlainTextEditorViewAction::Tab),
    Some(PlainTextEditorViewAction::AcceptCompletionSuggestion),
    Some(PlainTextEditorViewAction::CursorChanged),
    Some(PlainTextEditorViewAction::NewLine),
];

/// Border width for the line at the top of the input box in pixels
pub fn get_input_box_top_border_width() -> f32 {
    if FeatureFlag::MinimalistUI.is_enabled() {
        0.0
    } else {
        1.0
    }
}

/// The host the cloud-mode selector defaults to: the `WARP_CLOUD_MODE_DEFAULT_HOST` override when
/// set, otherwise the window's team default.
///
/// Takes the selector's handle, not `Input`'s: this runs inside `Input::new` before `Input` is in
/// `view_to_window`, so an `Input` handle would resolve no window here while the already-built
/// selector's does.
fn effective_default_host(
    host_selector: &WeakViewHandle<HostSelector>,
    app: &AppContext,
) -> Option<String> {
    if let Some(slug) = std::env::var("WARP_CLOUD_MODE_DEFAULT_HOST")
        .ok()
        .filter(|slug| !slug.is_empty())
    {
        return Some(slug);
    }
    let workspaces = UserWorkspaces::as_ref(app);
    workspaces
        .default_host_slug(&workspaces.team_context(host_selector, app))
        .map(String::from)
}

pub const COMPLETIONS_MENU_WIDTH: f32 = 330.;
pub const OPEN_COMPLETIONS_KEYBINDING_NAME: &str = "input:open_completion_suggestions";
pub(crate) const EXTERNAL_ALT_C_BINDING_CONTEXT: &str = "ExternalAltCDirectorySearch";
pub const INPUT_A11Y_LABEL: &str = "Command Input.";
pub const INPUT_A11Y_HELPER: &str = "Input your shell command, press enter to execute. Press cmd-up to navigate to output of previously executed commands. Press cmd-l to re-focus command input.";

const AGENT_MODE_AI_DISABLED_AUTODETECTION_DISABLED_HINT_TEXT: &str = "Run commands";

// Rotating hint text options for new Agent Mode conversations
const AGENT_MODE_HINT_OPTIONS: &[&str] = &[
    "Warp anything e.g. Deploy my React app to Vercel and set up environment variables",
    "Warp anything e.g. Help me debug why my Python tests are failing in CI",
    "Warp anything e.g. Set up a new microservice with Docker and create the deployment pipeline",
    "Warp anything e.g. Find and fix the memory leak in my Node.js application",
    "Warp anything e.g. Create a backup script for my PostgreSQL database and schedule it",
    "Warp anything e.g. Help me migrate my data from MySQL to PostgreSQL",
    "Warp anything e.g. Set up monitoring and alerts for my AWS infrastructure",
    "Warp anything e.g. Build a REST API for my mobile app using FastAPI",
    "Warp anything e.g. Help me optimize my SQL queries that are running slowly",
    "Warp anything e.g. Create a GitHub Actions workflow to automatically deploy on merge",
    "Warp anything e.g. Set up Redis caching for my web application",
    "Warp anything e.g. Help me troubleshoot why my Kubernetes pods keep crashing",
    "Warp anything e.g. Build a data pipeline to process CSV files and load them into BigQuery",
    "Warp anything e.g. Set up SSL certificates and configure HTTPS for my domain",
    "Warp anything e.g. Help me refactor this legacy code to use modern design patterns",
    "Warp anything e.g. Create unit tests for my authentication service",
    "Warp anything e.g. Set up log aggregation with ELK stack for my distributed system",
    "Warp anything e.g. Help me implement OAuth2 authentication in my Express.js app",
    "Warp anything e.g. Optimize my Docker images to reduce build times and size",
    "Warp anything e.g. Set up A/B testing infrastructure for my web application",
];

fn get_agent_mode_new_conversation_hint_text() -> &'static str {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static HINT_INDEX: AtomicUsize = AtomicUsize::new(0);

    let index = HINT_INDEX.fetch_add(1, Ordering::Relaxed) % AGENT_MODE_HINT_OPTIONS.len();
    AGENT_MODE_HINT_OPTIONS[index]
}

fn get_stable_agent_mode_hint_text(cached_hint: &mut Option<&'static str>) -> &'static str {
    if let Some(hint) = cached_hint {
        hint
    } else {
        let new_hint = get_agent_mode_new_conversation_hint_text();
        *cached_hint = Some(new_hint);
        new_hint
    }
}

const AGENT_MODE_AI_ENABLED_STEER_HINT_TEXT_UDI: &str = "Steer the running agent";
const AGENT_MODE_AI_ENABLED_STEER_HINT_TEXT_CLASSIC: &str =
    "Steer the running agent, or backspace to exit";
const AGENT_MODE_AI_ENABLED_QUEUE_HINT_TEXT_UDI: &str = "Queue a follow up for the running agent";
const AGENT_MODE_AI_ENABLED_QUEUE_HINT_TEXT_CLASSIC: &str =
    "Queue a follow up for the running agent, or backspace to exit";
const AGENT_MODE_AI_ENABLED_FOLLOW_UP_HINT_TEXT_UDI: &str = "Ask a follow up";
const AGENT_MODE_AI_ENABLED_FOLLOW_UP_HINT_TEXT_CLASSIC: &str =
    "Ask a follow up, or backspace to exit";

/// Action name for setting input mode to agent mode
pub const SET_INPUT_MODE_AGENT_ACTION_NAME: &str = "input:set_mode_agent";

/// Action name for setting input mode to terminal mode
pub const SET_INPUT_MODE_TERMINAL_ACTION_NAME: &str = "input:set_mode_terminal";

/// Action name for setting input mode to unlocked agent mode (with natural language detection)
pub const SET_INPUT_MODE_UNLOCKED_AGENT_ACTION_NAME: &str = "input:set_mode_unlocked_agent";

/// Action name for setting input mode to unlocked terminal mode (with natural language detection)
pub const SET_INPUT_MODE_UNLOCKED_TERMINAL_ACTION_NAME: &str = "input:set_mode_unlocked_terminal";

const START_NEW_CONVERSATION_KEYBINDING_NAME: &str = "input:start_new_agent_conversation";

/// The position ID used to identify the start of the replacement span for completions.
const COMPLETIONS_START_OF_REPLACEMENT_SPAN_POSITION_ID: &str =
    "start_of_completions_replacement_span";

const HISTORY_DETAILS_VIEW_WIDTH_REQUIREMENT: f32 = 1100.;

const MIN_BUFFER_LEN_TO_SHOW_COMPLETIONS_WHILE_TYPING: usize = 2;

const QUEUED_PROMPT_INLINE_EDITOR_OPEN_CONTEXT: &str = "QueuedPromptInlineEditorOpen";

/// If the editor buffer matches this prefix, AI input is enabled.
const AI_INPUT_PREFIX: &str = "* ";

/// If the editor buffer matches this prefix, terminal input is enabled and locked.
const TERMINAL_INPUT_PREFIX: &str = "!";
/// If the editor buffer matches this prefix, local agent input enters cloud handoff compose mode.
const CLOUD_HANDOFF_INPUT_PREFIX: &str = "&";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputPrefixMode {
    None,
    Shell,
    CloudHandoff,
}

const VIM_STATUS_BAR_BOTTOM_PADDING: f32 = 20.;

const DYNAMIC_ENUM_GENERATE_MESSAGE: &str = "Run the following command to generate variants:";
const DYNAMIC_ENUM_RUN_MESSAGE: &str = "Run command";
const DYNAMIC_ENUM_PENDING_MESSAGE: &str = "Command pending...";
const DYNAMIC_ENUM_FAILURE_MESSAGE: &str = "Command failed";
const DYNAMIC_ENUM_NO_RESULTS_MESSAGE: &str = "Command returned no results";
const DYNAMIC_ENUM_MENU_PADDING: f32 = 10.;
const DYNAMIC_ENUM_MENU_HEIGHT_OFFSET: f32 = 25.;
const DYNAMIC_ENUM_HORIZONTAL_TEXT_PADDING: f32 = 5.;

lazy_static! {
    static ref RUN_DYNAMIC_ENUM_COMMAND_KEYSTROKE: Keystroke = if OperatingSystem::get().is_mac() {
        Keystroke {
            cmd: true,
            key: "enter".to_owned(),
            ..Default::default()
        }
    } else {
        Keystroke {
            ctrl: true,
            shift: true,
            key: "enter".to_owned(),
            ..Default::default()
        }
    };
}

#[derive(PartialEq, Eq, Copy, Clone, Serialize)]
pub enum TelemetryInputSuggestionsMode {
    HistoryFuzzySearch,
    CompletionSuggestions,
    HistoryUp,
    NaturalLanguageCommandSearch,
    StaticWorkflowEnumSuggestions,
    DynamicWorkflowEnumSuggestions,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HistorySearchMode {
    /// Prefix match commands.
    Prefix,
    /// Fuzzy match commands.
    Fuzzy,
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum TabCompletionsMenuPosition {
    /// The menu should be positioned at the last cursor.
    AtLastCursor,
    /// The menu should be positioned at the first cursor.
    AtFirstCursor,
    /// The menu should be positioned at the given position.
    AtStartOfReplacementSpan,
}

impl TabCompletionsMenuPosition {
    fn to_position_id(self, editor_view_id: EntityId) -> String {
        match self {
            Self::AtLastCursor => position_id_for_cursor(editor_view_id),
            Self::AtFirstCursor => position_id_for_first_cursor(editor_view_id),
            Self::AtStartOfReplacementSpan => position_id_for_cached_point(
                editor_view_id,
                COMPLETIONS_START_OF_REPLACEMENT_SPAN_POSITION_ID,
            ),
        }
    }
}

#[derive(Default, PartialEq, Eq, Debug, Clone)]
pub enum InputSuggestionsMode {
    /// Mode used when arrow-up is pressed.
    HistoryUp {
        /// Text in the buffer when arrow-up is pressed (possibly empty).
        original_buffer: String,
        /// Cursor point when arrow-up is pressed.
        /// This is None when there are > 1 active selections when HistoryUp is invoked.
        /// TODO: eventually, we should support saving/resetting _many_ cursors rather than a single one.
        original_cursor_point: Option<BufferPoint>,
        search_mode: HistorySearchMode,
        /// The AI input mode when arrow-up is pressed.
        original_input_type: InputType,
        /// The AI input's lock status when the arrow-up is pressed.
        original_input_was_locked: bool,
    },
    CompletionSuggestions {
        /// Stores the byte index of the beginning of the text we are replacing
        replacement_start: usize,

        /// Stores the original buffer text before the user pressed TAB.
        /// Used to close the suggestions menu if the buffer_text_original is no longer in the input buffer.
        buffer_text_original: String,

        /// Stores the suggestions for the original buffer_text_original.
        /// Used to filter down results during prefix search.
        completion_results: SuggestionResults,

        /// Stores the original trigger of the completions, so that we can track whether the menu
        /// was opened automatically (AsYouType) or manually (with Tab)
        trigger: CompletionsTrigger,

        /// Where the menu should be positioned.
        menu_position: TabCompletionsMenuPosition,
    },

    StaticWorkflowEnumSuggestions {
        /// The suggested values for the workflow argument.
        suggestions: Vec<String>,

        /// Where the menu should be positioned.
        menu_position: TabCompletionsMenuPosition,

        /// The selected ranges for every instance of the argument.
        selected_ranges: Vec<Range<ByteOffset>>,

        /// Store the cursor point of the end of the first selected argument.
        cursor_point: BufferPoint,
    },

    DynamicWorkflowEnumSuggestions {
        /// The suggested values for the workflow argument.
        suggestions: Vec<String>,

        /// Where the menu should be positioned.
        menu_position: TabCompletionsMenuPosition,

        /// The selected ranges for every instance of the argument.
        selected_ranges: Vec<Range<ByteOffset>>,

        /// Store the cursor point of the end of the first selected argument.
        cursor_point: BufferPoint,

        /// Store the current state of the dynamic enum suggestions menu.
        dynamic_enum_status: DynamicEnumSuggestionStatus,

        /// The command associated with the dynamic enum.
        command: String,
    },

    /// Mode indicating that no suggestion UI is being shown.
    #[default]
    Closed,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DynamicEnumSuggestionStatus {
    /// When the command has not yet been approved to run on the users laptop
    Unapproved,
    /// The command is running asynchronously, but has not yet finished so we do not have suggestions to display
    Pending,
    /// The command succeeded; display suggested variants
    Success,
    /// The command failed
    Failure,
}

impl InputSuggestionsMode {
    pub fn is_visible(&self) -> bool {
        *self != InputSuggestionsMode::Closed
    }

    fn to_telemetry_mode(&self) -> TelemetryInputSuggestionsMode {
        match *self {
            InputSuggestionsMode::HistoryUp {
                search_mode: HistorySearchMode::Prefix,
                ..
            } => TelemetryInputSuggestionsMode::HistoryUp,
            InputSuggestionsMode::HistoryUp {
                search_mode: HistorySearchMode::Fuzzy,
                ..
            } => TelemetryInputSuggestionsMode::HistoryFuzzySearch,
            InputSuggestionsMode::CompletionSuggestions { .. } => {
                TelemetryInputSuggestionsMode::CompletionSuggestions
            }
            InputSuggestionsMode::StaticWorkflowEnumSuggestions { .. } => {
                TelemetryInputSuggestionsMode::StaticWorkflowEnumSuggestions
            }
            InputSuggestionsMode::DynamicWorkflowEnumSuggestions { .. } => {
                TelemetryInputSuggestionsMode::DynamicWorkflowEnumSuggestions
            }
            InputSuggestionsMode::Closed => unreachable!(),
        }
    }
}

struct SharedSessionInputState {
    /// History model for viewers in a shared session.
    // TODO: With this current approach, the shared session history crosses
    // subshell boundaries, we'll need to make it work with our current history model
    // to ensure we show the right shell history.
    history_model: ModelHandle<SharedSessionHistoryModel>,

    // Is [`Some`] iff a command execution was requested by a shared session executor.
    pending_command_execution_request: Option<ViewerCommandExecutionRequest>,
}

struct ViewerCommandExecutionRequest {
    /// Text in buffer when command execution was requested.
    original_buffer: String,
}

/// Where a command execution request originates from.
#[derive(Clone)]
pub enum CommandExecutionSource {
    /// A non-shared command execution request from Warp AI++.
    /// Shared commands use the SharedSession variant instead.
    AI {
        /// Metadata associated with the execution.
        metadata: AgentInteractionMetadata,
    },

    /// A command execution request in a shared session (by a viewer or sharer).
    ///
    /// For a sharer, this will be processed similar to [`CommandExecutionSource::User`]
    /// except the resulting block will be annotated with the participant ID.
    ///
    /// For a viewer, this will be handled by sending the request to the sharer.
    SharedSession {
        /// The participant ID of the
        participant_id: ParticipantId,
        /// The block ID associated to the active block when
        /// the request was fired.
        block_id: BlockId,
        /// Optional AI metadata if this command was requested by the AI agent
        /// in a shared session. This is used to associate the resulting command block
        /// with the original agent command.
        ai_metadata: Option<AgentInteractionMetadata>,
        /// True when the command was dispatched by a queued command row rather than the current
        /// editor buffer, so input draft state should be preserved.
        preserve_input: bool,
    },

    /// A normal command execution request.
    User,
    /// A command dispatched by the queued-prompts panel. It should execute like a user command but
    /// must not treat the current editor contents as the submitted command.
    QueuedCommand,

    EnvVarCollection {
        metadata: BlocklistEnvVarMetadata,
    },
}

impl CommandExecutionSource {
    /// Whether this command execution originates from an AI command.
    pub fn is_ai_command(&self) -> bool {
        // TODO: at some point we will want to couple both of these cases
        // into one source variant, as they are both AI sources.
        matches!(
            self,
            CommandExecutionSource::AI { .. }
                | CommandExecutionSource::SharedSession {
                    ai_metadata: Some(_),
                    ..
                }
        )
    }

    pub fn should_preserve_input(&self) -> bool {
        matches!(
            self,
            CommandExecutionSource::QueuedCommand
                | CommandExecutionSource::SharedSession {
                    preserve_input: true,
                    ..
                }
        )
    }
}

fn render_prompt_chip_shell_command(
    command: &PromptChipShellCommand,
    shell_type: ShellType,
) -> String {
    match command {
        PromptChipShellCommand::GitCheckout { branch_name } => {
            format!("git checkout {}", shell_quote_arg(branch_name, shell_type))
        }
        PromptChipShellCommand::GitCreateAndCheckoutBranch { branch_name } => {
            format!(
                "git checkout -b {} --",
                shell_quote_arg(branch_name, shell_type)
            )
        }
        PromptChipShellCommand::ChangeDirectory { dir_name } => {
            format!("cd {}", shell_quote_arg(dir_name, shell_type))
        }
        PromptChipShellCommand::NvmUse { version } => {
            format!("nvm use {}", shell_quote_arg(version, shell_type))
        }
        PromptChipShellCommand::NvmInstallLatestNode => "nvm install node".to_string(),
        PromptChipShellCommand::Echo { message } => {
            format!("echo {}", shell_quote_arg(message, shell_type))
        }
    }
}

#[derive(PartialEq, Eq, Copy, Clone)]
pub enum HistoryUpMode {
    // Show prefixed results.
    Prefixed,
    // Show all results with no query.
    RegularNoQuery,
    // Show all results with query.
    RegularWithQuery,
    // Used for ConfirmSuggestion event.
    NotApplicable,
}

impl HistoryUpMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            HistoryUpMode::Prefixed => "prefixed history up",
            HistoryUpMode::RegularNoQuery => "regular history up (no query)",
            HistoryUpMode::RegularWithQuery => "regular history up (with query)",
            HistoryUpMode::NotApplicable => "history up",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEmptyStateChangeReason {
    /// The buffer transitioned between empty and non-empty due to a regular edit.
    Edited,
    /// The buffer was cleared because a user-executed command completed and we reinitialized the
    /// buffer for the next command.
    UserCommandCompleted,
}

pub enum Event {
    AutosuggestionAccepted,
    ClearSelectedBlock,
    PageUp,
    PageDown,
    SelectRecentBlocks {
        /// Select the `count` most recent blocks.
        count: usize,
    },
    Copy,
    UnhandledModifierKeyOnEditor(Arc<String>),
    ClearSelectionsWhenShellMode,
    InputStateChanged(InputState),
    /// Emitted when the input text transitions between empty and non-empty states
    InputEmptyStateChanged {
        is_empty: bool,
        reason: InputEmptyStateChangeReason,
    },
    Escape,
    /// note: Terminal Inputs should only emit the variant
    /// SyncInputType::InputEditorContentsChanged.
    SyncInput(SyncInputType),
    ShowCommandSearch(CommandSearchOptions),
    CtrlD,
    CtrlC {
        // The number of chars cleared from the buffer, if the ctrl-c triggered a buffer clear.
        cleared_buffer_len: usize,
    },
    ExecuteCommand(Box<ExecuteCommandEvent>),
    ExecuteAIQuery,
    EmacsBindingUsed,
    /// The input editor was locally edited and
    /// peers should be notified, if applicable.
    EditorUpdated {
        /// The block ID associated to the buffer that
        /// these operations were made in.
        block_id: BlockId,

        /// The CRDT-compliant operations.
        operations: Rc<Vec<CrdtOperation>>,
    },
    /// A viewer in a shared session is requesting to send an agent prompt.
    SendAgentPrompt {
        server_conversation_token: Option<ServerConversationToken>,
        prompt: String,
        attachments: Vec<AgentAttachment>,
    },
    /// A disconnected Cloud Mode pane is requesting to submit a cloud follow-up.
    SubmitCloudFollowup {
        prompt: String,
    },
    /// A retained environment-setup-failure pane, or an already attached live viewer of one,
    /// is requesting a debug follow-up through the authenticated run follow-up service
    /// (REMOTE-2661).
    SubmitSetupFailureDebugFollowup {
        task_id: crate::ai::ambient_agents::AmbientAgentTaskId,
        prompt: String,
    },
    /// A viewer in a shared session is requesting to cancel the active agent conversation.
    CancelSharedSessionConversation {
        server_conversation_token: ServerConversationToken,
    },
    InputFocusedFromMiddleClick,
    EditorFocused,
    SignupAnonymousUser {
        entrypoint: AnonymousUserSignupEntrypoint,
    },
    OpenSettings(SettingsSection),
    #[cfg(feature = "local_fs")]
    OpenCodeInWarp {
        source: CodeSource,
        layout: external_editor::settings::EditorLayout,
    },
    OpenCodeReviewPane,
    OpenViewMCPPane,
    OpenAddMCPPane,
    OpenProjectRulesPane,
    OpenEnvironmentManagementPane,
    OpenFilesPalette {
        source: PaletteSource,
    },
    ToggleAIDocumentPane {
        document_id: AIDocumentId,
        document_version: AIDocumentVersion,
    },
    OpenAIDocumentPane {
        document_id: AIDocumentId,
        document_version: AIDocumentVersion,
    },
    AuthSecretDeleteConfirmationDialogToggled {
        is_open: bool,
    },
    ShowToast {
        message: String,
        flavor: ToastFlavor,
    },

    EnterAgentView {
        initial_prompt: Option<String>,
        conversation_id: Option<AIConversationId>,
        origin: AgentViewEntryOrigin,
    },
    EnterCloudAgentView {
        initial_prompt: Option<String>,
    },
    CreateDockerSandbox,
    /// Exit cloud mode (ambient agent) and start a new *local* agent conversation in the root terminal.
    ///
    /// If `initial_prompt` is `Some`, it should prefill the local agent prompt but not auto-send.
    ExitCloudModeAndStartLocalAgent {
        initial_prompt: Option<String>,
    },
    /// Trigger environment setup flow with optional repository arguments
    TriggerEnvironmentSetup {
        repos: Vec<String>,
    },
    OpenShareSessionModal,
    StartRemoteControl,
    OpenHandoffEnvironmentCreationModal,
    OpenCloudModeV2EnvironmentCreationModal,
}

pub enum InputState {
    Enabled,
    Disabled,
}

#[derive(Clone, Debug)]
pub enum InputAction {
    FocusInputBox,
    CtrlR,
    CtrlD,
    Up,
    PageUp,
    PageDown,
    ClearScreen,
    /// Open the completions menu if the cursor is in a valid position to generate completion
    /// suggestions.
    MaybeOpenCompletionSuggestions,
    HideWorkflowInfoCard,

    ToggleClassicCompletionsMode,

    StartNewAgentConversation {
        origin: AgentViewEntryOrigin,
    },

    /// This is for toggling whether autodetection is enabled/disabled at the app-level,
    /// not for whether its enabled/disabled for the current input
    ToggleInputAutoDetection,

    /// Triggers the lightbulb button click behavior to enable/toggle auto-detection
    EnableAutoDetection,

    /// Sets the hover state of the Universal Developer Input
    SetUDIHovered(bool),

    /// Persist the completions menu width when the user resizes it.
    UpdateCompletionsMenuWidth(f32),

    /// Persist the completions menu height when the user resizes it.
    UpdateCompletionsMenuHeight(f32),

    /// Toggles the '?' shortcuts UI in the agent view.
    ToggleAgentViewShortcuts,

    /// Opens the model selector menu.

    /// Clears attached blocks and text selection context.
    ClearAttachedContext,

    /// Fired when the "Get Figma MCP" contextual button is clicked.
    FigmaAddButtonClicked,

    /// Fired when the "Enable Figma MCP" contextual button is clicked.
    FigmaEnableButtonClicked,

    /// Activates `&` cloud handoff compose mode from the message bar hint.
    ActivateCloudHandoff,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum MenuPositioning {
    /// Position floating input menus above the input box -- corresponds
    /// to the regular blocklist.
    #[default]
    AboveInputBox,

    /// Position floating input menus below the input box -- corresponds
    /// to the inverted blocklist.
    BelowInputBox,
}

impl MenuPositioning {
    fn completion_suggestions_y_anchor(&self) -> AnchorPair<YAxisAnchor> {
        self.y_anchor()
    }

    fn history_y_anchor(&self) -> AnchorPair<YAxisAnchor> {
        self.y_anchor()
    }

    fn history_y_offset(&self) -> OffsetType {
        match *self {
            MenuPositioning::AboveInputBox => OffsetType::Pixel(0.),
            MenuPositioning::BelowInputBox => OffsetType::Pixel(-11.),
        }
    }

    fn command_xray_y_anchor(&self) -> AnchorPair<YAxisAnchor> {
        self.y_anchor()
    }

    fn workflows_info_y_anchor(&self) -> AnchorPair<YAxisAnchor> {
        self.y_anchor()
    }

    fn y_anchor(&self) -> AnchorPair<YAxisAnchor> {
        match *self {
            MenuPositioning::AboveInputBox => {
                AnchorPair::new(YAxisAnchor::Top, YAxisAnchor::Bottom)
            }
            MenuPositioning::BelowInputBox => {
                AnchorPair::new(YAxisAnchor::Bottom, YAxisAnchor::Top)
            }
        }
    }
}

impl MenuPositioningProvider for MenuPositioning {
    fn menu_position(&self, _app: &AppContext) -> MenuPositioning {
        *self
    }
}

struct WorkflowsState {
    selected_workflow_state: Option<SelectedWorkflowState>,
}

struct EnvVarCollectionState {
    selected_env_vars: Option<SyncId>,
}

/// State when a workflow is selected.
#[derive(Clone)]
struct SelectedWorkflowState {
    /// A handle to the WorkflowsMoreInfoView shown for the selected workflow.
    ///
    /// Note that this is unconditionally constructed, even when `should_show_more_info_view` is
    /// `false`, because the `WorkflowsMoreInfoView` itself contains business logic for the state
    /// of the input editor when editing workflow arguments with the shift-tab UX. This isn't
    /// ideal, and more of a symptom of retrofitting a `WorkflowsMoreInfoView`-less version of the
    /// shift-tab UX specifically for up-arrow history.
    more_info_view: ViewHandle<WorkflowsMoreInfoView>,

    /// Map of arguments to the corresponding index of highlights. This is necessary so that we can
    /// select all instances of an argument when a user changes the selected argument.
    argument_index_to_highlight_index: HashMap<WorkflowArgumentIndex, Vec<usize>>,

    /// Map of arguments with enum variants to those variants, which are used as suggested inputs to the argument.
    argument_index_to_enum_variants: HashMap<WorkflowArgumentIndex, EnumVariants>,

    workflow_source: WorkflowSource,
    workflow_type: WorkflowType,
    workflow_selection_source: WorkflowSelectionSource,

    /// `true` if the WorkflowsMoreInfoView should be shown for the selected workflow. This is true
    /// when the viewer may display workflow details.
    should_show_more_info_view: bool,
}

/// Helper struct for performing alias expansion.
struct ExpansionInfo {
    /// The expanded text to replace the alias with.
    alias_value: String,
    /// The buffer text to replace the alias in.
    buffer_text: String,
    /// The byte indices that should be replaced with the alias_value.
    byte_range: Range<usize>,
}

/// For inserting last word of last command in history - by default, this is the last command but consecutive
/// inserts fetch further in history. Represents reverse index of history command to reference.
/// (insert_command_from_history_index=0 for most recent, 1 for command before it, etc.) See self.update_last_word_insertion_state()
struct LastWordInsertion {
    insert_command_from_history_index: usize,
    is_latest_editor_event: bool,
}

/// Data pertaining to the session state and history is bundled together, making
/// it accessible to other objects coupled with the same terminal session, such as a notebook.
#[derive(Clone)]
pub struct CompleterData {
    pub sessions: ModelHandle<Sessions>,
    pub active_block_metadata: Option<BlockMetadata>,
    command_registry: Arc<CommandRegistry>,
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    last_user_block_completed: Option<UserBlockCompleted>,
}

impl CompleterData {
    pub fn new(
        sessions: ModelHandle<Sessions>,
        active_block_metadata: Option<BlockMetadata>,
        command_registry: Arc<CommandRegistry>,
        last_user_block_completed: Option<UserBlockCompleted>,
    ) -> Self {
        Self {
            sessions,
            active_block_metadata,
            command_registry,
            last_user_block_completed,
        }
    }

    pub fn active_block_session_id(&self) -> Option<SessionId> {
        self.active_block_metadata
            .as_ref()
            .and_then(BlockMetadata::session_id)
    }

    pub fn completion_session_context(&self, app: &AppContext) -> Option<SessionContext> {
        let active_block_session_id = self.active_block_session_id()?;
        let current_session = self.sessions.as_ref(app).get(active_block_session_id);
        let pwd = self
            .active_block_metadata
            .as_ref()
            .and_then(BlockMetadata::current_working_directory)
            .map(str::to_owned);

        current_session.zip(pwd).map(|(current_session, pwd)| {
            // TODO(abhishek): Ideally, BlockMetadata::current_working_directory should directly
            // return a TypedPathBuf. This shouldn't happen here in the view.
            let current_working_directory =
                current_session.convert_directory_to_typed_path_buf(pwd);

            SessionContext::new(
                current_session,
                self.command_registry.clone(),
                current_working_directory,
            )
        })
    }
}

/// Autosuggestion result returned by the generator.
pub struct AutoSuggestionResult {
    /// Text in the editor buffer.
    pub buffer_text: String,
    /// Generated autosuggestion result.
    pub autosuggestion_result: Option<String>,
}

/// Views that call into the autosuggestion generation logic must implement the Autosuggester
/// trait. This requires a callback on_autosuggestion_result and functions to set and abort
/// the latest future that's been spawned for autosuggestions.
pub trait Autosuggester {
    fn on_autosuggestion_result(
        &mut self,
        _result: AutoSuggestionResult,
        _ctx: &mut ViewContext<Self>,
    ) {
    }

    fn abort_latest_autosuggestion_future(&mut self);

    fn set_autosuggestion_future(&mut self, abort_handle: AbortHandle);
}

/// Implement this trait to provide whether input menus like autocomplete
/// should be positionined above or below the input.
pub trait MenuPositioningProvider {
    fn menu_position(&self, app: &AppContext) -> MenuPositioning;
}

/// Stores state referenced by the Input view and PromptRenderHelper.
/// Note that this is largely a workaround to avoid having to pass/upgrade
/// a weak view handle from `Input` to `PromptRenderHelper` for this state.
pub struct InputRenderStateModel {
    editor_modified_since_block_finished: bool,
    // For future: we should explore reading this directly off TerminalModel.
    size_info: SizeInfo,
}

impl InputRenderStateModel {
    pub fn new(editor_modified_since_block_finished: bool, size_info: SizeInfo) -> Self {
        Self {
            editor_modified_since_block_finished,
            size_info,
        }
    }

    pub fn editor_modified_since_block_finished(&self) -> bool {
        self.editor_modified_since_block_finished
    }

    pub fn size_info(&self) -> SizeInfo {
        self.size_info
    }

    pub fn set_editor_modified_since_block_finished(
        &mut self,
        editor_modified_since_block_finished: bool,
    ) {
        self.editor_modified_since_block_finished = editor_modified_since_block_finished;
    }

    pub fn set_size_info(&mut self, size_info: SizeInfo) {
        self.size_info = size_info;
    }
}

impl Entity for InputRenderStateModel {
    type Event = ();
}

lazy_static! {
    /// Define the regex patterns that we show completions-as-you-type in AI input on.
    /// We only show file completions - as such, we match on the following patterns:
    /// 1. "/": The last word starts with a slash
    /// 2. "./": The last word starts with "./"
    /// 3. "../": The last word starts with "../"
    /// 4. "{text}/": The last word contains a slash after some text
    /// We combine all the regex patterns for performance reasons (one string scan).
    /// NOTE: this assumes Unix-style paths. When we expand to Windows, we'll want to update this!
    static ref FILEPATH_PATTERN: Regex = Regex::new(
        r"^(?:/|\.\/|\.\./|[^/]+/)"
    ).expect("Expect regex to be valid");
}

/// Returns boolean indicating whether completions-as-you-type should pop up, while in AI input.
/// This is primarily based on the last word in the buffer text, and whether it makes sense to show
/// filepath completions.
fn should_show_completions_in_ai_input(buffer_text: &str) -> bool {
    if buffer_text.ends_with(char::is_whitespace) {
        return false;
    }

    let last_word = buffer_text.split_whitespace().last();

    if let Some(last_word) = last_word {
        FILEPATH_PATTERN.is_match(last_word)
    } else {
        false
    }
}

fn strip_control_characters(text: &str) -> Cow<'_, str> {
    if text.chars().any(|c| c.is_control()) {
        text.chars()
            .filter(|c| !c.is_control())
            .collect::<String>()
            .into()
    } else {
        text.into()
    }
}

/// Which completion sources a request draws on, once the two user toggles and native-completions
/// eligibility have been resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionSources {
    None,
    WarpOnly,
    NativeOnly,
    /// Bundled specs first, asking the shell only if they come back empty.
    WarpThenNative,
}

impl CompletionSources {
    fn resolve(warp_completions_enabled: bool, native_shell_completions_eligible: bool) -> Self {
        match (warp_completions_enabled, native_shell_completions_eligible) {
            (true, true) => Self::WarpThenNative,
            (true, false) => Self::WarpOnly,
            (false, true) => Self::NativeOnly,
            (false, false) => Self::None,
        }
    }

    /// Whether the shell's native completions are consulted for this request.
    fn uses_native(self) -> bool {
        matches!(self, Self::NativeOnly | Self::WarpThenNative)
    }
}

/// Resolves which [`CompletionSources`] a request draws on from the `NativeShellCompletions`
/// feature flag, the input type, the trigger, and the two user toggles -- the outer policy that
/// sits above [`CompletionSources::resolve`].
fn resolve_completion_sources(
    feature_flag_enabled: bool,
    is_ai_input: bool,
    buffer_text_is_multiline: bool,
    completions_trigger: CompletionsTrigger,
    warp_completions_enabled: bool,
    native_shell_completions_enabled: bool,
) -> CompletionSources {
    let (warp_completions_enabled, native_shell_completions_enabled) = if feature_flag_enabled {
        (warp_completions_enabled, native_shell_completions_enabled)
    } else {
        (true, false)
    };

    if is_ai_input {
        return CompletionSources::WarpOnly;
    }

    let native_shell_completions_eligible = completions_trigger != CompletionsTrigger::AsYouType
        && native_shell_completions_enabled
        && !buffer_text_is_multiline // For now, don't use native shell completions for multi-line commands.
        && !is_ai_input;

    CompletionSources::resolve(warp_completions_enabled, native_shell_completions_eligible)
}

/// Builds [`SuggestionResults`] from a shell's native-completions reply.
fn native_shell_suggestion_results(
    shell_results: Vec<ShellCompletion>,
    shell_replacement_span: Option<Span>,
    buffer_text: &str,
    cursor_position: usize,
) -> SuggestionResults {
    let suggestions = shell_results.into_iter().map(Into::into).collect_vec();
    let buffer_text_before_cursor = &buffer_text[0..cursor_position];
    let replacement_span = match shell_replacement_span {
        Some(span) => span.clamped_to(buffer_text_before_cursor),
        None => {
            // Within the section of the buffer from the start to the end of this token, find the
            // last whitespace char before the token end; the token starts just after it (or at the
            // start of the buffer if there's none).
            let token_start = buffer_text_before_cursor
                .rfind(char::is_whitespace)
                .map(|pos| pos + 1)
                .unwrap_or_default();
            (token_start, cursor_position).into()
        }
    };
    SuggestionResults {
        replacement_span,
        suggestions,
        match_strategy: MatchStrategy::Fuzzy,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DenyExecutionReason {
    /// Can't execute command because shell bootstrapping is still underway; shell isn't ready to
    /// execute user-supplied commands yet.
    NotBootstrapped,

    /// Can't execute command because there's an active command in control of the pty.
    ExistingActiveCommand,

    /// With the exception of shared sessions, we should only execute commands if they can be
    /// recorded in history.
    ///
    /// Gonna be honest, I (zach b) have the least amount of context on this one, don't really know
    /// why this is the case.
    ///
    /// This is not returned as a `CancellationReason::No` for shared sessions even if it may be
    /// true; we do not record shared sessions in the History model thus they are default not-
    /// appendable.
    HistoryNotAppendable,
}

impl DenyExecutionReason {
    pub fn is_existing_active_command(&self) -> bool {
        matches!(self, DenyExecutionReason::ExistingActiveCommand)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanExecuteCommand {
    Yes,
    No(DenyExecutionReason),
}

impl CanExecuteCommand {
    pub fn is_no(&self) -> bool {
        matches!(self, CanExecuteCommand::No(_))
    }
}

pub struct Input {
    model: Arc<FairMutex<TerminalModel>>,
    menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
    tips_completed: ModelHandle<TipsCompleted>,
    editor: ViewHandle<EditorView>,
    input_suggestions: ViewHandle<InputSuggestions>,
    suggestions_mode_model: ModelHandle<InputSuggestionsModeModel>,
    completions_menu_resizable_width: ResizableStateHandle,
    completions_menu_resizable_height: ResizableStateHandle,
    sessions: ModelHandle<Sessions>,
    focus_handle: Option<PaneFocusHandle>,
    active_block_metadata: Option<BlockMetadata>,
    /// The [`EntityId`] of the terminal view that this input view is inside.
    terminal_view_id: EntityId,
    view_id: EntityId,
    input_render_state_model_handle: ModelHandle<InputRenderStateModel>,
    workflows_state: WorkflowsState,
    env_var_collection_state: EnvVarCollectionState,
    command_x_ray_description: Option<Arc<Description>>,
    last_parsed_tokens: Option<decorations::ParsedTokensSnapshot>,
    debounce_input_background_tx: Sender<InputBackgroundJobOptions>,
    /// If true, will submit the command in the editor to the shell upon receiving the
    /// precmd message.
    has_pending_command: bool,
    last_word_insertion: LastWordInsertion,

    ai_controller: ModelHandle<BlocklistAIController>,
    ai_context_model: ModelHandle<BlocklistAIContextModel>,
    ai_input_model: ModelHandle<BlocklistAIInputModel>,
    ai_action_model: ModelHandle<BlocklistAIActionModel>,
    /// The input is responsible for managing the lifetime
    /// of this mouse state handle.
    #[allow(dead_code)]
    ai_follow_up_icon_mouse_state: MouseStateHandle,

    /// To ensure we only have one run of completions-as-you-type at any given time,
    /// we keep an abort handle of the current run. If we have reason to start a new run
    /// (e.g. new input), we simply abort the existing run. The same applies to the
    /// syntax highlighting and autosuggestions features (all which use the completer).
    completions_abort_handle: Option<AbortHandle>,
    decorations_future_handle: Option<SpawnedFutureHandle>,
    autosuggestions_abort_handle: Option<AbortHandle>,

    pub prompt_render_helper: PromptRenderHelper,
    prompt_type: ModelHandle<PromptType>,
    // A cached copy of enable_autosuggestions from settings (to avoid
    // a settings read on every typed character).
    enable_autosuggestions_setting: bool,

    /// Manages the input state for a shared session.
    /// Is [`Some`] iff this is a viewer in a shared session.
    shared_session_input_state: Option<SharedSessionInputState>,

    /// Manages presence state for shared session.
    ///
    /// Only [`Some`] if this is a shared session.
    shared_session_presence_manager: Option<ModelHandle<PresenceManager>>,

    /// A cache of the local buffer operations for the latest instance
    /// of the input buffer. Specifically, these only include operations
    /// resulting from local changes to the buffer (not remote changes / operations).
    /// Note that the input buffer is reinstantiated every time a command is executed,
    /// while ultimately clears this set.
    ///
    /// Today, we only expect to use this with when starting
    /// a shared session.
    ///
    /// TODO (suraj): technically, we don't need the full
    /// history for _selections_; we just need the latest.
    latest_buffer_operations: Vec<CrdtOperation>,

    /// Incoming remote edits that are not yet applied
    /// because the block ID they were meant for was
    /// not active when these operations were received.
    ///
    /// When the buffer is reinstantiated, we check
    /// if any of these pending remote edits can be flushed.
    ///
    /// Today, we only expect to use this for shared session viewers.
    deferred_remote_operations: DeferredRemoteOperations,

    /// The last block that the user ran. This is used for generating autosuggestions.
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    last_user_block_completed: Option<UserBlockCompleted>,

    hoverable_handle: MouseStateHandle,

    #[cfg(feature = "local_fs")]
    conn: Option<Arc<Mutex<SqliteConnection>>>,

    /// Cached hint text to ensure it remains stable during shell initialization hooks
    cached_agent_mode_hint_text: Option<&'static str>,

    attachment_chips: Vec<AttachmentChip>,

    is_processing_attached_images: bool,

    universal_developer_input_button_bar: ViewHandle<UniversalDeveloperInputButtonBar>,

    agent_input_footer: ViewHandle<AgentInputFooter>,
    handoff_compose_state: ModelHandle<HandoffComposeState>,

    /// Cached flag indicating whether the editor buffer is empty, used to track changes between
    /// empty and non-empty states.
    ///
    /// If simply looking for if the editor contents empty, check the editor view directly instead
    /// of using this flag.
    is_editor_empty_on_last_edit: bool,

    /// Weak handle to this input view for drop target data
    weak_view_handle: WeakViewHandle<Input>,

    agent_status_view: ViewHandle<BlocklistAIStatusBar>,
    /// Optional queued-prompts panel rendered between `agent_status_view` and the input editor.
    /// Constructed in [`Input::new`] when [`FeatureFlag::QueueSlashCommand`] is enabled.
    queued_prompts_panel: Option<ViewHandle<QueuedPromptsPanelView>>,
    agent_view_controller: ModelHandle<AgentViewController>,
    agent_shortcut_view_model: ModelHandle<AgentShortcutViewModel>,
    ambient_agent_view_state: Option<AmbientAgentViewState>,

    /// When a command is executed from a prompt chip (e.g. `cd` from the directory dropdown),
    /// we snapshot the current input contents here so we can restore them after the command
    /// completes and the buffer would normally be cleared.
    input_contents_before_prompt_chip_command: Option<String>,

    pending_shell_widget_handoff: Option<PendingShellWidgetHandoff>,
}

/// How a completed shell-widget handoff lands its selection. Fish's ctrl-t widget already performs
/// token-aware replacement and reports the whole line; bash/zsh ctrl-t report a path fragment to
/// splice at the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellWidgetApplyMode {
    Splice,
    Replace,
}

struct PendingShellWidgetHandoff {
    session_id: SessionId,
    original_buffer: String,
    selection: Option<String>,
    block_id: BlockId,
    apply_mode: ShellWidgetApplyMode,
    cursor_offset: Option<ByteOffset>,
}

impl PendingShellWidgetHandoff {
    fn maybe_apply_selection(&mut self, session_id: SessionId, selection: &str) {
        if self.session_id != session_id {
            return;
        }
        if !selection.is_empty() {
            self.selection = Some(selection.to_string());
        }
    }

    fn restore_text(&self) -> &str {
        match (self.apply_mode, &self.selection) {
            (ShellWidgetApplyMode::Replace, Some(selection)) => selection,
            (ShellWidgetApplyMode::Replace, None) | (ShellWidgetApplyMode::Splice, _) => {
                &self.original_buffer
            }
        }
    }
}

struct AmbientAgentViewState {
    view_model: ModelHandle<AmbientAgentViewModel>,
    #[allow(dead_code)]
    harness_selector: ViewHandle<HarnessSelector>,
    host_selector: Option<ViewHandle<HostSelector>>,
    auth_secret_selector: Option<ViewHandle<AuthSecretSelector>>,
    auth_secret_ftux_view: Option<ViewHandle<AuthSecretFtuxView>>,
}

impl AmbientAgentViewState {
    fn view_model(&self) -> &ModelHandle<AmbientAgentViewModel> {
        &self.view_model
    }
}

#[derive(Clone)]
struct AttachmentChip {
    file_name: String,
    mouse_state_handle: MouseStateHandle,
    attachment_type: AttachmentType,
    /// Index into the unified pending_attachments list for deletion.
    index: usize,
}

/// A map of remote buffer operations that were deferred because
/// the corresponding block ID was not active when these operations
/// were received.
struct DeferredRemoteOperations {
    /// The latest block ID that we flushed for.
    latest_block_id: BlockId,

    /// The deferred operations.
    deferred_ops: HashMap<BlockId, Vec<CrdtOperation>>,
}

impl DeferredRemoteOperations {
    fn new(latest_block_id: BlockId) -> Self {
        Self {
            latest_block_id,
            deferred_ops: HashMap::new(),
        }
    }

    /// Defers the `operations` corresponding to the `block_id`.
    fn defer(&mut self, block_id: BlockId, operations: Vec<CrdtOperation>) {
        self.deferred_ops
            .entry(block_id)
            .or_default()
            .extend(operations);
    }

    /// Removes and returns the deferred operations for the latest block ID, if any.
    fn flush(&mut self) -> Option<Vec<CrdtOperation>> {
        self.deferred_ops.remove(&self.latest_block_id)
    }
}

/// Per-attachment outcome from [`upload_pending_attachments_to_task`].
enum TaskAttachmentUploadOutcome {
    /// Successfully uploaded to the task's storage bucket. `attachment_id` is the
    /// server-assigned identifier the new VM downloads at startup.
    Uploaded {
        attachment_id: String,
        file_name: String,
    },
    /// Could not be uploaded — decode error, size limit exceeded, or HTTP failure.
    /// `error` is a human-readable message suitable for display.
    Failed { file_name: String, error: String },
}

/// Decode, size-check, and upload `pending_attachments` to the given task's storage
/// via presigned upload targets obtained from the server. Returns one
/// [`TaskAttachmentUploadOutcome`] per input attachment in the same order.
///
/// The outer `Err` is returned only when [`AIClient::prepare_attachments_for_upload`] fails
/// (meaning no individual uploads were attempted). Decode errors, size-limit violations,
/// and individual HTTP failures are surfaced as [`TaskAttachmentUploadOutcome::Failed`]
/// entries so each caller can choose its own error-handling policy (fail-fast vs. best-effort).
async fn upload_pending_attachments_to_task(
    ai_client: Arc<dyn AIClient>,
    server_api: Arc<ServerApi>,
    task_id: crate::ai::ambient_agents::AmbientAgentTaskId,
    pending_attachments: Vec<PendingAttachment>,
) -> anyhow::Result<Vec<TaskAttachmentUploadOutcome>> {
    let n = pending_attachments.len();
    // Reserve a slot for each input attachment; filled below in original order.
    let mut outcomes: Vec<Option<TaskAttachmentUploadOutcome>> =
        std::iter::repeat_with(|| None).take(n).collect();
    // Collect successfully decoded files together with their original index so we can
    // zip the prepare-upload response back to the correct outcome slot.
    let mut files_to_upload: Vec<(usize, String, String, Vec<u8>)> = Vec::new();

    for (i, attachment) in pending_attachments.into_iter().enumerate() {
        let decoded = match attachment {
            PendingAttachment::File(file) => std::fs::read(&file.file_path)
                .map(|bytes| (file.file_name.clone(), file.mime_type.clone(), bytes))
                .map_err(|e| (file.file_name, format!("Failed to read attachment: {e}"))),
            PendingAttachment::Image(image) => base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .map(|bytes| (image.file_name.clone(), image.mime_type.clone(), bytes))
                .map_err(|e| (image.file_name, format!("Failed to decode attachment: {e}"))),
        };
        match decoded {
            Ok((file_name, mime_type, bytes)) => {
                if bytes.len() > MAX_ATTACHMENT_SIZE_BYTES {
                    outcomes[i] = Some(TaskAttachmentUploadOutcome::Failed {
                        file_name: file_name.clone(),
                        error: format!("{file_name} exceeds the 10 MB attachment limit"),
                    });
                } else {
                    files_to_upload.push((i, file_name, mime_type, bytes));
                }
            }
            Err((file_name, error)) => {
                outcomes[i] = Some(TaskAttachmentUploadOutcome::Failed { file_name, error });
            }
        }
    }

    if !files_to_upload.is_empty() {
        let file_infos: Vec<AttachmentFileInfo> = files_to_upload
            .iter()
            .map(|(_, name, mime, _)| AttachmentFileInfo {
                filename: name.clone(),
                mime_type: mime.clone(),
            })
            .collect();

        let prepare_response = ai_client
            .prepare_attachments_for_upload(&task_id, &file_infos)
            .await?;

        if prepare_response.attachments.len() != files_to_upload.len() {
            anyhow::bail!(
                "Attachment upload preparation returned {} targets for {} files",
                prepare_response.attachments.len(),
                files_to_upload.len()
            );
        }

        for ((orig_idx, file_name, mime_type, file_bytes), upload_info) in files_to_upload
            .iter()
            .zip(prepare_response.attachments.iter())
        {
            let target = upload_info.resolve_upload_target(mime_type);
            let result =
                upload_to_target(server_api.http_client(), &target, file_bytes.clone()).await;

            outcomes[*orig_idx] = Some(match result {
                Ok(()) => TaskAttachmentUploadOutcome::Uploaded {
                    attachment_id: upload_info.attachment_id.clone(),
                    file_name: file_name.clone(),
                },
                Err(e) => TaskAttachmentUploadOutcome::Failed {
                    file_name: file_name.clone(),
                    error: format!("{e:#}"),
                },
            });
        }
    }

    Ok(outcomes
        .into_iter()
        .map(|o| o.expect("all slots filled during upload_pending_attachments_to_task"))
        .collect())
}

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    if cfg!(feature = "integration_tests") {
        app.register_fixed_bindings([
            // Hack: Add explicit ctrl-r binding for integration tests, since the tests' injected
            // keypresses won't trigger Mac menu items. Unfortunately we can't use
            // cfg[test] because we are a separate process!
            FixedBinding::new(
                "ctrl-r",
                WorkspaceAction::ShowCommandSearch(Default::default()),
                id!("Input"),
            ),
        ]);
    }

    app.register_fixed_bindings(vec![
        FixedBinding::new("ctrl-d", InputAction::CtrlD, id!("Input")),
        FixedBinding::custom(
            CustomAction::History,
            InputAction::Up,
            "Show History",
            // We need to ensure the workflow info box is not open as the "up" arrow
            // key is used to navigate the environment variables dropdown.
            // Same goes with the LLM menu.
            id!("Input")
                & !id!("IMEOpen")
                & !id!("WorkflowInfoBox")
                & !id!("AgentSelectorOpen")
                & !id!("PromptChipMenuOpen")
                & !id!(QUEUED_PROMPT_INLINE_EDITOR_OPEN_CONTEXT),
        ),
    ]);

    app.register_editable_bindings([EditableBinding::new(
        "input:insert_network_logging_workflow",
        "Show Warp network log",
        WorkspaceAction::OpenNetworkLogPane,
    )
    .with_enabled(|| ContextFlag::NetworkLogConsole.is_enabled())]);

    app.register_editable_bindings([EditableBinding::new(
        "input:clear_screen",
        "Clear screen",
        InputAction::ClearScreen,
    )
    .with_context_predicate(id!("Input"))
    .with_key_binding("ctrl-l")]);

    app.register_editable_bindings([
        EditableBinding::new(
            "terminal:scroll_up_one_page",
            "Scroll terminal output up one page",
            InputAction::PageUp,
        )
        .with_context_predicate(id!("Input") & !id!("IMEOpen"))
        .with_key_binding("pageup"),
        EditableBinding::new(
            "terminal:scroll_down_one_page",
            "Scroll terminal output down one page",
            InputAction::PageDown,
        )
        .with_context_predicate(id!("Input") & !id!("IMEOpen"))
        .with_key_binding("pagedown"),
    ]);

    app.register_editable_bindings([EditableBinding::new(
        "workspace:edit_prompt",
        BindingDescription::new("Edit Prompt")
            .with_custom_description(bindings::MAC_MENUS_CONTEXT, "Edit Prompt"),
        WorkspaceAction::OpenPromptEditor {
            open_source: PromptEditorOpenSource::CommandPalette,
        },
    )
    .with_group(bindings::BindingGroup::Settings.as_str())
    .with_context_predicate(
        id!("Input")
            & id!(SharedSessionStatus::ActiveSharer.as_keymap_context())
            & !id!("LongRunningCommand")
            & !id!(flags::ACTIVE_AGENT_VIEW)
            & !id!(flags::ACTIVE_INLINE_AGENT_VIEW),
    )]);

    if FeatureFlag::ClassicCompletions.is_enabled()
        && !FeatureFlag::ForceClassicCompletions.is_enabled()
    {
        app.register_editable_bindings([EditableBinding::new(
            "input:toggle_classic_completions_mode",
            "(Experimental) Toggle classic completions mode",
            InputAction::ToggleClassicCompletionsMode,
        )
        .with_context_predicate(id!("Input"))]);
    }

    // Register editable bindings relating to Command Search.
    app.register_editable_bindings([
        EditableBinding::new(
            "workspace:show_command_search",
            "Command Search",
            WorkspaceAction::ShowCommandSearch(Default::default()),
        )
        // Only show command search if none of the input-related panels are open, and if we aren't
        // in Vim normal mode. Command Search is ctrl-r by default, and so is Redo in Vim (in
        // normal mode). So, the child should be allowed to handle this action first. Child views
        // normally do get first precedence to handle keybindings, but this is _not_ the case when
        // a parent view binds a CustomAction, which is what is happening here in the Input view.
        // Therefore, this binding is guarded with !id!("VimNormalMode"). Note that although there
        // is usually a conflict between these, that isn't always the case if the user has
        // re-mapped CommandSearch to something else. However, we don't account for that here.
        .with_context_predicate(id!("Input") & !id!("VimNormalMode"))
        .with_custom_action(CustomAction::CommandSearch),
        EditableBinding::new(
            "input:search_command_history",
            "History Search",
            WorkspaceAction::ShowCommandSearch(CommandSearchOptions {
                filter: Some(QueryFilter::History),
                init_content: Default::default(),
            }),
        )
        .with_context_predicate(id!("Input"))
        .with_custom_action(CustomAction::HistorySearch),
        EditableBinding::new(
            OPEN_COMPLETIONS_KEYBINDING_NAME,
            "Open completions menu",
            InputAction::MaybeOpenCompletionSuggestions,
        )
        .with_context_predicate(id!("Input"))
        .with_key_binding("tab"),
        EditableBinding::new(
            "workspace:trigger_external_ctrl_t_file_search",
            "External File Search",
            WorkspaceAction::TriggerExternalCtrlTFileSearch,
        )
        .with_enabled(|| FeatureFlag::ShellWidgetHandoff.is_enabled())
        .with_context_predicate(id!("Input") & !id!("LongRunningCommand"))
        .with_key_binding("ctrl-t"),
        EditableBinding::new(
            "workspace:trigger_external_alt_c_directory_search",
            "External Directory Search",
            WorkspaceAction::TriggerExternalAltCDirectorySearch,
        )
        .with_enabled(|| FeatureFlag::ShellWidgetHandoff.is_enabled())
        .with_context_predicate(id!(EXTERNAL_ALT_C_BINDING_CONTEXT))
        .with_key_binding("alt-c"),
    ]);

    app.register_editable_bindings([
        EditableBinding::new(
            START_NEW_CONVERSATION_KEYBINDING_NAME,
            "New agent conversation",
            InputAction::StartNewAgentConversation {
                origin: AgentViewEntryOrigin::Input {
                    was_prompt_autodetected: false,
                },
            },
        )
        .with_enabled(|| !FeatureFlag::AgentView.is_enabled())
        .with_group(bindings::BindingGroup::WarpAi.as_str())
        .with_context_predicate(
            id!("Input") & id!(flags::IS_ANY_AI_ENABLED) & id!("TerminalView_NonEmptyBlockList"),
        )
        .with_mac_key_binding("cmd-shift-N")
        .with_linux_or_windows_key_binding("ctrl-alt-shift-N"),
        EditableBinding::new(
            "input:enable_auto_detection",
            "Trigger Auto Detection",
            InputAction::EnableAutoDetection,
        )
        .with_enabled(|| FeatureFlag::AgentMode.is_enabled())
        .with_group(bindings::BindingGroup::WarpAi.as_str())
        .with_context_predicate(
            id!("Input")
                & id!("UniversalDeveloperInput")
                & id!(flags::IS_ANY_AI_ENABLED)
                & !id!("IMEOpen"),
        )
        .with_key_binding("alt-shift-I"),
    ]);

    if FeatureFlag::AgentView.is_enabled() {
        app.register_fixed_bindings([FixedBinding::new(
            "shift-?",
            InputAction::ToggleAgentViewShortcuts,
            id!("Input")
                & !id!("IMEOpen")
                & id!(flags::EMPTY_INPUT_BUFFER)
                & id!(flags::ACTIVE_AGENT_VIEW)
                & !id!("LongRunningCommand")
                & !id!(QUEUED_PROMPT_INLINE_EDITOR_OPEN_CONTEXT)
                & !(id!(flags::TERMINAL_MODE_INPUT) & id!(flags::LOCKED_INPUT)),
        )]);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompletionsTrigger {
    Keybinding,
    AsYouType,
}

/// Represents whether the input editor should render the subshell flag.
#[derive(Clone, Debug)]
enum SubshellRenderState {
    /// Contains the subshell-spawning command for the flag. Render the flag
    /// and extend the flag into the input editor.
    Flag(SubshellSource),
    /// The input is inside a subshell, extend the flag into the input editor,
    /// but do not render the actual flag.
    Flagpole,
}

/// Represents whether a command is currently being executed.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Executing {
    Yes,
    No,
}

impl Input {
    pub fn send_input_buffer_to_terminal_editor(
        &mut self,
        buffer_contents: Arc<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text_for_syncing_inputs(buffer_contents, ctx);
        });
    }

    pub fn run_command_in_synced_terminal_input(&mut self, ctx: &mut ViewContext<Self>) {
        self.has_pending_command = true;
        self.execute_pending_command(ctx);
    }

    /// Subscribes the input to an ambient agent view model so it re-renders on status
    /// transitions and surfaces snapshot-upload failures. Shared by [`Self::new`] and
    /// [`Self::attach_ambient_agent_view_model`] so the upfront (construction) and late
    /// (`SessionJoined`) paths wire up identical behavior.
    fn subscribe_to_ambient_agent_view_model(
        view_model: &ModelHandle<AmbientAgentViewModel>,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.subscribe_to_model(view_model, |me, handle, event, ctx| {
            // Surface async snapshot upload failures as a toast.
            if let AmbientAgentViewModelEvent::HandoffSnapshotUploadFailed { error_message } = event
            {
                let window_id = ctx.window_id();
                let toast_message = format!("Failed to prepare cloud handoff: {error_message}");
                ToastStack::handle(ctx).update(ctx, |ts, ctx| {
                    ts.add_ephemeral_toast(DismissibleToast::error(toast_message), window_id, ctx);
                });
            }

            // Re-render on status-footer transitions and on status-affecting events that
            // decide whether the input is in its composing shape.
            let should_notify = handle.as_ref(ctx).should_show_status_footer()
                || matches!(
                    event,
                    AmbientAgentViewModelEvent::EnteredSetupState
                        | AmbientAgentViewModelEvent::EnteredComposingState
                        | AmbientAgentViewModelEvent::DispatchedAgent
                        | AmbientAgentViewModelEvent::SessionReady { .. }
                        | AmbientAgentViewModelEvent::Failed { .. }
                        | AmbientAgentViewModelEvent::Cancelled
                        | AmbientAgentViewModelEvent::NeedsGithubAuth
                        | AmbientAgentViewModelEvent::HarnessSelected
                        | AmbientAgentViewModelEvent::PendingHandoffChanged
                        | AmbientAgentViewModelEvent::HandoffSnapshotUploadFailed { .. }
                );

            if should_notify {
                me.set_zero_state_hint_text(ctx);
                ctx.notify();
            }
        });
    }

    /// Builds the cloud-mode harness selector for an ambient agent view model. Shared by
    /// [`Self::new`] and [`Self::attach_ambient_agent_view_model`] so construction and late
    /// attach produce the same selector wiring.
    fn build_harness_selector(
        view_model: ModelHandle<AmbientAgentViewModel>,
        menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<HarnessSelector> {
        let harness_selector = ctx.add_typed_action_view(|ctx| {
            HarnessSelector::new(menu_positioning_provider.clone(), view_model.clone(), ctx)
        });
        if FeatureFlag::CloudModeInputV2.is_enabled() {
            harness_selector.update(ctx, |selector, ctx| {
                selector.set_button_theme(NakedHeaderButtonTheme, ctx);
            });
        }
        // Mirror the V2 model selector / host selector refocus path: when the
        // harness selector menu closes (item picked or dismissed via Esc /
        // click-outside), restore focus to the input editor so typing resumes
        // immediately. This powers the "input is focused after the harness
        // selector closes" UX for the `/harness` slash command.
        ctx.subscribe_to_view(&harness_selector, |me, _, event, ctx| {
            let HarnessSelectorEvent::MenuVisibilityChanged { open } = event;
            if !*open {
                me.focus_input_box(ctx);
            }
        });
        harness_selector
    }

    /// Builds the cloud-mode host selector for an ambient agent view model. Composer-only
    /// (a viewer of an existing run does not choose a host). Shared by
    /// [`Self::attach_ambient_agent_view_model`], which is the single wiring point for both the
    /// eager (`Input::new`) and lazy (`SessionJoined`) paths.
    fn build_host_selector(
        view_model: ModelHandle<AmbientAgentViewModel>,
        menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<HostSelector> {
        let view = ctx
            .add_typed_action_view(|ctx| HostSelector::new(menu_positioning_provider.clone(), ctx));
        let weak_view = view.downgrade();
        let effective_host = effective_default_host(&weak_view, ctx);
        if let Some(slug) = &effective_host {
            view.update(ctx, |selector, ctx| {
                selector.set_default_host(slug.clone(), ctx);
            });
        }
        if let Some(slug) = effective_host {
            view_model.update(ctx, |model, _ctx| {
                model.set_worker_host(Some(slug));
            });
        }
        // When the host selector menu closes (item picked or dismissed via Esc / click-outside),
        // restore focus to the input editor so typing resumes immediately.
        ctx.subscribe_to_view(&view, |me, _, event, ctx| {
            if matches!(
                event,
                HostSelectorEvent::MenuVisibilityChanged { open: false }
            ) {
                me.focus_input_box(ctx);
            }
        });
        // Propagate host selection changes to the view model when a host is explicitly selected,
        // rather than on menu close, to avoid a race where the menu closes before the selection
        // updates.
        let vm_for_host = view_model.clone();
        ctx.subscribe_to_view(&view, move |_me, handle, event, ctx| {
            if matches!(event, HostSelectorEvent::HostSelected) {
                let selected = handle.as_ref(ctx).selected().clone();
                vm_for_host.update(ctx, |model, _ctx| {
                    model.set_worker_host(selected.worker_host_value());
                });
            }
        });
        // Keep the host selector and view model in sync when the host this window should
        // default to changes: because the admin edited the team's `default_host_slug`, or
        // because the window moved to a team that configures a different one.
        let view_for_ws = view.clone();
        let vm_for_ws = view_model.clone();
        ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), move |_me, _, event, ctx| {
            // Windows are independent, so a sibling window switching team must not retarget
            // this one.
            let affects_this_window = matches!(event, UserWorkspacesEvent::TeamsChanged)
                || matches!(
                    event,
                    UserWorkspacesEvent::WindowTeamChanged { window_id }
                        if *window_id == ctx.window_id()
                );
            if !affects_this_window {
                return;
            }
            let scope = UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx);
            ConnectedSelfHostedWorkersModel::handle(ctx).update(ctx, |model, ctx| {
                model.refresh(&scope, ctx);
            });
            // `None` has to be applied, not skipped: it means the window's team configures no
            // self-hosted default, and leaving the previous value in place would keep the
            // selector and the run config pointed at another team's worker.
            let effective_host = effective_default_host(&weak_view, ctx);
            match effective_host.clone() {
                Some(slug) => view_for_ws.update(ctx, |selector, ctx| {
                    selector.set_default_host(slug, ctx);
                }),
                None => view_for_ws.update(ctx, |selector, ctx| {
                    selector.clear_default_host(ctx);
                }),
            }
            vm_for_ws.update(ctx, |model, _ctx| {
                model.set_worker_host(effective_host);
            });
        });
        view
    }

    /// Builds the cloud-mode auth-secret selector and its FTUX view for an ambient agent view
    /// model. Composer-only. Shared by [`Self::attach_ambient_agent_view_model`], which is the
    /// single wiring point for both the eager (`Input::new`) and lazy (`SessionJoined`) paths.
    fn build_auth_secret_selector(
        view_model: ModelHandle<AmbientAgentViewModel>,
        menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
        ctx: &mut ViewContext<Self>,
    ) -> (
        ViewHandle<AuthSecretSelector>,
        ViewHandle<AuthSecretFtuxView>,
    ) {
        let selector = ctx.add_typed_action_view(|ctx| {
            AuthSecretSelector::new(menu_positioning_provider.clone(), view_model.clone(), ctx)
        });
        ctx.subscribe_to_view(&selector, |me, _, event, ctx| match event {
            AuthSecretSelectorEvent::MenuVisibilityChanged { open: false } => {
                me.focus_input_box(ctx);
            }
            AuthSecretSelectorEvent::NewTypeSelected {
                harness,
                type_index,
            } => {
                if let Some(ftux_view) = me.auth_secret_ftux_view().cloned() {
                    let harness = *harness;
                    let type_index = *type_index;
                    ftux_view.update(ctx, |view, ctx| {
                        view.enter_creation_state_public(harness, type_index, ctx);
                    });
                }
                ctx.notify();
            }
            AuthSecretSelectorEvent::DeleteConfirmationDialogToggled { is_open } => {
                ctx.emit(Event::AuthSecretDeleteConfirmationDialogToggled { is_open: *is_open });
            }
            AuthSecretSelectorEvent::MenuVisibilityChanged { open: true } => {}
        });
        let initial_harness = view_model.as_ref(ctx).selected_harness();
        let ftux_view =
            ctx.add_typed_action_view(|ctx| AuthSecretFtuxView::new(initial_harness, ctx));

        // Forward the pane's harness changes into the FTUX view.
        let ftux_for_harness_sub = ftux_view.clone();
        ctx.subscribe_to_model(&view_model, move |_me, vm, event, ctx| {
            if matches!(event, AmbientAgentViewModelEvent::HarnessSelected) {
                let harness = vm.as_ref(ctx).selected_harness();
                ftux_for_harness_sub.update(ctx, |view, ctx| {
                    view.set_harness(harness, ctx);
                });
            }
        });

        let vm_for_events = view_model.clone();
        ctx.subscribe_to_view(&ftux_view, move |_me, _, event, ctx| match event {
            AuthSecretFtuxViewEvent::SecretSelected { harness, name }
            | AuthSecretFtuxViewEvent::Created { harness, name } => {
                let harness = *harness;
                let name = name.clone();
                vm_for_events.update(ctx, |model, ctx| {
                    model.set_harness_auth_secret_name(Some(name.clone()), ctx);
                });
                let team_scope = UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx);
                CloudAgentSettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings.mark_harness_auth_ftux_completed(harness, ctx);
                    settings.persist_auth_secret_preference(
                        &team_scope,
                        harness,
                        Some(AuthSecretPreference::Named(name)),
                        ctx,
                    );
                });
            }
            AuthSecretFtuxViewEvent::Cancelled => {
                vm_for_events.update(ctx, |model, ctx| {
                    model.set_harness(Harness::Oz, ctx);
                });
            }
            AuthSecretFtuxViewEvent::Skipped { harness } => {
                let harness = *harness;
                let team_scope = UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx);
                CloudAgentSettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings.mark_harness_auth_ftux_completed(harness, ctx);
                    settings.persist_auth_secret_preference(
                        &team_scope,
                        harness,
                        Some(AuthSecretPreference::Inherit),
                        ctx,
                    );
                });
            }
            AuthSecretFtuxViewEvent::Failed { .. } => {}
        });
        (selector, ftux_view)
    }

    /// Wires an ambient agent view model into this input. This is the SINGLE wiring point,
    /// invoked by both `Input::new` (eager/composer, when a model is supplied at construction)
    /// and the shared-session viewer's `SessionJoined` path (lazy, e.g. a raw `shared_session`
    /// link that turns out to be a cloud run). Idempotent: a no-op when already wired. Builds the
    /// composer-only sub-views (host / auth-secret / FTUX selectors) only for a non-viewer, since
    /// a viewer of an existing run does not compose a new run.
    pub(crate) fn attach_ambient_agent_view_model(
        &mut self,
        view_model: ModelHandle<AmbientAgentViewModel>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.ambient_agent_view_state.is_some() {
            return;
        }
        Self::subscribe_to_ambient_agent_view_model(&view_model, ctx);
        let harness_selector = Self::build_harness_selector(
            view_model.clone(),
            self.menu_positioning_provider.clone(),
            ctx,
        );
        // Push the model down to the footer (parent -> child) so its environment selector
        // reflects the cloud run. On this link-join path the footer captured `None` at
        // construction, so it must be given the model now to render the environment chip.
        let footer_model = view_model.clone();
        let footer_menu_positioning = self.menu_positioning_provider.clone();
        self.agent_input_footer.update(ctx, |footer, ctx| {
            footer.set_ambient_agent_view_model(footer_model, footer_menu_positioning, ctx);
        });
        // Same parent -> child push for the agent status bar. It captured `None` at construction
        // on this link-join path, so without this it can't render the cloud-mode setup /
        // follow-up progress (`render_cloud_mode_setup_status`) while a follow-up VM spins up.
        let status_bar_model = view_model.clone();
        self.agent_status_view.update(ctx, |status_bar, ctx| {
            status_bar.set_ambient_agent_view_model(status_bar_model, ctx);
        });
        // NOTE: This method is the SINGLE point that wires a (lazily- or eagerly-created) ambient
        // view model into the input tree. Both `Input::new` (eager/composer) and the shared-session
        // viewer's `SessionJoined` path (lazy) funnel through here, so any component that captures
        // `Option<ModelHandle<AmbientAgentViewModel>>` must be wired here (via its
        // `set_ambient_agent_view_model` setter) rather than at construction, otherwise the two
        // paths drift. The input subscription, harness selector, footer environment selector,
        // display chips, and agent status bar are wired here.
        //
        // The host / auth-secret / FTUX selectors are composer-only: a viewer of an existing run
        // does not compose a new run, so they stay `None` for a shared-session viewer.
        let is_cloud_mode_composer = self.model.lock().is_dummy_cloud_mode_session();
        let (host_selector, auth_secret_selector, auth_secret_ftux_view) = if is_cloud_mode_composer
            && FeatureFlag::CloudModeInputV2.is_enabled()
        {
            let host_selector = Self::build_host_selector(
                view_model.clone(),
                self.menu_positioning_provider.clone(),
                ctx,
            );
            // Re-render when connected workers change so the host selector shows/hides
            // (it isn't mounted while hidden to drive this itself).
            ctx.subscribe_to_model(
                &ConnectedSelfHostedWorkersModel::handle(ctx),
                |_me, _, event, ctx| {
                    if matches!(event, ConnectedSelfHostedWorkersEvent::Changed) {
                        ctx.notify();
                    }
                },
            );
            let (auth_secret_selector, auth_secret_ftux_view) = Self::build_auth_secret_selector(
                view_model.clone(),
                self.menu_positioning_provider.clone(),
                ctx,
            );
            (
                Some(host_selector),
                Some(auth_secret_selector),
                Some(auth_secret_ftux_view),
            )
        } else {
            (None, None, None)
        };
        self.ambient_agent_view_state = Some(AmbientAgentViewState {
            view_model,
            harness_selector,
            host_selector,
            auth_secret_selector,
            auth_secret_ftux_view,
        });
        ctx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        model: Arc<FairMutex<TerminalModel>>,
        tips_completed: ModelHandle<TipsCompleted>,
        sessions: ModelHandle<Sessions>,
        size_info: SizeInfo,
        menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
        current_prompt: ModelHandle<PromptType>,
        ai_controller: ModelHandle<BlocklistAIController>,
        ai_context_model: ModelHandle<BlocklistAIContextModel>,
        ai_input_model: ModelHandle<BlocklistAIInputModel>,
        ai_action_model: ModelHandle<BlocklistAIActionModel>,
        cli_subagent_controller: ModelHandle<CLISubagentController>,
        terminal_view_id: EntityId,
        current_repo_path: Option<PathBuf>,
        model_events: ModelHandle<crate::terminal::model_events::ModelEventDispatcher>,
        agent_view_controller: ModelHandle<AgentViewController>,
        ambient_agent_view_model: Option<ModelHandle<AmbientAgentViewModel>>,
        ephemeral_message_model: ModelHandle<EphemeralMessageModel>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let initial_session_context = {
            let completer_data = CompleterData::new(
                sessions.clone(),
                None, // active_block_metadata will be set later when blocks are available
                CommandRegistry::global_instance(),
                None, // last_user_block_completed will be set later
            );
            completer_data.completion_session_context(ctx)
        };

        let is_shared_session_viewer = model.lock().shared_session_status().is_viewer();
        let handoff_compose_state = ctx.add_model(|_ctx| HandoffComposeState::default());
        ctx.subscribe_to_model(&handoff_compose_state, |me, _, _, ctx| {
            me.set_zero_state_hint_text(ctx);
            ctx.notify();
        });

        let footer_display_chip_config = DisplayChipConfig {
            ai_input_model: ai_input_model.clone(),
            ai_context_model: ai_context_model.clone(),
            terminal_view_id,
            menu_positioning_provider: menu_positioning_provider.clone(),
            session_context: initial_session_context.clone(),
            current_repo_path: current_repo_path.clone(),
            model_events: model_events.clone(),
            is_shared_session_viewer,
            agent_view_controller: agent_view_controller.clone(),
            // Wired post-construction via `attach_ambient_agent_view_model` (single wiring point).
            ambient_agent_view_model: None,
        };

        let prompt_view = ctx.add_typed_action_view(|ctx| {
            PromptDisplay::new(
                current_prompt.clone(),
                ai_input_model.clone(),
                ai_context_model.clone(),
                terminal_view_id,
                menu_positioning_provider.clone(),
                initial_session_context.clone(),
                current_repo_path.clone(),
                model_events.clone(),
                agent_view_controller.clone(),
                is_shared_session_viewer,
                ctx,
            )
        });
        ctx.subscribe_to_view(&prompt_view, |me, _, event, ctx| {
            me.handle_prompt_event(event, ctx);
        });
        ctx.subscribe_to_model(&Appearance::handle(ctx), move |me, _, event, ctx| {
            if let AppearanceEvent::ThemeChanged = event {
                me.handle_theme_change(ctx);
            }
        });
        ctx.subscribe_to_model(&TerminalSettings::handle(ctx), move |_, _, event, ctx| {
            if let TerminalSettingsChangedEvent::Spacing { .. } = event {
                ctx.notify();
            }
        });
        ctx.subscribe_to_model(&agent_view_controller, |me, _, event, ctx| {
            use crate::ai::blocklist::agent_view::AgentViewControllerEvent;
            if let AgentViewControllerEvent::EnteredAgentView { origin, .. } = event {
                me.close_suggestion_modes_for_new_conversation(ctx);
                // Entering Agent View can remove multiline same-line prompt decorator content in a
                // single render pass. Reset shrink-delay so we don't hold onto stale input height
                // for one frame (which shows up as extra bottom padding/jitter).
                me.editor.update(ctx, |editor, ctx| {
                    editor.reset_height_shrink_delay(ctx);
                });

                if *origin == AgentViewEntryOrigin::CloudAgent {
                    // By default, shared session viewers cannot edit the input - override that for composing ambient agent queries.
                    me.editor.update(ctx, |editor, ctx| {
                        editor.set_interaction_state(InteractionState::Editable, ctx);
                    });
                    me.set_zero_state_hint_text(ctx);
                }
            }
            ctx.notify();
        });

        let prompt_selection_state_handle = SelectionHandle::default();

        let view_id = ctx.view_id();

        let input_render_state_model_handle: ModelHandle<InputRenderStateModel> =
            ctx.add_model(|_| InputRenderStateModel::new(false, size_info));

        let universal_developer_input_button_bar = ctx.add_typed_action_view(|ctx| {
            UniversalDeveloperInputButtonBar::new(
                terminal_view_id,
                ai_input_model.clone(),
                cli_subagent_controller.clone(),
                model.clone(),
                ctx,
            )
        });
        ctx.subscribe_to_view(
            &universal_developer_input_button_bar,
            |me, _, event, ctx| {
                me.handle_universal_developer_input_button_bar_event(event, ctx);
            },
        );
        let agent_input_footer = ctx.add_typed_action_view(|ctx| {
            AgentInputFooter::new(
                menu_positioning_provider.clone(),
                terminal_view_id,
                model.clone(),
                // Wired post-construction via `attach_ambient_agent_view_model`.
                None,
                handoff_compose_state.clone(),
                current_prompt.clone(),
                footer_display_chip_config.clone(),
                ctx,
            )
        });

        // Ambient view state (harness / host / auth selectors) is built in
        // `attach_ambient_agent_view_model`, the single wiring point shared by this constructor
        // and the lazy shared-session viewer path.
        let ambient_agent_view_state: Option<AmbientAgentViewState> = None;
        ctx.subscribe_to_view(&agent_input_footer, |me, _, event, ctx| {
            match event {
                AgentInputFooterEvent::SelectFile => {
                    me.select_image(ctx);
                }
                AgentInputFooterEvent::StartRemoteControl
                | AgentInputFooterEvent::StopRemoteControl => {
                    // Handled by UseAgentToolbar's subscription, not here.
                }
                // The file explorer event is handled by UseAgentToolbar's subscription.
                AgentInputFooterEvent::ToggleFileExplorer => {}
                AgentInputFooterEvent::ToggledChipMenu { open } => {
                    me.handle_prompt_event(&PromptDisplayEvent::ToggleMenu { open: *open }, ctx);
                }
                AgentInputFooterEvent::TryExecuteChipCommand(cmd) => {
                    me.handle_prompt_event(
                        &PromptDisplayEvent::TryExecuteCommand(cmd.clone()),
                        ctx,
                    );
                }
                AgentInputFooterEvent::PromptAlert(prompt_alert_event) => {
                    me.handle_prompt_alert(prompt_alert_event, ctx);
                }
                AgentInputFooterEvent::EnvironmentSelectorClosed => {
                    me.focus_input_box(ctx);
                }
                AgentInputFooterEvent::OpenSettings(section) => {
                    ctx.emit(Event::OpenSettings(*section));
                }
                AgentInputFooterEvent::OpenCodeReview => {
                    ctx.emit(Event::OpenCodeReviewPane);
                }
                AgentInputFooterEvent::OpenAIDocument {
                    document_id,
                    document_version,
                } => {
                    ctx.emit(Event::ToggleAIDocumentPane {
                        document_id: *document_id,
                        document_version: *document_version,
                    });
                }
                AgentInputFooterEvent::ShowContextMenu { position } => {
                    let position_id = format!("prompt_area_{}", me.view_id);
                    let offset = if let Some(prompt_rect) = ctx.element_position_by_id(&position_id)
                    {
                        *position - prompt_rect.origin()
                    } else {
                        *position
                    };
                    ctx.dispatch_typed_action(&TerminalAction::PromptContextMenu {
                        position_offset_from_prompt: offset,
                    });
                }
                AgentInputFooterEvent::OpenEnvironmentManagementPane => {
                    ctx.emit(Event::OpenEnvironmentManagementPane);
                }
                AgentInputFooterEvent::HandoffChipClicked => {
                    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
                    if me.block_cloud_handoff_if_model_unsupported(ctx) {
                        return;
                    }

                    // Auto-handoff only when the input buffer is empty and the
                    // source conversation has content. Otherwise enter `&`
                    // compose mode so any in-flight prompt is preserved and
                    // the user can refine before forking.
                    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
                    let auto_handoff = me.editor.as_ref(ctx).buffer_text(ctx).trim().is_empty()
                        && me.source_conversation_has_content(ctx);
                    #[cfg(not(all(feature = "local_fs", not(target_family = "wasm"))))]
                    let auto_handoff = false;

                    if auto_handoff {
                        #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
                        ctx.dispatch_typed_action_deferred(
                            WorkspaceAction::OpenLocalToCloudHandoffPane {
                                launch: None,
                                environment_id: None,
                                entry_point: HandoffEntryPoint::FooterChip,
                            },
                        );
                    } else {
                        me.activate_cloud_handoff_compose(HandoffEntryPoint::FooterChip, ctx);
                    }
                }
            }
        });
        let prompt_render_helper = PromptRenderHelper::new(
            sessions.clone(),
            prompt_view,
            prompt_selection_state_handle,
            view_id,
            input_render_state_model_handle.clone(),
            ai_input_model.clone(),
        );

        let ai_follow_up_icon_mouse_state = MouseStateHandle::default();
        let editor = {
            // Clones used in render_decorator_elements closure below.
            let prompt_render_helper_clone = prompt_render_helper.clone();
            let model_clone = model.clone();
            // Clone used in keymap_context_modifier closure below.
            let input_render_state_model_handle_clone = input_render_state_model_handle.clone();

            let ai_context_model_clone = ai_context_model.clone();
            let ai_input_model = ai_input_model.clone();

            ctx.subscribe_to_model(&ai_input_model, |me, _, _, ctx| {
                me.update_image_context_options(ctx);
                me.update_editor_input_mode(ctx);
            });

            let ai_input_model_clone = ai_input_model.clone();
            let ai_follow_up_icon_mouse_state_clone = ai_follow_up_icon_mouse_state.clone();
            let agent_view_controller_clone = agent_view_controller.clone();
            let other_agent_view_controller_clone = agent_view_controller.clone();
            let handoff_compose_state_for_decorator = handoff_compose_state.clone();

            ctx.add_typed_action_view(|ctx| {
                let options = EditorOptions {
                    autogrow: true,
                    autocomplete_symbols: true,
                    propagate_and_no_op_vertical_navigation_keys:
                        PropagateAndNoOpNavigationKeys::Always,
                    propagate_horizontal_navigation_keys:
                        PropagateHorizontalNavigationKeys::AtBoundary,
                    propagate_and_no_op_escape_key: PropagateAndNoOpEscapeKey::PropagateFirst,
                    soft_wrap: true,
                    supports_vim_mode: true,
                    use_settings_line_height_ratio: true,
                    render_decorator_elements: Some(Box::new(
                        move |app| -> EditorDecoratorElements {
                            let terminal_model = model_clone.lock();
                            let prompt_block = terminal_model
                                .prompt_block()
                                .unwrap_or_else(|| terminal_model.block_list().active_block());

                            let mut editor_decorator_elements = EditorDecoratorElements::default();

                            let is_universal_developer_input_enabled = InputSettings::as_ref(app)
                                .is_universal_developer_input_enabled(app);

                            if (!FeatureFlag::AgentView.is_enabled()
                                || !agent_view_controller_clone.as_ref(app).is_active())
                                && should_render_prompt_using_editor_decorator_elements(
                                    is_universal_developer_input_enabled,
                                    &ai_input_model,
                                    &terminal_model,
                                    app,
                                )
                            {
                                let SameLinePromptElements {
                                    lprompt_top,
                                    lprompt_bottom,
                                    rprompt,
                                } = prompt_render_helper_clone.render_same_line_prompt_areas(
                                    &terminal_model,
                                    Appearance::as_ref(app),
                                    app,
                                );

                                editor_decorator_elements.top_section = lprompt_top;
                                editor_decorator_elements.left_notch = lprompt_bottom;
                                editor_decorator_elements.right_notch = rprompt;
                                editor_decorator_elements.right_notch_offset_px = Some(
                                    prompt_block.rprompt_render_offset(
                                        &input_render_state_model_handle_clone
                                            .as_ref(app)
                                            .size_info,
                                    ),
                                )
                            }

                            // Render the AI mode indicator to the left of the editor if we're in AI mode or the AI suggested a command.
                            // Also renders the reply icon when following up in an existing conversation.
                            if let Some(ai_input_indicator) = maybe_render_ai_input_indicators(
                                &ai_input_model,
                                &ai_context_model_clone,
                                &agent_view_controller_clone,
                                &handoff_compose_state_for_decorator,
                                ai_follow_up_icon_mouse_state_clone.clone(),
                                app,
                            ) {
                                editor_decorator_elements.left_notch =
                                    match editor_decorator_elements.left_notch {
                                        Some(left_notch) => {
                                            // If there is already a left notch, place the  to
                                            // the right of the notch to keep the pill immediately
                                            // to the left of the editor.
                                            Some(
                                                Flex::row()
                                                    .with_child(left_notch)
                                                    .with_child(ai_input_indicator)
                                                    .finish(),
                                            )
                                        }
                                        None => Some(ai_input_indicator),
                                    }
                            }

                            editor_decorator_elements
                        },
                    )),
                    cursor_colors_fn: Box::new(move |app| {
                        let is_ai_input_enabled =
                            ai_input_model_clone.as_ref(app).is_ai_input_enabled();
                        let appearance = Appearance::as_ref(app);
                        if is_ai_input_enabled {
                            let color_identifier = if FeatureFlag::AgentView.is_enabled() {
                                AnsiColorIdentifier::Magenta
                            } else {
                                AnsiColorIdentifier::Yellow
                            };
                            let cursor_color = color_identifier
                                .to_ansi_color(&appearance.theme().terminal_colors().normal);
                            let selection_color = ColorU::new(
                                cursor_color.r,
                                cursor_color.g,
                                cursor_color.b,
                                // Text selection color tones down the alpha to 40%.
                                (0.4 * 255.) as u8,
                            );

                            CursorColors {
                                cursor: cursor_color.into(),
                                selection: selection_color.into(),
                            }
                        } else {
                            default_cursor_colors(app)
                        }
                    }),
                    baseline_position_computation_method: BaselinePositionComputationMethod::Grid,
                    // We implement middle-click paste at the [`TerminalView`] level,
                    // and we don't want to double-paste.
                    middle_click_paste: false,
                    allow_user_cursor_preference: true,
                    delegate_paste_handling: true,
                    keymap_context_modifier: Some(Box::new(move |context, app| {
                        context
                            .set
                            .insert(flags::TERMINAL_INPUT_PAGE_KEYS_HANDLED_BY_INPUT);

                        if FeatureFlag::AgentView.is_enabled() {
                            context.set.insert(flags::AGENT_VIEW_ENABLED);
                        }

                        if !other_agent_view_controller_clone.as_ref(app).is_active()
                            && !cfg!(target_os = "macos")
                        {
                            context.set.insert(flags::CTRL_ENTER_ENTERS_AGENT_VIEW);
                        }
                    })),
                    ..Default::default()
                };
                EditorView::new(options, ctx).with_context_model(ai_context_model.clone())
            })
        };

        let buffer_model = ctx.add_model(|ctx| InputBufferModel::new(&editor, ctx));
        let suggestions_mode_model = ctx.add_model(|_| InputSuggestionsModeModel::default());

        let agent_shortcut_view_model = ctx.add_model(|ctx| {
            AgentShortcutViewModel::new(buffer_model.clone(), agent_view_controller.clone(), ctx)
        });
        ctx.subscribe_to_model(&agent_shortcut_view_model, |_, _, _, ctx| {
            ctx.notify();
        });

        current_prompt.update(ctx, |prompt_type, ctx| {
            if let PromptType::Dynamic { prompt } = prompt_type {
                prompt.update(ctx, |current_prompt, ctx| {
                    current_prompt.subscribe_to_input_editor(
                        editor.clone(),
                        agent_view_controller.clone(),
                        ctx,
                    );
                });
            }
        });

        ctx.subscribe_to_view(&editor, move |me, _, event, ctx| {
            me.handle_editor_event(event, ctx);
        });

        let input_suggestions = ctx.add_typed_action_view(InputSuggestions::new);
        ctx.subscribe_to_view(&input_suggestions, move |me, _, event, ctx| {
            me.handle_suggestions_event(event, ctx);
        });

        let safe_mode_settings = SafeModeSettings::handle(ctx);
        ctx.subscribe_to_model(&safe_mode_settings, |me, _, event, ctx| {
            me.handle_safe_mode_settings_changed_event(event, ctx)
        });

        ctx.subscribe_to_model(&InputModeSettings::handle(ctx), |_, _, _, ctx| {
            ctx.notify();
        });

        let (debounce_input_background_tx, debounce_input_background_rx) =
            async_channel::unbounded();
        let _ = ctx.spawn_stream_local(
            debounce(
                DEBOUNCE_INPUT_DECORATION_PERIOD,
                debounce_input_background_rx,
            ),
            |me, mode, ctx| me.run_input_background_jobs(mode, ctx),
            |_me, _ctx| {},
        );

        ctx.subscribe_to_model(&SessionSettings::handle(ctx), move |me, _, evt, ctx| {
            me.handle_session_settings_event(evt, ctx);
        });

        let editor_settings_handle = &AppEditorSettings::handle(ctx);
        ctx.subscribe_to_model(
            editor_settings_handle,
            Self::handle_app_editor_settings_event,
        );

        ctx.subscribe_to_model(&LigatureSettings::handle(ctx), |_, _, _, ctx| ctx.notify());

        let workflows_state = WorkflowsState {
            selected_workflow_state: None,
        };

        let env_var_collection_state = EnvVarCollectionState {
            selected_env_vars: None,
        };

        let last_word_insertion = LastWordInsertion {
            insert_command_from_history_index: 0,
            is_latest_editor_event: false,
        };

        ctx.subscribe_to_model(
            &InputSettings::handle(ctx),
            Self::handle_input_settings_event,
        );

        ctx.subscribe_to_model(&ai_controller, |me, _, event, ctx| match event {
            BlocklistAIControllerEvent::SentRequest {
                contains_user_query: is_user_initiated,
                is_queued_prompt,
                ..
            } => {
                // Skip the buffer clear for queued prompts. The user may have typed new
                // input while the agent was busy and we don't want to wipe it on auto-send.
                if *is_user_initiated && !*is_queued_prompt {
                    me.editor.update(ctx, |editor, ctx| {
                        editor.system_clear_buffer(true, ctx);
                    });
                    ctx.notify();
                }
            }
            BlocklistAIControllerEvent::ExportConversationToFile {
                #[cfg_attr(target_family = "wasm", allow(unused))]
                filename,
            } => {
                #[cfg(not(target_family = "wasm"))]
                {
                    me.export_conversation_to_file(filename.clone(), ctx);
                }
                #[cfg(target_family = "wasm")]
                {
                    log::warn!("Export to file is not supported on WASM");
                }
            }
            _ => {}
        });

        ctx.subscribe_to_model(&suggestions_mode_model, |me, _, _, ctx| {
            me.set_zero_state_hint_text(ctx);
            ctx.notify();
        });

        ctx.subscribe_to_model(&ai_input_model, |me, _, event, ctx| {
            let _ = me
                .debounce_input_background_tx
                .try_send(InputBackgroundJobOptions::default().with_command_decoration());

            let config = event.updated_config();
            if config.is_locked && me.suggestions_mode_model.as_ref(ctx).is_visible() {
                me.close_input_suggestions(/*should_focus_input=*/ false, ctx);
            }

            if config.input_type.is_ai() && !config.is_locked {
                // If command x-ray is open, hide it -- it shouldn't show in AI input mode.
                me.hide_x_ray(ctx);
            }

            me.editor.update(ctx, |editor, ctx| {
                editor.clear_autosuggestion_for_input_type(config.input_type, ctx);
            });
            me.set_zero_state_hint_text(ctx);
            ctx.notify();
        });
        // Re-render the input's placeholder/hint text when any history event that could
        // change `selected_conversation_status_for_hint` fires — the selected conversation
        // swapping, its status changing, its title being set/unset, it becoming non-empty,
        // or its being cleared. Mirrors the set used by
        // `agent_conversation_event_affects_vertical_tabs` in `workspace/view.rs` that keeps
        // vertical tab progress indicators in sync.
        ctx.subscribe_to_model(
            &BlocklistAIHistoryModel::handle(ctx),
            move |me, _, event, ctx| {
                let affects_hint = matches!(
                    event,
                    BlocklistAIHistoryEvent::UpdatedConversationStatus { .. }
                        | BlocklistAIHistoryEvent::SetActiveConversation { .. }
                        | BlocklistAIHistoryEvent::ClearedActiveConversation { .. }
                        | BlocklistAIHistoryEvent::ClearedConversationsForTerminalSurface { .. }
                        | BlocklistAIHistoryEvent::StartedNewConversation { .. }
                        | BlocklistAIHistoryEvent::SplitConversation { .. }
                        | BlocklistAIHistoryEvent::AppendedExchange { .. }
                        | BlocklistAIHistoryEvent::UpdatedStreamingExchange { .. }
                        | BlocklistAIHistoryEvent::UpdatedConversationTitle { .. }
                        | BlocklistAIHistoryEvent::UpdatedConversationMetadata { .. }
                        | BlocklistAIHistoryEvent::RestoredConversations { .. }
                );
                if !affects_hint {
                    return;
                }
                if event.terminal_surface_id() != Some(terminal_view_id) {
                    return;
                }
                me.set_zero_state_hint_text(ctx);
                ctx.notify();
            },
        );
        ctx.subscribe_to_model(&QueuedQueryModel::handle(ctx), |me, _, event, ctx| {
            let affects_hint = match event {
                QueuedQueryEvent::QueueNextPromptToggled { conversation_id } => me
                    .ai_context_model
                    .as_ref(ctx)
                    .selected_conversation_id(ctx)
                    .is_some_and(|selected_id| selected_id == *conversation_id),
                QueuedQueryEvent::DefaultModeChanged => true,
                _ => false,
            };
            if affects_hint {
                me.set_zero_state_hint_text(ctx);
                ctx.notify();
            }
        });

        // Refresh the ghost text when control of a long-running command changes hands —
        // queue mode is auto-enabled while the agent holds control, so the steer/queue
        // hint must track the control state.
        ctx.subscribe_to_model(&cli_subagent_controller, |me, _, event, ctx| {
            if matches!(
                event,
                CLISubagentEvent::SpawnedSubagent { .. }
                    | CLISubagentEvent::UpdatedControl { .. }
                    | CLISubagentEvent::FinishedSubagent { .. }
                    | CLISubagentEvent::ControlHandedBackAfterTransfer
            ) {
                me.set_zero_state_hint_text(ctx);
                ctx.notify();
            }
        });

        ctx.subscribe_to_model(&ai_context_model, |me, context_model, event, ctx| {
            match event {
                BlocklistAIContextEvent::PendingQueryStateUpdated => {
                    me.remove_excess_images(ctx);
                    me.update_image_context_options(ctx);
                    me.set_zero_state_hint_text(ctx);
                    // If buffer empty and autodetect enabled, set the underlying input type to AI.
                    // Visually to the user, empty buffer is really a separate unclassified state. But since we don't support a third state
                    // in the model right now, we set the type to AI to make sure conversation block context is rendered when a conversation is selected
                    // on empty buffer. The actual underlying type doesn't otherwise matter on an empty buffer.
                    let is_empty_buffer = me.editor().as_ref(ctx).buffer_text(ctx).is_empty();
                    if is_empty_buffer {
                        me.ai_input_model.update(ctx, |ai_input_model, ctx| {
                            let is_auto_detection_enabled = !ai_input_model.is_input_type_locked();
                            if is_auto_detection_enabled {
                                ai_input_model.set_input_type(
                                    InputType::AI,
                                    Some(InputTypeAutoDetectionSource::ConversationContextRender),
                                    ctx,
                                );
                            }
                        });
                    }
                    // The editor view renders the follow up icon, so we need to re-render the editor view.
                    me.editor().update(ctx, |_, ctx| {
                        ctx.notify();
                    })
                }
                BlocklistAIContextEvent::UpdatedPendingContext { .. } => {
                    me.update_image_context_options(ctx);
                    me.attachment_chips = context_model
                        .as_ref(ctx)
                        .pending_attachments()
                        .iter()
                        .enumerate()
                        .map(|(i, attachment)| AttachmentChip {
                            file_name: attachment.file_name().to_string(),
                            mouse_state_handle: Default::default(),
                            attachment_type: attachment.attachment_type(),
                            index: i,
                        })
                        .collect_vec();
                }
            }
            ctx.notify();
        });

        ctx.subscribe_to_model(&LLMPreferences::handle(ctx), |me, _, event, ctx| {
            if let LLMPreferencesEvent::UpdatedActiveAgentModeLLM = event {
                // If the new model doesn't support vision and we had image chips,
                // the context model already cleared them — show a toast.
                let has_image_chips = me
                    .attachment_chips
                    .iter()
                    .any(|c| matches!(c.attachment_type, AttachmentType::Image));
                let scope = ResolvedTeamScope::from_scope(
                    &UserWorkspaces::as_ref(ctx).team_context_for_view(ctx),
                );
                let vision_supported = LLMPreferences::as_ref(ctx)
                    .vision_supported(&scope, ctx, Some(me.terminal_view_id));
                if has_image_chips && !vision_supported {
                    let window_id = ctx.window_id();
                    ToastStack::handle(ctx).update(ctx, |ts, ctx| {
                        ts.add_ephemeral_toast(
                            DismissibleToast::error(
                                "Attached images were removed — the selected model does not support images.".to_string(),
                            ),
                            window_id,
                            ctx,
                        );
                    });
                }
                me.update_image_context_options(ctx);
                ctx.notify();
            }
        });

        ctx.subscribe_to_model(&AISettings::handle(ctx), |me, _, event, ctx| {
            me.handle_ai_settings_changed_event(event, ctx)
        });

        ctx.subscribe_to_model(
            &IgnoredSuggestionsModel::handle(ctx),
            |me, _, event, ctx| {
                me.handle_ignored_suggestions_event(event, ctx);
            },
        );

        let ai_req_usage_model = AIRequestUsageModel::handle(ctx);
        ctx.subscribe_to_model(&ai_req_usage_model, |_, _, _, ctx| {
            ctx.notify();
        });
        ctx.observe(&ai_req_usage_model, |_, _, ctx| {
            ctx.notify();
        });

        let agent_status_view = ctx.add_typed_action_view(|ctx| {
            BlocklistAIStatusBar::new(
                ai_controller.clone(),
                agent_view_controller.clone(),
                cli_subagent_controller.clone(),
                ai_action_model.clone(),
                ai_context_model.clone(),
                ai_input_model.clone(),
                buffer_model,
                &model_events,
                model.clone(),
                agent_shortcut_view_model.clone(),
                // Wired post-construction via `attach_ambient_agent_view_model`.
                None,
                ephemeral_message_model.clone(),
                handoff_compose_state.clone(),
                terminal_view_id,
                ctx,
            )
        });

        let queued_prompts_panel = FeatureFlag::QueueSlashCommand.is_enabled().then(|| {
            let cli_subagent_controller = cli_subagent_controller.clone();
            let host_editor = editor.clone();
            let panel = ctx.add_typed_action_view(|ctx| {
                QueuedPromptsPanelView::new(
                    terminal_view_id,
                    cli_subagent_controller,
                    host_editor,
                    ctx,
                )
            });
            ctx.subscribe_to_view(&panel, |me, _, event, ctx| {
                me.handle_queued_prompts_panel_event(event, ctx);
            });
            // Seed the host-pushed send permission; later changes flow through the
            // shared-session role-change push in `TerminalView::on_self_role_updated`. Input
            // emptiness is not pushed: the panel reads the host editor live.
            let can_send_prompt = !model.lock().shared_session_status().is_reader();
            panel.update(ctx, |panel, ctx| {
                panel.set_can_send_prompt(can_send_prompt, ctx);
            });
            panel
        });

        let deferred_remote_operations =
            DeferredRemoteOperations::new(model.lock().block_list().active_block_id().clone());

        // Use persisted menu sizes from settings, or fall back to defaults
        let input_settings = InputSettings::as_ref(ctx);
        let completions_menu_width = *input_settings.completions_menu_width.value();
        let completions_menu_height = *input_settings.completions_menu_height.value();

        let is_editor_empty = editor.as_ref(ctx).is_empty(ctx);
        let mut input = Self {
            input_suggestions,
            suggestions_mode_model,
            completions_menu_resizable_width: resizable_state_handle(completions_menu_width),
            completions_menu_resizable_height: resizable_state_handle(completions_menu_height),
            tips_completed,
            editor,
            model,
            sessions,
            focus_handle: None,
            active_block_metadata: None,
            view_id,
            input_render_state_model_handle,
            workflows_state,
            env_var_collection_state,
            command_x_ray_description: None,
            last_parsed_tokens: None,
            debounce_input_background_tx,
            has_pending_command: false,
            last_word_insertion,
            decorations_future_handle: None,
            autosuggestions_abort_handle: None,
            completions_abort_handle: None,
            menu_positioning_provider,
            universal_developer_input_button_bar,
            prompt_render_helper,
            prompt_type: current_prompt,
            ai_controller,
            ai_context_model,
            ai_input_model,
            ai_action_model,
            ai_follow_up_icon_mouse_state: MouseStateHandle::default(),
            enable_autosuggestions_setting: *editor_settings_handle
                .as_ref(ctx)
                .enable_autosuggestions,
            latest_buffer_operations: Vec::new(),
            deferred_remote_operations,
            shared_session_input_state: None,
            shared_session_presence_manager: None,
            last_user_block_completed: None,
            hoverable_handle: Default::default(),
            terminal_view_id,
            #[cfg(feature = "local_fs")]
            conn: None,
            attachment_chips: Default::default(),
            is_processing_attached_images: false,
            handoff_compose_state,
            cached_agent_mode_hint_text: None,
            is_editor_empty_on_last_edit: is_editor_empty,
            weak_view_handle: ctx.handle(),
            agent_status_view,
            queued_prompts_panel,
            agent_view_controller,
            agent_input_footer,
            agent_shortcut_view_model,
            ambient_agent_view_state,
            input_contents_before_prompt_chip_command: None,
            pending_shell_widget_handoff: None,
        };

        #[cfg(feature = "local_fs")]
        if let Some(db_url) = database_file_path_for_current_scope().to_str()
            && let Ok(conn) = establish_ro_connection(db_url)
        {
            input.conn = Some(Arc::new(Mutex::new(conn)));
        }

        if input.model.lock().shared_session_status().is_viewer() {
            input.editor.update(ctx, |editor, ctx| {
                editor.set_interaction_state(InteractionState::Selectable, ctx);
            });
        } else {
            input.set_zero_state_hint_text(ctx);
        }

        input.update_image_context_options(ctx);
        input.update_editor_input_mode(ctx);
        // Ambient wiring goes through the single setter path (`attach_ambient_agent_view_model`)
        // so construction and the lazy shared-session viewer attach share one implementation.
        if let Some(ambient_agent_view_model) = ambient_agent_view_model {
            input.attach_ambient_agent_view_model(ambient_agent_view_model, ctx);
        }
        input
    }

    fn update_editor_input_mode(&mut self, ctx: &mut ViewContext<Self>) {
        let ai_input_model = self.ai_input_model.as_ref(ctx);
        let is_ai_input = ai_input_model.input_type().is_ai();
        self.editor.update(ctx, move |editor, ctx| {
            editor.set_is_ai_input(is_ai_input, ctx);
            ctx.notify();
        });
    }

    pub fn agent_status_bar(&self) -> &ViewHandle<BlocklistAIStatusBar> {
        &self.agent_status_view
    }

    fn handle_queued_prompts_panel_event(
        &mut self,
        event: &QueuedPromptsPanelEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            QueuedPromptsPanelEvent::SendNow {
                conversation_id,
                query_id,
                text,
                is_command,
            } => {
                self.send_queued_row_immediately(
                    *conversation_id,
                    *query_id,
                    text.clone(),
                    *is_command,
                    QueuedPromptSendNowTrigger::SendNowButton,
                    ctx,
                );
            }
            QueuedPromptsPanelEvent::RowDeleted => {
                self.focus_input_box(ctx);
            }
            QueuedPromptsPanelEvent::EditEnded => {
                self.focus_input_box(ctx);
            }
        }
    }

    /// Dispatches a queued row immediately: commands execute in the terminal, prompts submit to
    /// the conversation's current target. On dispatch, removes the fired row and refocuses the
    /// input. Shared by the row's send-now button and empty-buffer Enter.
    fn send_queued_row_immediately(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        text: String,
        is_command: bool,
        trigger: QueuedPromptSendNowTrigger,
        ctx: &mut ViewContext<Self>,
    ) {
        // Read the origin before dispatch; the row is removed once it fires.
        let origin = QueuedQueryModel::as_ref(ctx)
            .queue(conversation_id)
            .iter()
            .find(|row| row.id() == query_id)
            .map(|row| row.origin());
        let dispatched = if is_command {
            self.execute_queued_command(&text, conversation_id, ctx)
        } else {
            self.submit_queued_prompt_for_active_pane(text, conversation_id, query_id, ctx);
            true
        };
        if !dispatched {
            return;
        }
        if let Some(origin) = origin {
            send_telemetry_from_ctx!(
                TelemetryEvent::QueuedPromptSentNow {
                    origin: origin.into(),
                    trigger,
                },
                ctx
            );
        }
        QueuedQueryModel::handle(ctx).update(ctx, |model, ctx| {
            model.remove_fired_row(conversation_id, query_id, ctx);
        });
        self.focus_input_box(ctx);
    }

    /// The queued prompts panel, when [`FeatureFlag::QueueSlashCommand`] is enabled.
    pub(crate) fn queued_prompts_panel(&self) -> Option<&ViewHandle<QueuedPromptsPanelView>> {
        self.queued_prompts_panel.as_ref()
    }

    /// Returns whether this input's queued-prompt inline editor is currently focused.
    pub(crate) fn is_queued_prompt_inline_editor_focused(&self, ctx: &AppContext) -> bool {
        self.queued_prompts_panel
            .as_ref()
            .is_some_and(|panel| panel.as_ref(ctx).is_inline_edit_editor_focused(ctx))
    }

    /// Returns whether the active queued prompt is being edited inline.
    fn is_editing_queued_prompt(&self, ctx: &AppContext) -> bool {
        let Some(conversation_id) =
            BlocklistAIHistoryModel::as_ref(ctx).active_conversation_id(self.terminal_view_id)
        else {
            return false;
        };
        QueuedQueryModel::as_ref(ctx)
            .editing_row(conversation_id)
            .is_some()
    }

    pub fn agent_input_footer(&self) -> &ViewHandle<AgentInputFooter> {
        &self.agent_input_footer
    }

    fn ambient_agent_view_model(&self) -> Option<&ModelHandle<AmbientAgentViewModel>> {
        self.ambient_agent_view_state
            .as_ref()
            .map(AmbientAgentViewState::view_model)
    }

    /// The ambient agent run this pane belongs to, if any.
    fn ambient_agent_task_id(&self, ctx: &AppContext) -> Option<AmbientAgentTaskId> {
        resolve_ambient_agent_task_id(self.ambient_agent_view_model(), &self.model.lock(), ctx)
    }

    /// Blocks a submission for `task_id` while that task is not in [`AgentConversationsModel`]
    /// yet, starting (or deduping) its fetch and telling the user to retry (REMOTE-2661).
    /// `resolve_ai_query_routing` can't distinguish an absent task from an ineligible one, and
    /// treating unknown as ineligible would wrongly fall back to a local conversation. Returns
    /// `true` when the caller must stop.
    fn block_submission_while_ambient_task_unresolved(
        &self,
        task_id: Option<AmbientAgentTaskId>,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(task_id) = task_id.filter(|task_id| {
            AgentConversationsModel::as_ref(ctx)
                .get_task_data(task_id)
                .is_none()
        }) else {
            return false;
        };
        AgentConversationsModel::handle(ctx).update(ctx, |model, ctx| {
            model.get_or_async_fetch_task_data(&task_id, ctx);
        });
        self.show_ephemeral_error_toast(
            "Still checking this session's status — please try sending your message again in a moment.",
            ctx,
        );
        true
    }

    /// Shows a transient error toast for a follow-up submission that was blocked or redirected.
    fn show_ephemeral_error_toast(&self, message: &str, ctx: &mut ViewContext<Self>) {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(
                DismissibleToast::error(message.to_string()),
                window_id,
                ctx,
            );
        });
    }

    /// Routes an AI query submission to the correct non-local target, using the same
    /// [`resolve_ai_query_routing`] source of truth as the footer live-VM indicator, so a
    /// cloud/remote conversation never continues on the local agent. Shared by
    /// [`Self::submit_ai_query_with_routing`] (the Enter submit path) and
    /// `input_cmd_enter`.
    ///
    /// Returns `true` when the submission was handled here (forwarded to the live VM, started a
    /// cloud follow-up, or blocked with a toast) and the caller should stop; `false` when the
    /// caller should handle the local case (submit locally for Enter). Also returns `false` for an executor viewer
    /// running a local-action slash command such as `/fork`.
    fn maybe_route_ai_query_to_remote_target(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        // Nothing to route for an empty buffer; let the caller's normal (no-op) handling run.
        if self.editor.as_ref(ctx).buffer_text(ctx).trim().is_empty() {
            return false;
        }

        // Scoped to an attached ambient live viewer, the case where unresolved eligibility
        // would otherwise fall through to the ordinary `LiveRemoteVm` path.
        let is_attached_ambient_viewer = {
            let model = self.model.lock();
            model.shared_session_status().is_active_viewer()
                && (model.is_shared_ambient_agent_session()
                    || self
                        .ambient_agent_view_model()
                        .is_some_and(|m| m.as_ref(ctx).is_ambient_agent()))
        };
        if self.block_submission_while_ambient_task_unresolved(
            is_attached_ambient_viewer
                .then(|| self.ambient_agent_task_id(ctx))
                .flatten(),
            ctx,
        ) {
            return true;
        }

        // Route by the shared source of truth. A live shared-session viewer forwards to the sharer
        // (an ambient cloud run or a shared local session); the other arms cover panes that are not
        // attached to a live session.
        let ai_query_routing = {
            let model = self.model.lock();
            resolve_ai_query_routing(
                self.terminal_view_id,
                self.ambient_agent_view_model(),
                &model,
                ctx,
            )
        };
        match ai_query_routing {
            AIQueryRouting::Local => false,
            AIQueryRouting::LiveRemoteVm {
                is_executor: true, ..
            } => {
                // Returns false for local-action slash commands (e.g. /fork), which should still
                // run on the viewer's own machine; the caller then proceeds to local submission.
                self.submit_viewer_ai_query(ctx)
            }
            AIQueryRouting::LiveRemoteVm {
                is_executor: false, ..
            } => {
                if self.model.lock().shared_session_status().is_active_viewer() {
                    // Connected to the live session but without an executor role.
                    log::warn!("Viewer tried to submit AI query without executor role");
                    self.show_ephemeral_error_toast(
                        "Cannot send queries as a read-only viewer.",
                        ctx,
                    );
                } else {
                    // The Oz run has a live execution this pane never attached to (a new execution
                    // was started for the run while this pane was open from earlier), so there is
                    // no live shared session to forward the prompt to.
                    // TODO: instead of blocking, connect to the live shared session
                    // and submit the prompt to the running remote VM. Or, auto close and reopen the link.
                    self.show_ephemeral_error_toast(
                        "This pane is out of date. Reopen the Oz session link in a new pane and try submitting again.",
                        ctx,
                    );
                }
                true
            }
            AIQueryRouting::NewCloudVm { task_id } => {
                if FeatureFlag::HandoffCloudCloud.is_enabled() {
                    let prompt = self.editor.as_ref(ctx).buffer_text(ctx).trim().to_owned();
                    let pending_attachments = self
                        .ai_context_model
                        .as_ref(ctx)
                        .pending_attachments()
                        .to_vec();
                    if Self::should_upload_cloud_followup_attachments(&pending_attachments) {
                        self.freeze_input_in_loading_state(ctx);
                        self.upload_files_then_submit_cloud_followup(
                            task_id,
                            prompt,
                            pending_attachments,
                            ctx,
                        );
                    } else {
                        if !pending_attachments.is_empty() {
                            log::warn!(
                                "Cannot upload cloud follow-up attachments: CloudModeImageContext is disabled"
                            );
                        }
                        ctx.emit(Event::SubmitCloudFollowup { prompt });
                    }
                } else {
                    // Cloud-to-cloud follow-up is unavailable; block rather than run locally.
                    self.show_ephemeral_error_toast(
                        "This cloud conversation can't continue on your local machine.",
                        ctx,
                    );
                }
                true
            }
            AIQueryRouting::UnconnectedReadOnly => {
                self.show_ephemeral_error_toast(
                    "This cloud conversation can't continue on your local machine.",
                    ctx,
                );
                true
            }
            AIQueryRouting::RetainedSetupFailureDebug { task_id } => {
                // Every authenticated origin converges on the same follow-up service call,
                // never the direct viewer prompt path or the local agent (REMOTE-2661).
                let prompt = self.editor.as_ref(ctx).buffer_text(ctx).trim().to_owned();
                ctx.emit(Event::SubmitSetupFailureDebugFollowup { task_id, prompt });
                true
            }
        }
    }

    fn should_upload_cloud_followup_attachments(pending_attachments: &[PendingAttachment]) -> bool {
        !pending_attachments.is_empty() && FeatureFlag::CloudModeImageContext.is_enabled()
    }

    /// Primary entry point for submitting the input buffer as an AI query. Routes to the correct
    /// target via [`Self::maybe_route_ai_query_to_remote_target`] (live viewer, new cloud VM, stale or
    /// read-only), falling back to [`Self::submit_ai_query_local`] for ordinary local panes and
    /// for an executor viewer running a local-action slash command (e.g. `/fork`).
    fn submit_ai_query_with_routing(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.maybe_route_ai_query_to_remote_target(ctx) {
            self.submit_ai_query_local(ctx);
        }
    }

    fn harness_selector(&self) -> Option<&ViewHandle<HarnessSelector>> {
        self.ambient_agent_view_state
            .as_ref()
            .map(|state| &state.harness_selector)
    }

    fn host_selector(&self) -> Option<&ViewHandle<HostSelector>> {
        self.ambient_agent_view_state
            .as_ref()
            .and_then(|state| state.host_selector.as_ref())
    }

    fn auth_secret_selector(&self) -> Option<&ViewHandle<AuthSecretSelector>> {
        self.ambient_agent_view_state
            .as_ref()
            .and_then(|state| state.auth_secret_selector.as_ref())
    }

    pub(super) fn auth_secret_delete_confirmation_dialog_element(
        &self,
        ctx: &AppContext,
    ) -> Option<Box<dyn Element>> {
        self.auth_secret_selector()
            .map(|selector| selector.as_ref(ctx).delete_confirmation_dialog_element())
    }

    pub(super) fn auth_secret_ftux_view(&self) -> Option<&ViewHandle<AuthSecretFtuxView>> {
        self.ambient_agent_view_state
            .as_ref()
            .and_then(|state| state.auth_secret_ftux_view.as_ref())
    }

    /// Restores the `&` handoff compose draft after a workspace failure.
    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
    pub(crate) fn restore_cloud_handoff_draft(
        &mut self,
        launch: PendingCloudLaunch,
        environment_id: Option<SyncId>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.activate_cloud_handoff_compose(HandoffEntryPoint::Ampersand, ctx);
        self.editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&launch.prompt, ctx);
        });
        self.ai_context_model.update(ctx, |model, ctx| {
            for attachment in launch.attachments.display_attachments {
                model.append_pending_attachments(vec![attachment], ctx);
            }
        });
        if let Some(env_id) = environment_id {
            self.handoff_compose_state.update(ctx, |state, ctx| {
                state.set_environment_id(Some(env_id), true, ctx);
            });
        }
    }

    fn prefix_mode(&self, ctx: &AppContext) -> InputPrefixMode {
        let is_handoff_active = self.handoff_compose_state.as_ref(ctx).is_active();
        let ai_input_model = self.ai_input_model.as_ref(ctx);
        let is_shell_active =
            ai_input_model.is_input_type_locked() && !ai_input_model.input_type().is_ai();

        if is_handoff_active {
            InputPrefixMode::CloudHandoff
        } else if is_shell_active {
            InputPrefixMode::Shell
        } else {
            InputPrefixMode::None
        }
    }

    /// Switches the input into cloud handoff compose mode, locking it to AI input
    /// and activating the handoff compose state.
    fn activate_cloud_handoff_compose(
        &mut self,
        entry_point: HandoffEntryPoint,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.prefix_mode(ctx) == InputPrefixMode::CloudHandoff {
            return;
        }

        let is_input_buffer_empty = self.editor.as_ref(ctx).is_empty(ctx);
        self.ai_input_model.update(ctx, |ai_input_model, ctx| {
            ai_input_model.set_input_config(
                InputConfig {
                    input_type: InputType::AI,
                    is_locked: true,
                },
                is_input_buffer_empty,
                Some(InputTypeAutoDetectionSource::CloudHandoffEnter),
                ctx,
            );
        });

        self.handoff_compose_state
            .update(ctx, |state, ctx| state.activate(entry_point, ctx));
        self.is_editor_empty_on_last_edit = is_input_buffer_empty;

        #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
        self.auto_select_environment_from_pwd(ctx);

        ctx.notify();
    }

    /// Spawns an async task to resolve the pwd's git repo and pick the best
    /// environment overlap, updating the handoff compose state when done.
    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
    fn auto_select_environment_from_pwd(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(pwd) = self
            .active_session_path_if_local(ctx)
            .map(Path::to_path_buf)
        else {
            return;
        };

        let handoff_compose_state = self.handoff_compose_state.clone();
        let suggestion = suggest_handoff_environment(pwd, ctx);
        ctx.spawn(
            async move {
                suggestion
                    .with_timeout(Duration::from_secs(5))
                    .await
                    .ok()
                    .flatten()
            },
            move |_input, environment_id, ctx| {
                if let Some(environment_id) = environment_id {
                    handoff_compose_state.update(ctx, |state, ctx| {
                        state.set_environment_id(Some(environment_id), false, ctx);
                    });
                }
            },
        );
    }

    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub(crate) fn handoff_entry_point(&self, ctx: &AppContext) -> HandoffEntryPoint {
        self.handoff_compose_state.as_ref(ctx).entry_point()
    }

    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
    pub(crate) fn exit_cloud_handoff_compose_and_clear_prompt(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) {
        self.exit_cloud_handoff_compose(ctx);
        self.editor.update(ctx, |editor, ctx| {
            editor.clear_buffer(ctx);
        });
    }

    fn exit_cloud_handoff_compose(&mut self, ctx: &mut ViewContext<Self>) {
        if self.prefix_mode(ctx) != InputPrefixMode::CloudHandoff {
            return;
        }

        let is_input_buffer_empty = self.editor.as_ref(ctx).is_empty(ctx);
        self.handoff_compose_state
            .update(ctx, |state, ctx| state.exit(ctx));
        self.ai_input_model.update(ctx, |ai_input_model, ctx| {
            ai_input_model.set_input_config(
                InputConfig {
                    input_type: InputType::AI,
                    is_locked: true,
                }
                .unlocked_if_autodetection_enabled(true, ctx),
                is_input_buffer_empty,
                Some(InputTypeAutoDetectionSource::CloudHandoffExit),
                ctx,
            );
        });
    }

    // Cloud handoff methods — candidates for extraction to a separate file
    // following the pattern used by `agent.rs`, `classic.rs`, etc.
    fn can_activate_cloud_handoff_prefix(
        &self,
        edit_origin: &EditOrigin,
        ctx: &AppContext,
    ) -> bool {
        let is_powershell_with_nld_enabled = self.editor.as_ref(ctx).shell_family()
            == Some(ShellFamily::PowerShell)
            && AISettings::as_ref(ctx).is_ai_autodetection_enabled(ctx);
        let is_cloud = {
            let terminal_model = self.model.lock();
            is_in_cloud_context(&terminal_model)
        };
        *edit_origin == EditOrigin::UserTyped
            && AISettings::as_ref(ctx).is_ampersand_handoff_enabled(ctx)
            && !is_powershell_with_nld_enabled
            && FeatureFlag::AgentView.is_enabled()
            && self.agent_view_controller.as_ref(ctx).is_fullscreen()
            && !is_cloud
            && self.prefix_mode(ctx) == InputPrefixMode::None
    }

    fn maybe_activate_cloud_handoff_prefix(
        &mut self,
        edit_origin: &EditOrigin,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let is_new_handoff_prefix = {
            let editor = self.editor.as_ref(ctx);
            editor
                .buffer_text(ctx)
                .starts_with(CLOUD_HANDOFF_INPUT_PREFIX)
                && !editor
                    .last_buffer_text(ctx)
                    .starts_with(CLOUD_HANDOFF_INPUT_PREFIX)
        };
        if !self.can_activate_cloud_handoff_prefix(edit_origin, ctx) || !is_new_handoff_prefix {
            return false;
        }

        let is_input_buffer_empty = self.editor.update(ctx, |editor, ctx| {
            if let Some(rest) = editor
                .buffer_text(ctx)
                .strip_prefix(CLOUD_HANDOFF_INPUT_PREFIX)
            {
                editor.set_buffer_text(rest, ctx);
            }
            editor.buffer_text(ctx).is_empty()
        });
        self.ai_input_model.update(ctx, |ai_input_model, ctx| {
            ai_input_model.set_input_config(
                InputConfig {
                    input_type: InputType::AI,
                    is_locked: true,
                },
                is_input_buffer_empty,
                Some(InputTypeAutoDetectionSource::CloudHandoffEnter),
                ctx,
            );
        });

        self.handoff_compose_state.update(ctx, |state, ctx| {
            state.activate(HandoffEntryPoint::Ampersand, ctx)
        });
        self.is_editor_empty_on_last_edit = is_input_buffer_empty;

        #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
        self.auto_select_environment_from_pwd(ctx);

        ctx.notify();
        true
    }

    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
    pub(crate) fn collect_cloud_launch_attachments(
        &self,
        ctx: &mut ViewContext<Self>,
    ) -> HandoffLaunchAttachments {
        if !FeatureFlag::CloudModeImageContext.is_enabled() {
            return HandoffLaunchAttachments::default();
        }

        let mut request_attachments: Vec<AttachmentInput> = self
            .ai_context_model
            .as_ref(ctx)
            .pending_images()
            .iter()
            .map(|image| AttachmentInput {
                file_name: image.file_name.clone(),
                mime_type: image.mime_type.clone(),
                data: image.data.clone(),
            })
            .collect();

        let mut skipped_files: Vec<String> = Vec::new();
        for file in self.ai_context_model.as_ref(ctx).pending_files() {
            match std::fs::read(&file.file_path) {
                Ok(bytes) => {
                    if bytes.len() > MAX_ATTACHMENT_SIZE_BYTES {
                        skipped_files.push(file.file_name.clone());
                        continue;
                    }
                    request_attachments.push(AttachmentInput {
                        file_name: file.file_name.clone(),
                        mime_type: file.mime_type.clone(),
                        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    });
                }
                Err(e) => {
                    log::warn!("Failed to read file {}: {e}", file.file_path.display());
                }
            }
        }

        if !skipped_files.is_empty() {
            let window_id = ctx.window_id();
            let message = if skipped_files.len() == 1 {
                format!(
                    "{} was not attached — exceeds 10MB limit.",
                    skipped_files[0]
                )
            } else {
                format!(
                    "{} files were not attached — exceed 10MB limit.",
                    skipped_files.len()
                )
            };
            ToastStack::handle(ctx).update(ctx, |ts, ctx| {
                ts.add_ephemeral_toast(DismissibleToast::error(message), window_id, ctx);
            });
        }

        let display_attachments: Vec<PendingAttachment> = self
            .ai_context_model
            .as_ref(ctx)
            .pending_attachments()
            .to_vec();

        HandoffLaunchAttachments {
            request_attachments,
            display_attachments,
        }
    }

    /// Source-content guardrail shared by the three local-to-cloud handoff entry
    /// points (footer chip, `&` compose, `/handoff`): true when this terminal's
    /// active source conversation has at least one exchange to hand off.
    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
    fn source_conversation_has_content(&self, ctx: &AppContext) -> bool {
        BlocklistAIHistoryModel::as_ref(ctx)
            .active_conversation(self.terminal_view_id)
            .is_some_and(|c| !c.is_empty())
    }

    /// Cloud handoff is Oz-only. When this pane's active Agent Mode model can't
    /// run in a Warp cloud (Oz) agent (e.g. a custom-endpoint/BYOK model or
    /// local custom router), shows an explanatory error toast and returns true
    /// so the `&`, footer-chip, and `/handoff` entry points can bail out up
    /// front instead of failing at spawn time.
    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
    fn block_cloud_handoff_if_model_unsupported(&self, ctx: &mut ViewContext<Self>) -> bool {
        let scope =
            ResolvedTeamScope::from_scope(&UserWorkspaces::as_ref(ctx).team_context_for_view(ctx));
        if LLMPreferences::as_ref(ctx).is_active_base_model_cloud_runnable(
            &scope,
            self.terminal_view_id,
            ctx,
        ) {
            return false;
        }
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |ts, ctx| {
            ts.add_ephemeral_toast(
                DismissibleToast::error(
                    "Custom models can't run in the cloud. Switch to a Warp model to hand off."
                        .to_owned(),
                ),
                window_id,
                ctx,
            );
        });
        true
    }

    #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
    fn maybe_launch_cloud_handoff_request(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        use crate::cloud_object::CloudObjectLookup as _;

        if !FeatureFlag::OzHandoff.is_enabled()
            || !FeatureFlag::HandoffLocalCloud.is_enabled()
            || !cfg!(all(feature = "local_fs", not(target_family = "wasm")))
            || self.prefix_mode(ctx) != InputPrefixMode::CloudHandoff
        {
            return false;
        }

        if self.block_cloud_handoff_if_model_unsupported(ctx) {
            // Keep compose state, the typed prompt, and attachments so the user
            // can switch models and resubmit.
            return true;
        }

        let prompt = self.editor.as_ref(ctx).buffer_text(ctx).trim().to_owned();
        // Empty buffer + source conversation with content launches an immediate empty-prompt handoff.
        if prompt.is_empty() {
            if !self.source_conversation_has_content(ctx) {
                return true;
            }

            if CloudAmbientAgentEnvironment::get_all(ctx).is_empty() {
                ctx.emit(Event::OpenHandoffEnvironmentCreationModal);
                return true;
            }

            let environment_id = self
                .handoff_compose_state
                .as_ref(ctx)
                .selected_environment_id()
                .cloned();
            let entry_point = self.handoff_compose_state.as_ref(ctx).entry_point();
            self.exit_cloud_handoff_compose_and_clear_prompt(ctx);
            ctx.dispatch_typed_action_deferred(WorkspaceAction::OpenLocalToCloudHandoffPane {
                launch: None,
                environment_id,
                entry_point,
            });
            return true;
        }

        if CloudAmbientAgentEnvironment::get_all(ctx).is_empty() {
            ctx.emit(Event::OpenHandoffEnvironmentCreationModal);
            return true;
        }

        let attachments = self.collect_cloud_launch_attachments(ctx);
        let environment_id = self
            .handoff_compose_state
            .as_ref(ctx)
            .selected_environment_id()
            .cloned();
        let entry_point = self.handoff_compose_state.as_ref(ctx).entry_point();
        let launch = PendingCloudLaunch {
            prompt,
            attachments,
        };

        self.exit_cloud_handoff_compose_and_clear_prompt(ctx);

        ctx.dispatch_typed_action_deferred(WorkspaceAction::OpenLocalToCloudHandoffPane {
            launch: Some(launch),
            environment_id,
            entry_point,
        });
        true
    }

    #[cfg(not(all(feature = "local_fs", not(target_family = "wasm"))))]
    fn maybe_launch_cloud_handoff_request(&mut self, _ctx: &mut ViewContext<Self>) -> bool {
        false
    }

    #[cfg(not(target_family = "wasm"))]
    fn export_conversation_to_file(
        &mut self,
        filename_arg: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let history = BlocklistAIHistoryModel::handle(ctx);
        let Some(conversation) = history
            .as_ref(ctx)
            .active_conversation(self.terminal_view_id)
        else {
            let window_id = ctx.window_id();
            ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                let toast =
                    DismissibleToast::default(String::from("No active conversation to export"));
                toast_stack.add_ephemeral_toast(toast, window_id, ctx);
            });
            return;
        };
        let current_directory = self
            .active_block_metadata
            .as_ref()
            .and_then(|metadata| metadata.current_working_directory())
            .map(str::to_owned);
        let conversation_title = conversation.title();

        let action_model = self.ai_action_model.as_ref(ctx);
        let conversation_text = conversation.export_to_markdown(Some(action_model));
        match export_conversation_markdown(
            current_directory.as_deref(),
            filename_arg.as_deref(),
            conversation_title.as_deref(),
            &conversation_text,
        ) {
            Ok(export) => {
                let window_id = ctx.window_id();
                let display_path = export.path().display().to_string();
                ToastStack::handle(ctx).update(ctx, move |toast_stack, ctx| {
                    if export.overwrote_existing() {
                        let toast = DismissibleToast::default(format!(
                            "File {display_path} already exists and will be overwritten"
                        ));
                        toast_stack.add_ephemeral_toast(toast, window_id, ctx);
                    }
                    let toast = DismissibleToast::default(format!(
                        "Conversation exported to {display_path}"
                    ));
                    toast_stack.add_ephemeral_toast(toast, window_id, ctx);
                });
            }
            Err(error) => {
                let user_message = error.user_message();
                let path = error.path().to_path_buf();

                log::error!(
                    "Failed to write conversation to file {}: {error}",
                    path.display()
                );
                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, move |toast_stack, ctx| {
                    let toast = DismissibleToast::default(user_message);
                    toast_stack.add_ephemeral_toast(toast, window_id, ctx);
                });
            }
        }
        self.editor.update(ctx, |editor, ctx| {
            editor.clear_buffer(ctx);
        });
    }
    /// When the active conversation is changed, the number of attached images may exceed the
    /// limit of images for a conversation
    pub fn remove_excess_images(&mut self, ctx: &mut ViewContext<Self>) {
        let num_images_attached = self.ai_context_model.as_ref(ctx).pending_images().len();

        let Some(conversation) = self.ai_context_model.as_ref(ctx).selected_conversation(ctx)
        else {
            return;
        };

        let num_images_in_conversation = conversation
            .get_root_task()
            .into_iter()
            .flat_map(|task| {
                task.all_contexts()
                    .filter(|context| matches!(context, AIAgentContext::Image(_)))
            })
            .count();

        let excess_images = (num_images_in_conversation + num_images_attached)
            .saturating_sub(MAX_IMAGES_PER_CONVERSATION);

        let images_removed = self.ai_context_model.update(ctx, |context_model, ctx| {
            context_model.remove_last_pending_images(excess_images, ctx)
        });

        if images_removed > 0 {
            let window_id = ctx.window_id();

            let message = if images_removed == 1 {
                "1 image was removed - limit is 20 per conversation.".into()
            } else {
                format!("{images_removed} images were removed - limit is 20 per conversation.")
            };

            ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                toast_stack.add_persistent_toast(DismissibleToast::error(message), window_id, ctx);
            });
        }
    }

    pub fn update_image_context_options(&mut self, ctx: &mut ViewContext<Self>) {
        let ai_input_model = self.ai_input_model.as_ref(ctx);

        let llm_prefs = LLMPreferences::as_ref(ctx);

        let scope =
            ResolvedTeamScope::from_scope(&UserWorkspaces::as_ref(ctx).team_context_for_view(ctx));
        let vision_supported = llm_prefs.vision_supported(&scope, ctx, Some(self.terminal_view_id));

        let num_images_attached = self.ai_context_model.as_ref(ctx).pending_images().len();

        let conversation = self.ai_context_model.as_ref(ctx).selected_conversation(ctx);

        let num_images_in_conversation = conversation
            .and_then(|conversation| conversation.get_root_task())
            .into_iter()
            .flat_map(|task| {
                task.all_contexts()
                    .filter(|context| matches!(context, AIAgentContext::Image(_)))
            })
            .count();

        // Image context is available whenever the feature flag is enabled and we're in AI input
        // mode, including cloud mode
        let image_context_options = if FeatureFlag::ImageAsContext.is_enabled()
            && matches!(ai_input_model.input_type(), InputType::AI)
        {
            ImageContextOptions::Enabled {
                unsupported_model: !vision_supported,
                is_processing_attached_images: self.is_processing_attached_images,
                num_images_attached,
                num_images_in_conversation,
            }
        } else {
            ImageContextOptions::Disabled
        };

        self.editor.update(ctx, move |editor, ctx| {
            editor.update_image_context_options(image_context_options, ctx);
            ctx.notify();
        });
    }

    pub fn set_shared_session_presence_manager(
        &mut self,
        presence_manager: ModelHandle<PresenceManager>,
    ) {
        self.shared_session_presence_manager = Some(presence_manager);
    }

    // Auto-attach the last block for this query.
    pub fn clear_attached_context(&mut self, ctx: &mut ViewContext<Self>) {
        self.ai_context_model.update(ctx, |model, ctx| {
            model.reset_context_to_default(ctx);
        });
        ctx.emit(Event::ClearSelectionsWhenShellMode);
    }

    pub fn ai_input_model(&self) -> &ModelHandle<BlocklistAIInputModel> {
        &self.ai_input_model
    }

    fn cancel_active_conversation(
        &mut self,
        ctx: &mut ViewContext<Self>,
        cancellation_reason: CancellationReason,
    ) {
        self.ai_controller.update(ctx, |controller, ctx| {
            let active_conversation_id = BlocklistAIHistoryModel::as_ref(ctx)
                .active_conversation(self.terminal_view_id)
                .filter(|conversation| conversation.status().is_in_progress())
                .map(|conversation| conversation.id());
            if let Some(active_conversation_id) = active_conversation_id {
                controller.cancel_conversation_progress(
                    active_conversation_id,
                    cancellation_reason,
                    ctx,
                );
            }
        });
    }

    fn handle_prompt_event(&mut self, event: &PromptDisplayEvent, ctx: &mut ViewContext<Self>) {
        match event {
            PromptDisplayEvent::OpenFile(file_name) => {
                // Insert the filename into the terminal input
                self.editor.update(ctx, |editor, ctx| {
                    editor.set_buffer_text(file_name, ctx);
                });
                ctx.notify();
            }
            PromptDisplayEvent::OpenTextFileInCodeEditor(file_name) => {
                // Open text file in a new code editor pane
                let result = self.open_file_in_code_editor(file_name, ctx);
                if let Err(e) = result {
                    log::warn!("Failed to open file in code editor: {e}");
                }
            }
            PromptDisplayEvent::ToggleMenu { open } => {
                if *open {
                    // Close any open input suggestion menus (history, Ctrl+R, etc.) when chip menus
                    // are opened to prevent overlapping menus in UDI
                    self.close_overlays(false, ctx);
                    ctx.notify();
                } else {
                    self.focus_input_box(ctx);
                }
            }
            PromptDisplayEvent::OpenCodeReview => {
                ctx.emit(Event::OpenCodeReviewPane);
            }
            PromptDisplayEvent::OpenCommandPaletteFiles => {
                ctx.emit(Event::OpenFilesPalette {
                    source: PaletteSource::ContextChip,
                });
            }
            PromptDisplayEvent::RunAgentQuery(query) => {
                self.cancel_active_conversation(ctx, CancellationReason::UserCommandExecuted);
                let query = query.clone();
                self.ai_controller.update(ctx, |controller, ctx| {
                    controller.send_user_query_in_new_conversation(
                        query,
                        None,
                        EntrypointType::UserInitiated,
                        None,
                        ctx,
                    );
                });
            }
            PromptDisplayEvent::TryExecuteCommand(command) => {
                let Some(shell_type) = self
                    .active_session(ctx)
                    .map(|session| session.shell().shell_type())
                else {
                    log::warn!("Tried to execute prompt chip command without an active session");
                    return;
                };
                let command = render_prompt_chip_shell_command(command, shell_type);
                // Snapshot the current input so we can restore it after the command completes.
                let current_input = self.buffer_text(ctx);
                if self.try_execute_command_from_source(
                    &command,
                    CommandExecutionSource::User,
                    true,
                    ctx,
                ) {
                    self.cancel_active_conversation(ctx, CancellationReason::UserCommandExecuted);
                    if !current_input.is_empty() {
                        self.input_contents_before_prompt_chip_command = Some(current_input);
                    }
                }
            }
            PromptDisplayEvent::OpenAIDocument {
                document_id,
                document_version,
            } => {
                ctx.emit(Event::ToggleAIDocumentPane {
                    document_id: *document_id,
                    document_version: *document_version,
                });
            }
        }
    }

    fn open_file_in_code_editor(
        &mut self,
        _file_name: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let Some(session_id) = self.active_block_session_id() else {
            return Err("Tried to open file in code editor without a session id".to_string());
        };

        let Some(session) = self.sessions.as_ref(ctx).get(session_id) else {
            return Err("Tried to open file in code editor without a session".to_string());
        };

        if !session.is_local() {
            return Err("Tried to open file in code editor for a remote session".to_string());
        }

        #[cfg(feature = "local_fs")]
        {
            // Get the current working directory from the active terminal session
            let current_dir = self
                .active_block_metadata
                .as_ref()
                .and_then(|metadata| metadata.current_working_directory())
                .map(std::path::PathBuf::from)
                .ok_or("Failed to get current working directory".to_string())?;
            let file_path = current_dir.join(_file_name);
            // Create a CodeSource for the file
            let code_source = CodeSource::Link {
                path: file_path,
                range_start: None,
                range_end: None,
            };
            // Emit an event to create a new code pane
            ctx.emit(Event::OpenCodeInWarp {
                source: code_source,
                layout: *external_editor::EditorSettings::as_ref(ctx)
                    .open_file_layout
                    .value(),
            });
        }

        Ok(())
    }

    fn handle_theme_change(&mut self, ctx: &mut ViewContext<Self>) {
        if self.should_apply_decorations(ctx) {
            self.run_input_background_jobs(
                InputBackgroundJobOptions::default().with_command_decoration(),
                ctx,
            );
        }
    }

    pub fn sessions<'a, A: ModelAsRef>(&self, ctx: &'a A) -> &'a Sessions {
        self.sessions.as_ref(ctx)
    }

    pub fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle.clone());
        let focus_model = focus_handle.focus_state_handle().clone();
        ctx.subscribe_to_model(&focus_model, move |me, _, event, ctx| {
            if !focus_handle.is_affected(event) {
                return;
            }

            let is_focused = focus_handle.is_focused(ctx);

            me.prompt_render_helper
                .prompt_view()
                .update(ctx, |prompt_view, ctx| {
                    prompt_view.on_pane_focus_changed(is_focused, ctx);
                });

            me.set_zero_state_hint_text(ctx);

            // Update the universal developer input button bar blurred state when focus changes
            if me.should_show_universal_developer_input(ctx) {
                me.universal_developer_input_button_bar
                    .update(ctx, |button_bar, ctx| {
                        button_bar.set_is_in_active_terminal(is_focused, ctx);
                    });
            }
        });
    }

    fn is_pane_focused(&self, app: &AppContext) -> bool {
        // If the focus handle hasn't been set yet, assume we're not in a split pane and therefore focused.
        self.focus_handle.as_ref().is_none_or(|h| h.is_focused(app))
    }

    pub(super) fn team_scope<'a>(&self, app: &'a AppContext) -> TeamContext<'a> {
        UserWorkspaces::as_ref(app).team_context(&self.weak_view_handle, app)
    }

    fn is_active_session(&self, app: &AppContext) -> bool {
        self.focus_handle
            .as_ref()
            .is_some_and(|h| h.is_active_session(app))
    }

    pub fn menu_positioning(&self, app: &AppContext) -> MenuPositioning {
        self.menu_positioning_provider.menu_position(app)
    }

    fn size_info(&self, ctx: &AppContext) -> SizeInfo {
        ctx.model(&self.input_render_state_model_handle).size_info()
    }

    pub fn set_size_info(&mut self, size_info: SizeInfo, ctx: &mut AppContext) {
        self.input_render_state_model_handle
            .update(ctx, |input_render_state_model, _| {
                input_render_state_model.set_size_info(size_info);
            });
    }

    pub fn editor(&self) -> &ViewHandle<EditorView> {
        &self.editor
    }

    pub(crate) fn ai_context_model(&self) -> &ModelHandle<BlocklistAIContextModel> {
        &self.ai_context_model
    }

    pub fn buffer_text(&self, ctx: &AppContext) -> String {
        self.editor.as_ref(ctx).buffer_text(ctx)
    }

    pub fn buffer_text_number_of_lines(&self, ctx: &AppContext) -> usize {
        self.buffer_text(ctx).lines().count()
    }

    #[cfg(feature = "integration_tests")]
    pub fn input_suggestions(&self) -> &ViewHandle<InputSuggestions> {
        &self.input_suggestions
    }

    pub fn suggestions_mode_model(&self) -> &ModelHandle<InputSuggestionsModeModel> {
        &self.suggestions_mode_model
    }

    pub fn completer_data(&self) -> CompleterData {
        CompleterData::new(
            self.sessions.clone(),
            self.active_block_metadata.clone(),
            CommandRegistry::global_instance(),
            self.last_user_block_completed.clone(),
        )
    }

    fn start_byte_index_of_first_selection(&self, ctx: &ViewContext<Self>) -> ByteOffset {
        self.editor
            .as_ref(ctx)
            .start_byte_index_of_first_selection(ctx)
    }

    // Returns the appropriate hint/placeholder text to render in an empty input when Agent Mode is
    // enabled (the feature flag, not the specific AI input mode). This method ensures that hint text
    // is cached when needed for new conversations.
    fn agent_mode_hint_text(&mut self, app: &AppContext) -> String {
        let input_model = self.ai_input_model.as_ref(app);
        let is_udi_enabled = InputSettings::as_ref(app).is_universal_developer_input_enabled(app);
        let selected_conversation_id = self
            .ai_context_model
            .as_ref(app)
            .selected_conversation_id(app);
        let is_queue_next_prompt_enabled = FeatureFlag::QueueSlashCommand.is_enabled()
            && selected_conversation_id.is_some_and(|conversation_id| {
                let terminal_model = self.model.lock();
                QueuedQueryModel::as_ref(app).is_queue_next_prompt_enabled(
                    conversation_id,
                    terminal_model.block_list().active_block(),
                    app,
                )
            });

        match (
            input_model.input_type(),
            input_model.should_run_input_autodetection(app),
        ) {
            (InputType::Shell, false) => {
                AGENT_MODE_AI_DISABLED_AUTODETECTION_DISABLED_HINT_TEXT.to_owned()
            }
            (InputType::Shell, true) => {
                // Ensure hint text is cached for new conversations
                get_stable_agent_mode_hint_text(&mut self.cached_agent_mode_hint_text).to_owned()
            }
            (InputType::AI, _) => {
                if let Some(conversation) =
                    self.ai_context_model.as_ref(app).selected_conversation(app)
                    && conversation.is_child_agent_conversation()
                {
                    let agent_name = conversation.agent_name().unwrap_or("child");
                    if conversation.status().is_in_progress() {
                        if is_queue_next_prompt_enabled {
                            return format!("Queue a follow up for the {agent_name} agent");
                        }
                        return format!("Steer the {agent_name} agent");
                    }
                    return format!("Ask the {agent_name} agent a follow up");
                }

                // Follow the `agent_indicator` pattern (see `app/src/tab.rs`):
                //  * `None` (no conversation, empty, or untitled) => new conversation => "Warp anything"
                //  * `InProgress`                                           => agent running    => "Steer"
                //  * Any other status                                       => finished         => "Ask a follow up"
                match self
                    .ai_context_model
                    .as_ref(app)
                    .selected_conversation_status_for_hint(app)
                {
                    Some(status) if status.is_in_progress() => {
                        if is_queue_next_prompt_enabled {
                            if is_udi_enabled {
                                AGENT_MODE_AI_ENABLED_QUEUE_HINT_TEXT_UDI.to_owned()
                            } else {
                                AGENT_MODE_AI_ENABLED_QUEUE_HINT_TEXT_CLASSIC.to_owned()
                            }
                        } else if is_udi_enabled {
                            AGENT_MODE_AI_ENABLED_STEER_HINT_TEXT_UDI.to_owned()
                        } else {
                            AGENT_MODE_AI_ENABLED_STEER_HINT_TEXT_CLASSIC.to_owned()
                        }
                    }
                    Some(_) => {
                        if is_udi_enabled {
                            AGENT_MODE_AI_ENABLED_FOLLOW_UP_HINT_TEXT_UDI.to_owned()
                        } else {
                            AGENT_MODE_AI_ENABLED_FOLLOW_UP_HINT_TEXT_CLASSIC.to_owned()
                        }
                    }
                    None => {
                        // Ensure hint text is cached for new conversations
                        get_stable_agent_mode_hint_text(&mut self.cached_agent_mode_hint_text)
                            .to_owned()
                    }
                }
            }
        }
    }

    fn handle_input_settings_event(
        &mut self,
        input_settings: ModelHandle<InputSettings>,
        event: &InputSettingsChangedEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            InputSettingsChangedEvent::ShowHintText { .. } => {
                self.set_zero_state_hint_text(ctx);
                ctx.notify();
            }
            InputSettingsChangedEvent::SyntaxHighlighting { .. } => {
                if !*input_settings.as_ref(ctx).syntax_highlighting.value() {
                    self.clear_decorations(ctx);
                }
                self.run_input_background_jobs(
                    InputBackgroundJobOptions::default().with_command_decoration(),
                    ctx,
                );
            }
            InputSettingsChangedEvent::ErrorUnderliningEnabled { .. } => {
                if !*input_settings.as_ref(ctx).error_underlining.value() {
                    self.clear_decorations(ctx);
                }
                self.run_input_background_jobs(
                    InputBackgroundJobOptions::default().with_command_decoration(),
                    ctx,
                );
            }
            InputSettingsChangedEvent::InputBoxTypeSetting { .. } => {
                // Force a re-render when switching between Universal and Classic input modes
                // to ensure all UI elements update in real-time
                self.set_zero_state_hint_text(ctx);
                ctx.notify();
            }
            InputSettingsChangedEvent::CompletionsMenuWidth { .. } => {
                let new_value = *input_settings.as_ref(ctx).completions_menu_width.value();
                if let Ok(mut guard) = self.completions_menu_resizable_width.lock() {
                    guard.set_size(new_value);
                }
                ctx.notify();
            }
            InputSettingsChangedEvent::CompletionsMenuHeight { .. } => {
                let new_value = *input_settings.as_ref(ctx).completions_menu_height.value();
                if let Ok(mut guard) = self.completions_menu_resizable_height.lock() {
                    guard.set_size(new_value);
                }
                ctx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn attach_file(&mut self, ctx: &mut ViewContext<Self>) {
        self.agent_input_footer.update(ctx, |footer, ctx| {
            footer.select_file(ctx);
        });
    }

    fn select_image(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus_input_box(ctx);
        self.ensure_agent_mode_for_ai_features(
            true,
            Some(InputTypeAutoDetectionSource::AttachmentForcedAi),
            ctx,
        );

        // Update image context options immediately after switching to AI mode
        // to ensure attach_images has the correct state
        self.update_image_context_options(ctx);

        self.editor.update(ctx, |editor, ctx| {
            editor.attach_files(ctx);
        });
    }
    fn handle_prompt_alert(
        &mut self,
        prompt_alert: &PromptAlertEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match prompt_alert {
            PromptAlertEvent::SignupAnonymousUser => {
                ctx.emit(Event::SignupAnonymousUser {
                    entrypoint: AnonymousUserSignupEntrypoint::SignUpAIPrompt,
                });
            }
        }
    }

    fn enable_auto_detection(&mut self, ctx: &mut ViewContext<Self>) {
        // Don't allow input mode changes for read-only viewers in shared sessions
        if self.model.lock().shared_session_status().is_reader() {
            return;
        }

        // Don't allow enabling autodetection when agent is monitoring a command
        if self
            .model
            .lock()
            .block_list()
            .active_block()
            .is_agent_in_control_or_tagged_in()
        {
            return;
        }

        let ai_settings = AISettings::as_ref(ctx);
        if FeatureFlag::AgentView.is_enabled() {
            if self.agent_view_controller.as_ref(ctx).is_fullscreen() {
                if !ai_settings.is_ai_autodetection_enabled(ctx) {
                    return;
                }
            } else if !ai_settings.is_nld_in_terminal_enabled(ctx) {
                return;
            }
        } else if !ai_settings.is_ai_autodetection_enabled(ctx) {
            return;
        }

        self.focus_input_box(ctx);

        if !self.ai_input_model.as_ref(ctx).is_input_type_locked() {
            return;
        }

        let buffer_text = self.buffer_text(ctx);
        if buffer_text.is_empty() {
            // For empty buffer, immediately set to Shell mode with auto-detection enabled
            self.ai_input_model.update(ctx, |model, ctx| {
                let new_config = InputConfig {
                    input_type: InputType::Shell,
                    is_locked: false, // Set to auto-detection mode
                };
                model.set_input_config(new_config, buffer_text.is_empty(), None, ctx);
            });
        } else {
            // For non-empty buffer, run the actual auto-detection algorithm
            // First unlock the input mode to enable auto-detection
            self.ai_input_model.update(ctx, |model, ctx| {
                let current_config = model.input_config();
                let new_config = InputConfig {
                    input_type: current_config.input_type, // Keep current type temporarily
                    is_locked: false,                      // Enable auto-detection
                };
                model.set_input_config(new_config, buffer_text.is_empty(), None, ctx);
            });

            // Then run auto-detection on the current buffer content
            self.run_input_background_jobs(
                InputBackgroundJobOptions::default().with_ai_input_detection(),
                ctx,
            );
        }
    }

    fn handle_universal_developer_input_button_bar_event(
        &mut self,
        event: &UniversalDeveloperInputButtonBarEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            UniversalDeveloperInputButtonBarEvent::InputTypeSelected(input_type) => {
                if self.is_input_mode_toggle_disabled(ctx) {
                    return;
                }

                self.focus_input_box(ctx);

                let is_input_buffer_empty = self.editor.as_ref(ctx).buffer_text(ctx).is_empty();

                let switch_to_auto = self.ai_input_model.update(ctx, |model, ctx| {
                    let is_autodetection_enabled =
                        AISettings::as_ref(ctx).is_ai_autodetection_enabled(ctx);
                    let input_type = *input_type;

                    // If the user clicked on the button to "unlock" the current mode,
                    // we want to enable autodetection.
                    if input_type == model.input_type()
                        && is_autodetection_enabled
                        && model.is_input_type_locked()
                    {
                        true
                    } else {
                        let new_config = InputConfig {
                            input_type,
                            is_locked: true,
                        };
                        model.set_input_config(
                            new_config,
                            is_input_buffer_empty,
                            Some(InputTypeAutoDetectionSource::ManualToggle),
                            ctx,
                        );
                        false
                    }
                });

                if switch_to_auto {
                    self.set_input_mode_natural_language_detection(ctx);
                } else if *input_type == InputType::AI {
                    send_telemetry_from_ctx!(
                        TelemetryEvent::AgentModeClickedEntrypoint {
                            entrypoint: AgentModeEntrypoint::UDITerminalInputSwitcher,
                        },
                        ctx
                    );
                }
            }
            UniversalDeveloperInputButtonBarEvent::EnableAutoDetection => {
                self.enable_auto_detection(ctx);
            }
            UniversalDeveloperInputButtonBarEvent::SelectFile => {
                self.select_image(ctx);
            }
            UniversalDeveloperInputButtonBarEvent::PromptAlert(prompt_alert_event) => {
                self.handle_prompt_alert(prompt_alert_event, ctx);
            }
        }
    }

    /// Switches to AI mode but preserves current lock state.
    fn enter_ai_mode(
        &mut self,
        decision_source: Option<InputTypeAutoDetectionSource>,
        ctx: &mut ViewContext<Self>,
    ) {
        let is_input_buffer_empty = self.editor.as_ref(ctx).buffer_text(ctx).is_empty();
        self.ai_input_model.update(ctx, |input_model, ctx| {
            let new_config = input_model.input_config().with_input_type(InputType::AI);
            input_model.set_input_config(new_config, is_input_buffer_empty, decision_source, ctx);
        });
    }

    /// Helper function to ensure agent mode when needed, using the same logic as SelectFile.
    /// This handles the transition from shell mode to agent mode while preserving lock semantics.
    /// Only forces agent mode if the user hasn't explicitly locked the mode to Shell.
    ///
    /// Pass `decision_source` to attribute the resulting input type change in NLD telemetry;
    /// callers without a meaningful source may pass `None`.
    pub fn ensure_agent_mode_for_ai_features(
        &mut self,
        should_override_shell_lock: bool,
        decision_source: Option<InputTypeAutoDetectionSource>,
        ctx: &mut ViewContext<Self>,
    ) {
        let ai_input_model = self.ai_input_model.as_ref(ctx);
        let config = ai_input_model.input_config();

        // Don't force agent mode if user has explicitly locked to Shell mode
        if (!should_override_shell_lock || FeatureFlag::AgentView.is_enabled())
            && config.is_locked
            && !config.input_type.is_ai()
        {
            return;
        }
        self.enter_ai_mode(decision_source, ctx);
    }

    /// Clear the cached hint text to generate a new one on next render
    pub fn clear_cached_hint_text(&mut self) {
        self.cached_agent_mode_hint_text = None;
    }
    pub fn set_zero_state_hint_text(&mut self, ctx: &mut ViewContext<Self>) {
        if self.prefix_mode(ctx) == InputPrefixMode::CloudHandoff {
            let conversation_is_empty = BlocklistAIHistoryModel::as_ref(ctx)
                .active_conversation(self.terminal_view_id)
                .is_none_or(|c| c.is_empty());
            let hint = if conversation_is_empty {
                CLOUD_MODE_V2_HINT_TEXT.to_owned()
            } else {
                self.handoff_compose_state
                    .as_ref(ctx)
                    .selected_environment_id()
                    .and_then(|id| CloudAmbientAgentEnvironment::get_by_id(id, ctx))
                    .map(|env| format!("Hand off to {}", env.model().string_model.display_name()))
                    .unwrap_or_else(|| "Handoff to cloud".to_owned())
            };
            self.editor.update(ctx, |editor, ctx| {
                editor.set_placeholder_text(&hint, ctx);
            });
            return;
        }

        if self.is_cloud_mode_input_v2_composing(ctx) {
            let show_hint = *InputSettings::as_ref(ctx).show_hint_text;
            self.editor.update(ctx, |editor, ctx| {
                if show_hint {
                    editor.set_placeholder_text(CLOUD_MODE_V2_HINT_TEXT, ctx);
                } else {
                    editor.clear_placeholder_text(ctx);
                }
            });
            return;
        }
        let toggled_on = *InputSettings::as_ref(ctx).show_hint_text;

        if toggled_on
            && FeatureFlag::AgentMode.is_enabled()
            && AISettings::as_ref(ctx).is_any_ai_enabled(ctx)
        {
            let hint_text = self.agent_mode_hint_text(ctx);
            self.editor.update(ctx, |editor, ctx| {
                editor.set_placeholder_text(hint_text, ctx);
            });
        } else {
            self.editor.update(ctx, |editor, ctx| {
                editor.clear_placeholder_text(ctx);
                ctx.notify();
            });
        }
    }

    /// Finds the start byte of the token under the given hovered point
    fn start_byte_index_at_point(
        &self,
        point: &DisplayPoint,
        ctx: &AppContext,
    ) -> Option<ByteOffset> {
        self.editor.read(ctx, |editor, ctx| {
            editor.start_byte_offset_at_point(point, ctx)
        })
    }

    fn handle_safe_mode_settings_changed_event(
        &mut self,
        event: &SafeModeSettingsChangedEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            SafeModeSettingsChangedEvent::SafeModeEnabled { .. }
            | SafeModeSettingsChangedEvent::HideSecretsInBlockList { .. }
            | SafeModeSettingsChangedEvent::SecretDisplayModeSetting { .. } => {
                self.model
                    .lock()
                    .set_obfuscate_secrets(get_secret_obfuscation_mode(ctx));
            }
        }
    }

    fn handle_ai_settings_changed_event(
        &mut self,
        event: &AISettingsChangedEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            AISettingsChangedEvent::IsAnyAIEnabled { .. } => {
                let ai_settings = AISettings::handle(ctx);
                self.set_zero_state_hint_text(ctx);

                let is_input_buffer_empty = self.editor.as_ref(ctx).buffer_text(ctx).is_empty();
                // If there is no AI enabled, ensure input is locked in command mode.
                if !ai_settings.as_ref(ctx).is_any_ai_enabled(ctx) {
                    self.ai_input_model.update(ctx, |input_model, ctx| {
                        input_model.set_input_config(
                            InputConfig {
                                input_type: InputType::Shell,
                                is_locked: true,
                            },
                            is_input_buffer_empty,
                            None,
                            ctx,
                        );
                    });
                }

                ctx.notify();
            }
            AISettingsChangedEvent::AIAutoDetectionEnabled { .. }
            | AISettingsChangedEvent::NLDInTerminalEnabled { .. } => {
                // NLD is irrelevant in cloud mode v2 — the input is always AI.
                if self.is_cloud_mode_input_v2_composing(ctx) {
                    return;
                }
                // The input model handles updating the lock state via its own subscription.
                // If NLD is now enabled for the current context and the buffer is non-empty,
                // trigger autodetection on the current buffer contents.
                if self
                    .ai_input_model
                    .as_ref(ctx)
                    .should_run_input_autodetection(ctx)
                    && !self.editor.as_ref(ctx).buffer_text(ctx).is_empty()
                {
                    self.run_input_background_jobs(
                        InputBackgroundJobOptions::default().with_ai_input_detection(),
                        ctx,
                    );
                }
            }
            _ => {}
        }
    }

    fn handle_ignored_suggestions_event(
        &mut self,
        event: &IgnoredSuggestionsModelEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            IgnoredSuggestionsModelEvent::SuggestionIgnored => {
                // We may need to regenerate the autosuggestion if the suggestion just ignored
                // was the one suggested in the input.
                self.editor.update(ctx, |editor, ctx| {
                    editor.clear_autosuggestion(ctx);
                });
                self.maybe_generate_autosuggestion(ctx);
            }
        }
    }

    /// Returns `true` if we can query the [`History`] model for the active session.
    fn can_query_history(&self, ctx: &AppContext) -> bool {
        let model = self.model.lock();
        let Some(session_id) = model.block_list().active_block().session_id() else {
            return false;
        };

        let is_bootstrapped = model.block_list().is_bootstrapped();
        let is_history_queryable = History::as_ref(ctx).is_queryable(&session_id);

        // TODO: we should investigate why we need to check for bootstrapped here.
        // It's confusing and might actually be implied
        // (session history is only queryable if the session is bootstrapped).

        // We also return true for shared session executors since they're able to view the history
        // of a shared session without yet being hooked up to the history model.
        is_bootstrapped && (is_history_queryable || model.shared_session_status().is_executor())
    }

    /// Returns enum indicating if we can execute a command in the active session.
    ///
    /// We can only execute a command if:
    /// 1. the session is bootstrapped, because we don't want to interfere
    ///    with the PTY while bootstrapping is in progress
    /// 2. there isn't an active, long-running command (in-band commands are okay)
    /// 3. if the history for the session is appendable, because we want to
    ///    acknowledge the command in the session's history. Except when viewing
    ///    a shared session, since those sessions aren't registered in the [`History`]
    ///    model.
    fn can_execute_command(&self, ctx: &AppContext) -> CanExecuteCommand {
        let model = self.model.lock();
        let active_block = model.block_list().active_block();

        if !model.block_list().is_bootstrapped() {
            CanExecuteCommand::No(DenyExecutionReason::NotBootstrapped)
        } else if active_block.is_active_and_long_running()
            && !active_block.is_in_band_command_block()
        {
            CanExecuteCommand::No(DenyExecutionReason::ExistingActiveCommand)
        } else if !model.shared_session_status().is_executor()
            && active_block
                .session_id()
                .is_none_or(|session_id| !History::as_ref(ctx).is_appendable(&session_id))
        {
            CanExecuteCommand::No(DenyExecutionReason::HistoryNotAppendable)
        } else {
            CanExecuteCommand::Yes
        }
    }

    pub fn execute_pending_command(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.has_pending_command {
            return;
        }

        let command = self.get_command(ctx);
        if self.can_execute_command(ctx).is_no() {
            return;
        }

        self.try_execute_command(&command, ctx);
        self.has_pending_command = false;

        self.editor.update(ctx, |editor, ctx| {
            editor.set_interaction_state(InteractionState::Editable, ctx);
        });
    }

    /// Try to execute a command in the local session that was
    /// requested by a shared session participant (sharer or viewer).
    ///
    /// Returns `true` if the command was executed, `false` otherwise.
    pub fn try_execute_command_on_behalf_of_shared_session_participant(
        &mut self,
        command: &str,
        participant_id: ParticipantId,
        preserve_input: bool,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        // Cancel any active agent conversation when the sharer executes a command on behalf of the viewer
        // (this is handled automatically when the sharer executes a command that they requested).
        // This will also notify viewers to cancel their representation of the conversation.
        let is_participant_viewer = self
            .shared_session_presence_manager
            .as_ref()
            .and_then(|pm| pm.as_ref(ctx).get_participant(&participant_id))
            .and_then(|participant| participant.role)
            .is_some();
        if FeatureFlag::AgentMode.is_enabled()
            && self.model.lock().shared_session_status().is_sharer()
            && is_participant_viewer
        {
            self.cancel_active_agent_conversation_for_shared_session(
                CancellationReason::UserCommandExecuted,
                ctx,
            );
        }

        let block_id = self.model.lock().block_list().active_block_id().clone();
        self.try_execute_command_from_source(
            command,
            CommandExecutionSource::SharedSession {
                participant_id,
                block_id,
                ai_metadata: None,
                preserve_input,
            },
            true,
            ctx,
        )
    }

    /// Freeze the editor and put it in a loading state.
    pub fn freeze_input_in_loading_state(&mut self, ctx: &mut ViewContext<Self>) -> String {
        let buffer_text = self.editor.as_ref(ctx).buffer_text(ctx);
        self.freeze_input_in_loading_state_with_text(&buffer_text, ctx);
        buffer_text
    }

    /// Freeze the editor and render `"{display_text} ◌"` as the loading indicator.
    /// Shared between the user-initiated viewer submission path (which passes the
    /// editor's current buffer text) and the queued-prompt drain path (which passes
    /// the popped prompt text without ever reading from / writing to the user's
    /// in-progress buffer).
    fn freeze_input_in_loading_state_with_text(
        &mut self,
        buffer_text: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        self.editor.update(ctx, |editor, ctx| {
            // Use an ephemeral edit to show the loading state
            // and disallow edits.
            // TODO: the ◌ treatment is a stop-gap to rendering an svg
            // to the right of the buffer text.
            editor.set_buffer_text_ignoring_undo(&format!("{buffer_text} ◌"), ctx);
            editor.set_interaction_state(InteractionState::Selectable, ctx);

            // We manually set the text color to appear disabled.
            // We could use the [`InteractionState::Disabled`] interaction state
            // but that disallows text selection.
            let appearance = Appearance::as_ref(ctx);
            editor.set_text_colors(TextColors::all_hint_color(appearance), ctx);
        });
    }

    pub fn try_execute_command(&mut self, command: &str, ctx: &mut ViewContext<Self>) -> bool {
        self.try_execute_command_with_options(command, false, ctx)
    }

    /// Applies `selection` only if `session_id` matches the in-flight handoff.
    pub fn set_external_shell_widget_selection(&mut self, session_id: SessionId, selection: &str) {
        let Some(handoff) = self.pending_shell_widget_handoff.as_mut() else {
            return;
        };
        handoff.maybe_apply_selection(session_id, selection);
    }

    /// Runs `helper_command` (a bootstrap-installed shell function), snapshotting the current
    /// buffer so it can be restored once the command's block completes. Returns `true` if the
    /// command was started.
    pub fn trigger_external_shell_widget_handoff(
        &mut self,
        helper_command: &str,
        apply_mode: ShellWidgetApplyMode,
        capture_cursor: bool,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(session_id) = self.active_block_session_id() else {
            return false;
        };
        let original_buffer = self.buffer_text(ctx);
        let cursor_offset = capture_cursor.then(|| {
            self.editor
                .as_ref(ctx)
                .end_byte_index_of_last_selection(ctx)
        });
        let block_id = self.model.lock().block_list().active_block_id().clone();
        // Prefixed with a leading space, the "ignorespace" convention.
        let mut command = format!(" {helper_command}");
        if let Some(cursor_offset) = cursor_offset
            && apply_mode == ShellWidgetApplyMode::Replace
        {
            let char_cursor = original_buffer[..cursor_offset.as_usize()].chars().count();
            command.push_str(&format!(" {char_cursor}:{}", hex::encode(&original_buffer)));
        }
        let started = self.try_execute_command_from_source(
            &command,
            CommandExecutionSource::User,
            false, /* should_add_command_to_history */
            ctx,
        );
        if started {
            self.pending_shell_widget_handoff = Some(PendingShellWidgetHandoff {
                session_id,
                original_buffer,
                selection: None,
                block_id,
                apply_mode,
                cursor_offset,
            });
        }
        started
    }

    fn try_execute_command_with_options(
        &mut self,
        command: &str,
        preserve_input: bool,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let shared_session_status = self.model.lock().shared_session_status().clone();
        if shared_session_status.is_sharer_or_viewer() {
            // If this is a viewer who isn't also an executor, they should not
            // be allowed to execute commands.
            if shared_session_status.is_reader() {
                // TODO: consider showing a toast in this scenario. It should be unlikely
                // that a viewer can get here without being an executor because the main
                // caller of this API is the `enter` handler.
                log::warn!("Viewer tried to execute a command as a reader");
                return false;
            } else if shared_session_status.is_executor() && !preserve_input {
                let original_buffer = self.freeze_input_in_loading_state(ctx);

                if let Some(shared_session_input_state) = self.shared_session_input_state.as_mut() {
                    shared_session_input_state.pending_command_execution_request =
                        Some(ViewerCommandExecutionRequest { original_buffer });
                }
            }

            // Get our own shared session participant ID.
            let Some(participant_id) = self
                .shared_session_presence_manager
                .as_ref()
                .map(|m| m.as_ref(ctx).id())
            else {
                return false;
            };
            self.try_execute_command_on_behalf_of_shared_session_participant(
                command,
                participant_id,
                preserve_input,
                ctx,
            )
        } else if preserve_input {
            self.try_execute_command_from_source(
                command,
                CommandExecutionSource::QueuedCommand,
                true,
                ctx,
            )
        } else {
            self.try_execute_command_from_source(command, CommandExecutionSource::User, true, ctx)
        }
    }

    /// Executes a command drained or sent immediately from the queued-prompts panel and keeps the
    /// remaining queue paused until the command's terminal block finishes.
    pub(crate) fn execute_queued_command(
        &mut self,
        command: &str,
        conversation_id: AIConversationId,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let started = self.try_execute_command_with_options(command, true, ctx);
        if started {
            QueuedQueryModel::handle(ctx).update(ctx, |model, _| {
                model.arm_command_in_flight(conversation_id);
            });
        }
        started
    }

    fn has_queued_command_in_flight(&self, ctx: &AppContext) -> bool {
        QueuedQueryModel::as_ref(ctx)
            .command_in_flight_for_terminal_view(
                self.terminal_view_id,
                BlocklistAIHistoryModel::as_ref(ctx),
            )
            .is_some()
    }

    /// Executes the given command if the terminal session is in a valid state to accept and
    /// execute a command. Afterwards, ensures the workflows info menu and input suggestions menu
    /// are both closed.
    ///
    /// This will _not_ execute a command if any of the following are true:
    ///     1. The history list and/or blocklist are not yet bootstrapped.
    ///     2. The active blocklist has not yet received the precmd payload.
    ///     3. There is an active, long-running command.
    ///
    /// Returns `true` if the command was executed, `false` otherwise.
    fn try_execute_command_from_source(
        &mut self,
        command: &str,
        source: CommandExecutionSource,
        should_add_command_to_history: bool,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if let CanExecuteCommand::No(reason) = self.can_execute_command(ctx) {
            if reason.is_existing_active_command() {
                const MAX_COMMAND_LENGTH: usize = 43;
                let truncated_command = truncate_from_end(command, MAX_COMMAND_LENGTH);

                // Block user submissions while a requested command is actively running
                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                    toast_stack.add_ephemeral_toast(
                        DismissibleToast::error(format!(
                            "Cannot run `{truncated_command}` (command already running)."
                        )),
                        window_id,
                        ctx,
                    );
                });
            }

            log::warn!("Tried to execute command but can_execute_command was false: {reason:?}");
            return false;
        }

        // Clear the auto-suggestion in the editor, so the height of
        // the input box is not inaccurate for its contents. Since we
        // we adjust the height of the long running block to be the same
        // as the height of the input box, we don't want the long
        // running block to have a lot of extra space for the frames
        // before it has any output or if it's a command that doesn't
        // have any output.
        //
        // Note that we do not clear the input box here (we do it in
        // `TerminalView` when we handle the `BlockCompleted` message
        // instead) for a similar reason. Specifically, we don't want
        // multi-line commands to have the height of the empty input
        // box because we don't want its contents to be cut off.
        if !command.is_empty() {
            self.editor.update(ctx, |editor, ctx| {
                editor.clear_autosuggestion(ctx);
                editor.clear_all_placeholder_text();
                ctx.notify();
            });
        }

        let home_dir = prompt::home_dir_for_block(
            self.model.lock().block_list().active_block(),
            self.sessions.as_ref(ctx),
        );
        self.model
            .lock()
            .block_list_mut()
            .active_block_mut()
            .set_home_dir(home_dir);

        let env_var_collection_id = self.env_var_collection_state.selected_env_vars;
        self.model
            .lock()
            .block_list_mut()
            .active_block_mut()
            .set_cloud_env_var_state(env_var_collection_id);

        // Record whether NLD was overridden (input type manually locked) at submission time.
        let nld_overridden = self.ai_input_model.as_ref(ctx).is_input_type_locked();
        self.model
            .lock()
            .block_list_mut()
            .active_block_mut()
            .set_nld_overridden(nld_overridden);

        let did_execute = if self
            .model
            .lock()
            .block_list()
            .active_block()
            .has_received_precmd()
        {
            self.tips_completed.update(ctx, |tips, ctx| {
                mark_feature_used_and_write_to_user_defaults(
                    Tip::Hint(TipHint::CreateBlock),
                    tips,
                    ctx,
                );
                ctx.notify();
            });

            if !command.is_empty() {
                IgnoredSuggestionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.remove_ignored_suggestion(
                        command.to_string(),
                        SuggestionType::ShellCommand,
                        ctx,
                    );
                });
            }

            self.start_block_and_write_command_to_pty(
                command,
                source,
                should_add_command_to_history,
                ctx,
            );
            true
        } else {
            // We don't want to submit the command if precmd has not
            // been received. Instead, we want the user to be aware
            // that the prompt might not be up to date.
            send_telemetry_from_ctx!(TelemetryEvent::TriedToExecuteBeforePrecmd, ctx);
            false
        };

        // Close the workflows info box if it was open.
        self.clear_selected_workflow(ctx);

        // Close the input suggestions menu if it was open.
        self.close_input_suggestions(/*should_focus_input=*/ false, ctx);
        did_execute
    }

    /// We locked the viewer's input when they attempted to execute a command.
    /// On failure, we must restore the editor to its original state before the attempt.
    pub fn on_execute_command_for_shared_session_participant_failure(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(shared_session_input_state) = self.shared_session_input_state.as_mut() else {
            return;
        };
        let Some(ViewerCommandExecutionRequest { original_buffer }) = shared_session_input_state
            .pending_command_execution_request
            .as_ref()
        else {
            return;
        };

        // Unfreeze the editor
        if let SharedSessionStatus::ActiveViewer { role } =
            self.model.lock().shared_session_status()
        {
            self.editor.update(ctx, |editor, ctx| {
                // Restore the original buffer and interaction state based on the viewer's role.
                editor.set_buffer_text(original_buffer, ctx);
                editor.set_interaction_state(role.into(), ctx);

                // Shared-session pending-command and cloud-followup flows can swap the editor into
                // a frozen/pending color treatment, so restore the normal palette alongside the
                // buffer + interaction state reset.
                let appearance: &Appearance = Appearance::as_ref(ctx);
                editor.set_text_colors(TextColors::from_appearance(appearance), ctx);
            });
        }
        shared_session_input_state.pending_command_execution_request = None;
    }

    /// Restores the frozen/loading visual state of the agent input for both the sharer
    /// and viewer without touching the CRDT buffer contents.
    ///
    /// Does NOT clear or reinitialize the buffer. Buffer clearing for agent prompts is
    /// handled by the sharer emitting CRDT delete operations via `system_clear_buffer`
    /// (triggered when `BlocklistAIControllerEvent::SentRequest` fires). Viewers receive
    /// those delete ops through `InputUpdated` and apply them via the normal CRDT path.
    ///
    /// For viewers, this exits the ephemeral loading state created by
    /// `freeze_input_in_loading_state`. When `is_shared_session_viewer_prompt_inflight` is true,
    /// we optimistically clear the buffer using a display-only empty ephemeral
    /// so the viewer sees an empty buffer immediately before crdt operations for actually clearing
    /// the real buffer are received from the sharer.
    ///
    /// The display-only ephemeral is safe for CRDT: when the viewer next makes an edit
    /// (materializing the ephemeral), its empty content is **discarded** — no delete ops
    /// are generated for the regular buffer's contents. The edit proceeds directly on
    /// the regular buffer (which the sharer's delete ops will have cleared by then).
    pub fn unfreeze_agent_input(
        &mut self,
        is_shared_session_viewer_prompt_inflight: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if matches!(
            self.model.lock().shared_session_status(),
            SharedSessionStatus::ActiveViewer { .. } | SharedSessionStatus::ActiveSharer
        ) {
            self.editor.update(ctx, |editor, ctx| {
                if let SharedSessionStatus::ActiveViewer { role } =
                    self.model.lock().shared_session_status()
                {
                    // reinstate role for viewers
                    editor.set_interaction_state(role.into(), ctx);
                    // Exit the ephemeral loading state so the regular CRDT buffer is
                    // accessible. The sharer's delete ops (arriving via InputUpdated)
                    // will clear the regular buffer.
                    editor.exit_ephemeral_loading_state(ctx);
                    if is_shared_session_viewer_prompt_inflight {
                        // Create a display-only empty ephemeral for immediate visual
                        // feedback. This is an optimistic clear for UI purposes, without
                        // affecting the real buffer synced by crdt operations.
                        // Unlike a regular ephemeral, materializing this one
                        // discards its content instead of restoring it to the regular
                        // buffer, so no spurious CRDT delete ops are generated.
                        editor.show_display_only_empty_buffer(ctx);
                    }
                }

                let appearance: &Appearance = Appearance::as_ref(ctx);
                editor.set_text_colors(TextColors::from_appearance(appearance), ctx);
            });
        }
    }

    /// Restores a VM-down cloud follow-up after an attachment upload fails. Unlike
    /// [`Self::unfreeze_agent_input`], this path runs on a disconnected cloud pane rather than an
    /// active shared-session viewer, so it must restore the visible prompt and editable state
    /// directly.
    fn restore_cloud_followup_input_after_upload_failure(
        &mut self,
        prompt: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        self.editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(prompt, ctx);
            editor.set_interaction_state(InteractionState::Editable, ctx);
            let appearance: &Appearance = Appearance::as_ref(ctx);
            editor.set_text_colors(TextColors::from_appearance(appearance), ctx);
        });
    }

    pub fn reset_after_cloud_followup_submission(&mut self, ctx: &mut ViewContext<Self>) {
        self.editor.update(ctx, |editor, ctx| {
            editor.set_interaction_state(InteractionState::Editable, ctx);
            editor.clear_buffer_and_reset_undo_stack(ctx);

            let appearance: &Appearance = Appearance::as_ref(ctx);
            editor.set_text_colors(TextColors::from_appearance(appearance), ctx);
        });
    }

    /// Cancel any active agent conversation in a shared session
    /// and fan out a cancellation control action.
    pub(crate) fn cancel_active_agent_conversation_for_shared_session(
        &mut self,
        cancellation_reason: CancellationReason,
        ctx: &mut ViewContext<Self>,
    ) {
        let active_conversation =
            BlocklistAIHistoryModel::as_ref(ctx).active_conversation(self.terminal_view_id);

        if self.model.lock().shared_session_status().is_viewer() {
            let server_conversation_token = active_conversation
                .and_then(|conversation| conversation.server_conversation_token().cloned())
                .and_then(|server_token| {
                    server_token
                        .as_str()
                        .parse()
                        .ok()
                        .map(ServerConversationToken::from_uuid)
                });

            if let Some(server_conversation_token) = server_conversation_token {
                ctx.emit(Event::CancelSharedSessionConversation {
                    server_conversation_token,
                });
            }
        } else if self.model.lock().shared_session_status().is_sharer() {
            let active_conversation_id = active_conversation
                .filter(|conversation| conversation.status().is_in_progress())
                .map(|conversation| conversation.id());

            if let Some(active_conversation_id) = active_conversation_id {
                // First, cancel locally via the existing pipeline.
                self.ai_controller.update(ctx, |controller, ctx| {
                    controller.cancel_conversation_progress(
                        active_conversation_id,
                        cancellation_reason,
                        ctx,
                    );
                });
            }
        }
    }

    fn clear_selected_env_var_collection(&mut self) {
        self.env_var_collection_state.selected_env_vars = None;
    }

    /// Closes the workflows panel.
    fn clear_selected_workflow(&mut self, ctx: &mut ViewContext<Self>) {
        // Clear the env var state if we had one.
        self.clear_selected_env_var_collection();

        // `take()` closes the Workflows panel because the panel is only
        // rendered if `selected_workflow_state` is Some(..).
        if let Some(state) = self.workflows_state.selected_workflow_state.take() {
            self.update_workflows_info_box_expanded_setting(ctx, &state);
        }
        ctx.notify();
    }

    /// Hides the workflows panel, persisting the shift-tab UX.
    fn hide_workflows_info_box(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(state) = &mut self.workflows_state.selected_workflow_state {
            state.should_show_more_info_view = false;
        }
        if let Some(state) = self.workflows_state.selected_workflow_state.clone() {
            self.update_workflows_info_box_expanded_setting(ctx, &state);
        }
        ctx.notify();
    }

    /// Returns the starting byte index position of the last selection.
    fn start_byte_index_of_last_selection(&self, ctx: &ViewContext<Self>) -> ByteOffset {
        self.editor
            .as_ref(ctx)
            .start_byte_index_of_last_selection(ctx)
    }

    fn handle_session_settings_event(
        &mut self,
        evt: &SessionSettingsChangedEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match evt {
            SessionSettingsChangedEvent::HonorPS1 { .. } => {
                let mut model = self.model.lock();
                model.set_honor_ps1(*SessionSettings::as_ref(ctx).honor_ps1);
                ctx.notify();
            }
            SessionSettingsChangedEvent::SavedPrompt { .. } => {
                self.notify_and_notify_children(ctx);
            }
            _ => {}
        }
    }

    fn handle_app_editor_settings_event(
        &mut self,
        settings: ModelHandle<AppEditorSettings>,
        evt: &AppEditorSettingsChangedEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        if let AppEditorSettingsChangedEvent::EnableAutosuggestions { .. } = evt {
            let next_enable_autosuggestions_setting =
                *AppEditorSettings::as_ref(ctx).enable_autosuggestions;
            if self.enable_autosuggestions_setting && !next_enable_autosuggestions_setting {
                // Clear the active autosuggestion if autosuggestions was turned off.
                self.editor.update(ctx, |view, ctx| {
                    view.clear_autosuggestion(ctx);
                });
                ctx.notify();
            }
            // Ensure our cached copy of the enabled_autosuggestions setting
            // is up-to-date.
            self.enable_autosuggestions_setting = next_enable_autosuggestions_setting;
        }

        // The cursor and status bar may change appearance when vim mode is enabled or disabled.
        if let AppEditorSettingsChangedEvent::VimModeEnabled { .. } = evt {
            ctx.notify();
        }

        if let AppEditorSettingsChangedEvent::CursorDisplayState { .. } = evt {
            ctx.notify();
        }

        // The vim status bar should be shown and hidden immediately upon toggling.
        if settings.as_ref(ctx).vim_mode_enabled()
            && let AppEditorSettingsChangedEvent::VimStatusBar { .. } = evt
        {
            ctx.notify();
        }
    }

    pub fn set_autosuggestion(
        &mut self,
        autosuggestion: impl Into<String>,
        autosuggestion_type: AutosuggestionType,
        ctx: &mut ViewContext<Self>,
    ) {
        self.editor.update(ctx, |editor, ctx| {
            editor.set_autosuggestion(
                autosuggestion,
                AutosuggestionLocation::EndOfBuffer,
                autosuggestion_type,
                ctx,
            );
        })
    }

    // Whether a workflow info box is open or not
    pub fn is_workflows_info_box_open(&self) -> bool {
        self.workflows_state.selected_workflow_state.is_some()
    }

    pub fn workflows_info_box_open_workflow_cloud_id(&self) -> Option<SyncId> {
        if let Some(state) = &self.workflows_state.selected_workflow_state {
            match &state.workflow_type {
                WorkflowType::Cloud(workflow) => Some(workflow.id),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn show_workflows_info_box_on_workflow_selection(
        &mut self,
        workflow_type: WorkflowType,
        workflow_source: WorkflowSource,
        workflow_selection_source: WorkflowSelectionSource,
        argument_override: Option<HashMap<String, String>>,
        ctx: &mut ViewContext<Input>,
    ) {
        // Should not show workflows info box for read-only viewers
        let should_show_more_info_view = !self.model.lock().shared_session_status().is_reader();
        let env_vars = workflow_type.as_workflow().default_env_vars();
        self.insert_workflow_into_input(
            workflow_type,
            workflow_source,
            workflow_selection_source,
            argument_override,
            env_vars,
            should_show_more_info_view,
            ctx,
        );
    }

    /// Inserts the given workflow into the input editor and initiates the shift-tab workflow
    /// parameter editing "mode".
    ///
    /// If `should_show_more_info_view`, the `WorkflowsMoreInfoView` for the selected workflow is
    /// displayed above the input.
    #[allow(clippy::too_many_arguments)]
    fn insert_workflow_into_input(
        &mut self,
        workflow_type: WorkflowType,
        workflow_source: WorkflowSource,
        workflow_selection_source: WorkflowSelectionSource,
        argument_overrides: Option<HashMap<String, String>>,
        selected_env_vars: Option<SyncId>,
        should_show_more_info_view: bool,
        ctx: &mut ViewContext<Input>,
    ) {
        let input_type = if workflow_type.as_workflow().is_agent_mode_workflow() {
            InputType::AI
        } else {
            InputType::Shell
        };

        // Set input type based on whether or not this is a shell or AI workflow.
        self.ai_input_model.update(ctx, |input_model, ctx| {
            input_model.set_input_type(
                input_type,
                Some(InputTypeAutoDetectionSource::WorkflowInsertion),
                ctx,
            );
        });

        // As the first step, clear the existing buffer so that selecting a workflow
        // is effectively a buffer replacement (not append).
        self.editor.update(ctx, |editor, ctx| {
            editor.clear_buffer(ctx);
        });

        if let Some(env_vars_command) = selected_env_vars
            .as_ref()
            .and_then(|id| self.env_vars_command_prefix(id, ctx))
        {
            self.editor.update(ctx, |editor, ctx| {
                editor.system_insert(
                    &env_vars_command,
                    PlainTextEditorViewAction::SystemInsert,
                    ctx,
                )
            });
        }

        let WorkflowDisplayData {
            command_with_replaced_arguments,
            replaced_ranges,
            argument_index_to_highlight_index_map,
            argument_index_to_object_id_map,
            ..
        } = if let Some(arguments_to_override) = argument_overrides {
            compute_workflow_display_data_with_overrides(
                workflow_type.as_workflow(),
                arguments_to_override,
            )
        } else {
            compute_workflow_display_data(workflow_type.as_workflow())
        };

        let text_style_ranges = replaced_ranges
            .into_iter()
            .map(|range| {
                (
                    range,
                    TextStyle::new().with_background_color(ColorU::from_u32(
                        WORKFLOW_PARAMETER_HIGHLIGHT_COLOR,
                    )),
                )
            })
            .collect_vec();

        self.editor.update(ctx, |editor, ctx| {
            editor.insert_with_styles(
                &command_with_replaced_arguments,
                &text_style_ranges,
                PlainTextEditorViewAction::SystemInsert,
                ctx,
            );
        });

        // Get enum variants
        let cloud_model = CloudModel::as_ref(ctx);
        let enum_variants_map = argument_index_to_object_id_map
            .iter()
            .filter_map(|(index, object_id)| {
                cloud_model
                    .get_workflow_enum(object_id)
                    .map(|workflow_enum| workflow_enum.model().string_model.variants.clone())
                    .map(|variants| (*index, variants))
            })
            .collect();

        self.workflows_state.selected_workflow_state = Some(SelectedWorkflowState {
            more_info_view: self.create_workflows_info_view(workflow_type.clone(), ctx),
            argument_index_to_highlight_index: argument_index_to_highlight_index_map,
            argument_index_to_enum_variants: enum_variants_map,
            workflow_source,
            workflow_type,
            workflow_selection_source,
            should_show_more_info_view,
        });

        self.env_var_collection_state.selected_env_vars = selected_env_vars;

        // Ensure the env var selector dropdown is consistent with the selected env vars.
        if let Some(more_info_view) = self
            .workflows_state
            .selected_workflow_state
            .as_ref()
            .map(|state| &state.more_info_view)
        {
            more_info_view.update(ctx, |info_view, ctx| {
                info_view.set_environment_variables_selection(selected_env_vars, ctx);
            })
        }

        // Emit the a11y content as the last step so that it overwrites any of the a11y content
        // emitted by the editor (if multiple `AccessibilityContent`s are emitted within the same
        // event loop, the last one wins).
        let mut accessibility_text =
            format!("Workflow command {command_with_replaced_arguments} inserted.");
        if let Some(a11y_content) = self.selected_workflow_a11y_text(ctx) {
            let _ = write!(accessibility_text, " {a11y_content}");
        }
        ctx.emit_a11y_content(AccessibilityContent::new(
            accessibility_text,
            "Press shift-tab to select the next workflow argument",
            WarpA11yRole::UserAction,
        ));

        // Only highlight an argument and show enum suggestions if history suggestions are not active
        if !matches!(
            self.suggestions_mode_model.as_ref(ctx).mode(),
            InputSuggestionsMode::HistoryUp { .. },
        ) {
            self.highlight_selected_workflow_argument(
                self.get_text_style_ranges_for_workflow(ctx),
                ctx,
            );
        }
        self.focus_input_box(ctx);
    }

    /// Builds a prefix for applying env vars to a command in the current session.
    fn env_vars_command_prefix(&self, env_vars_id: &SyncId, ctx: &AppContext) -> Option<String> {
        let shell_type = self.active_session(ctx)?.shell().shell_type();
        let env_vars = &CloudModel::as_ref(ctx)
            .get_env_var_collection(env_vars_id)?
            .model()
            .string_model;

        if shell_type == ShellType::Fish {
            // Warp currently doesn't support newlines in Fish, just prepend the vars
            let mut command = env_vars.export_variables_for_shell(ShellType::Fish);
            command.push(' ');
            Some(command)
        } else {
            // Add newlines at the end to separate the vars from the comment/command
            Some(format!(
                "# Environment variables\n{}\n\n",
                env_vars.export_variables(" ", shell_type.into())
            ))
        }
    }

    fn create_workflows_info_view(
        &mut self,
        workflow: WorkflowType,
        ctx: &mut ViewContext<Input>,
    ) -> ViewHandle<WorkflowsMoreInfoView> {
        let workflow_more_info_view = ctx.add_typed_action_view(|ctx| {
            WorkflowsMoreInfoView::new(
                *InputSettings::as_ref(ctx).workflows_box_expanded.value(),
                workflow,
                ctx,
            )
        });

        ctx.subscribe_to_view(&workflow_more_info_view, move |me, _, event, ctx| {
            me.handle_workflow_more_info_event(event, ctx);
        });

        workflow_more_info_view
    }

    fn handle_workflow_more_info_event(
        &mut self,
        event: &WorkflowsInfoBoxViewEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            WorkflowsInfoBoxViewEvent::PrefixCommandWithEnvironmentVariables(env_vars) => {
                self.reset_workflow_state(*env_vars, ctx);

                // The ID may be `None` if the user is *clearing* environment variables.
                if let Some(env_vars_id) = env_vars {
                    let env_vars_object =
                        CloudModel::as_ref(ctx).get_env_var_collection(env_vars_id);
                    let telemetry_metadata = EnvVarTelemetryMetadata {
                        object_id: env_vars_id.into_server().map(Into::into),
                        team_uid: env_vars_object
                            .and_then(|object| object.permissions.owner.into()),
                        space: env_vars_object
                            .map_or(Space::Personal, |object| object.space(ctx))
                            .into(),
                    };
                    send_telemetry_from_ctx!(
                        TelemetryEvent::EnvVarWorkflowParameterization(telemetry_metadata),
                        ctx
                    );
                }
            }
        }
    }

    /// Returns the a11y text for a workflow that is selected. `None`, if there is no workflow
    /// selected.
    fn selected_workflow_a11y_text(&self, ctx: &mut ViewContext<Self>) -> Option<String> {
        self.workflows_state
            .selected_workflow_state
            .as_ref()
            .and_then(|selected_workflow_state| {
                selected_workflow_state.more_info_view.read(ctx, |view, _| {
                    view.selected_argument()
                        .map(|argument| format!("Selected Workflow argument {}", argument.name()))
                })
            })
    }

    fn workflow_arg_was_deleted(
        &self,
        text_style_run_count: usize,
        argument_index_to_highlight_index: &HashMap<WorkflowArgumentIndex, Vec<usize>>,
    ) -> bool {
        let expected_run_count: usize = argument_index_to_highlight_index
            .values()
            .map(|indices| indices.len())
            .sum();
        text_style_run_count != expected_run_count
    }

    fn get_text_style_ranges_for_workflow(
        &self,
        ctx: &ViewContext<Self>,
    ) -> Vec<Range<ByteOffset>> {
        let text_style_runs: Vec<_> = self
            .editor
            .as_ref(ctx)
            .text_style_runs(ctx)
            .filter(|style_run| style_run.text_style().background_color.is_some())
            .collect();
        self.build_text_run_ranges_for_workflows(&text_style_runs)
    }

    /// We are currently using the styling of text runs in the input as a way of tracking
    /// where our workflow arguments are.
    /// This doesn't work in 2 cases:
    ///
    /// 1. When part of a workflow argument is subject to syntax highlighting, it breaks
    ///    a run into one or more runs. Example: "--env JOB_EXECUTION_MODE=REAL" will wind
    ///    up with syntax highlighting on `--env`, resulting in 2 runs.
    /// 2. When workflow arguments directly follow each other with no spacing, they will
    ///    both be covered by a single run.. Example: {a}{b}{c} will only get a single
    ///    run covering "abc"
    ///
    /// This helper acts as a quick hack to address the first issue:
    /// if two background-highlighted runs are contiguous, they are merged into a single run.
    /// This is a short-term fix and should be addressed in a more comprehensive way that does
    /// not rely on the styling of the input.
    ///
    /// See [CLD-997](https://linear.app/warpdotdev/issue/CLD-997)
    fn build_text_run_ranges_for_workflows(
        &self,
        text_style_runs: &[TextRun],
    ) -> Vec<Range<ByteOffset>> {
        let mut ranges = text_style_runs
            .iter()
            .map(|style_run| style_run.byte_range().clone())
            .collect::<Vec<_>>();
        ranges.sort_by(|a, b| a.start.cmp(&b.start));

        let capacity = ranges.len();

        ranges.into_iter().fold(
            Vec::<Range<ByteOffset>>::with_capacity(capacity),
            |mut acc: Vec<Range<ByteOffset>>, next| -> Vec<Range<ByteOffset>> {
                match acc.last() {
                    Some(current) if current.end >= next.start => {
                        let new_range = std::cmp::min(current.start, next.start)
                            ..std::cmp::max(current.end, next.end);
                        acc.pop();
                        acc.push(new_range);
                    }
                    _ => {
                        acc.push(next);
                    }
                };
                acc
            },
        )
    }

    /// Highlight the currently selected workflow argument and open the enum suggestions menu if applicable.
    /// Takes in `text_style_ranges`, which contains ByteOffset Ranges of arguments in the input editor.
    fn highlight_selected_workflow_argument(
        &mut self,
        text_style_ranges: Vec<Range<ByteOffset>>,
        ctx: &mut ViewContext<Self>,
    ) {
        let mut variants = None;
        let mut selected_ranges = Vec::new();

        if let Some(active_workflow_state) = self.workflows_state.selected_workflow_state.as_ref() {
            active_workflow_state
                .more_info_view
                .update(ctx, |workflows_info_view, ctx| {
                    let selected_workflow_state = &mut workflows_info_view.selected_workflow_state;
                    // Update the editor given what the currently selected argument index is
                    self.editor.update(ctx, |editor, ctx| {
                        // If an argument has been completely deleted - pause the shift-tab cycling
                        if self.workflow_arg_was_deleted(
                            text_style_ranges.len(),
                            &active_workflow_state.argument_index_to_highlight_index,
                        ) {
                            selected_workflow_state.set_argument_cycling_enabled(false);
                        } else {
                            variants = active_workflow_state
                                .argument_index_to_enum_variants
                                .get(&selected_workflow_state.currently_selected_argument());

                            selected_workflow_state.set_argument_cycling_enabled(true);
                            // Get all of the highlighted ranges for the currently selected argument.
                            let byte_ranges = active_workflow_state
                                .argument_index_to_highlight_index
                                .get(&selected_workflow_state.currently_selected_argument())
                                .map(|indices| {
                                    indices
                                        .iter()
                                        .filter_map(|index| text_style_ranges.get(*index).cloned())
                                });

                            if let Some(byte_ranges) = byte_ranges {
                                selected_ranges = byte_ranges.clone().collect();
                                editor.select_ranges_by_byte_offset(byte_ranges, ctx);
                            }
                        }
                    });
                });
        }

        if let Some(enum_variants) = variants {
            self.populate_enum_suggestions_menu(enum_variants.clone(), selected_ranges, ctx);
        } else {
            self.suggestions_mode_model.update(ctx, |m, ctx| {
                m.set_mode(InputSuggestionsMode::Closed, ctx);
            });
        }
        ctx.notify();
    }

    fn populate_enum_suggestions_menu(
        &mut self,
        enum_variants: EnumVariants,
        selected_ranges: Vec<Range<ByteOffset>>,
        ctx: &mut ViewContext<Self>,
    ) {
        // If the newly highlighted argument has enum variants, populate the suggestions menu
        let position = self.editor.as_ref(ctx).first_selection_end_to_point(ctx);

        self.editor.update(ctx, |editor, ctx| {
            editor.cache_buffer_point(
                position,
                COMPLETIONS_START_OF_REPLACEMENT_SPAN_POSITION_ID,
                ctx,
            );
        });

        let variants = match enum_variants {
            EnumVariants::Static(variants) => {
                self.suggestions_mode_model.update(ctx, |m, ctx| {
                    m.set_mode(
                        InputSuggestionsMode::StaticWorkflowEnumSuggestions {
                            suggestions: variants.clone(),
                            menu_position: TabCompletionsMenuPosition::AtFirstCursor,
                            selected_ranges,
                            cursor_point: position,
                        },
                        ctx,
                    );
                });
                variants
            }
            EnumVariants::Dynamic(command) => {
                if FeatureFlag::DynamicWorkflowEnums.is_enabled() {
                    self.suggestions_mode_model.update(ctx, |m, ctx| {
                        m.set_mode(
                            InputSuggestionsMode::DynamicWorkflowEnumSuggestions {
                                suggestions: vec![],
                                menu_position: TabCompletionsMenuPosition::AtFirstCursor,
                                selected_ranges,
                                cursor_point: position,
                                dynamic_enum_status: DynamicEnumSuggestionStatus::Unapproved,
                                command,
                            },
                            ctx,
                        );
                    });
                }
                vec![]
            }
        };

        self.input_suggestions.update(ctx, |input, ctx| {
            input.set_enum_variants(variants, ctx);
        });

        ctx.notify();
    }

    fn handle_suggestions_event(
        &mut self,
        event: &InputSuggestionsEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.suggestions_mode_model.as_ref(ctx).is_visible() {
            return;
        }

        match event {
            InputSuggestionsEvent::ConfirmSuggestion {
                suggestion,
                match_type,
            } => {
                if !self.confirm_suggestion(suggestion, ctx) {
                    return;
                }

                send_telemetry_from_ctx!(
                    TelemetryEvent::ConfirmSuggestion {
                        mode: self
                            .suggestions_mode_model
                            .as_ref(ctx)
                            .mode()
                            .to_telemetry_mode(),
                        match_type: *match_type,
                    },
                    ctx
                );
                self.close_input_suggestions(/*should_focus_input=*/ true, ctx);
            }
            InputSuggestionsEvent::ConfirmAndExecuteSuggestion {
                suggestion,
                match_type,
            } => {
                if !self.confirm_and_execute_suggestion(suggestion, ctx) {
                    return;
                }

                send_telemetry_from_ctx!(
                    TelemetryEvent::ConfirmSuggestion {
                        mode: self
                            .suggestions_mode_model
                            .as_ref(ctx)
                            .mode()
                            .to_telemetry_mode(),
                        match_type: *match_type,
                    },
                    ctx
                );

                self.close_input_suggestions(/*should_focus_input=*/ true, ctx);

                let command = self.get_command(ctx);
                self.try_execute_command(&command, ctx);

                ctx.emit_a11y_content(AccessibilityContent::new_without_help(
                    format!("Executed: {command}"),
                    WarpA11yRole::UserAction,
                ));
            }
            InputSuggestionsEvent::CloseSuggestion {
                should_restore_buffer_before_history_up,
            } => {
                self.close_input_suggestions_and_restore_buffer(
                    true,
                    *should_restore_buffer_before_history_up,
                    ctx,
                );
            }
            InputSuggestionsEvent::Select(selected_item) => {
                let mode = self.suggestions_mode_model.as_ref(ctx).mode().clone();
                match &mode {
                    InputSuggestionsMode::HistoryUp { .. } => {
                        self.editor.update(ctx, |editor, ctx| {
                            editor.set_buffer_text_ignoring_undo(selected_item.text(), ctx);
                        });

                        self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                            ai_input_model.set_input_type(
                                InputType::Shell,
                                Some(InputTypeAutoDetectionSource::HistorySelection),
                                ctx,
                            );
                        });
                    }
                    InputSuggestionsMode::CompletionSuggestions {
                        replacement_start, ..
                    } => {
                        let replacement_start = *replacement_start;
                        if self.is_classic_completions_enabled(ctx) {
                            self.editor.update(ctx, |editor, ctx| {
                                let cursor_end_offset =
                                    editor.end_byte_index_of_last_selection(ctx);
                                editor.select_and_replace(
                                    selected_item.text(),
                                    [ByteOffset::from(replacement_start)..cursor_end_offset],
                                    PlainTextEditorViewAction::CycleCompletionSuggestion,
                                    ctx,
                                );
                                ctx.notify();
                            });
                            ctx.notify();
                        }
                    }
                    InputSuggestionsMode::StaticWorkflowEnumSuggestions { .. }
                    | InputSuggestionsMode::DynamicWorkflowEnumSuggestions { .. } => {
                        // If in the future we want to replace the selected arguments with suggestion options as we cycle, this is where we do it
                    }
                    InputSuggestionsMode::Closed => {
                        log::warn!("Got a InputSuggestionsEvent::Select when the mode was Closed!");
                    }
                }
            }
            InputSuggestionsEvent::IgnoreItem { item } => {
                let command_text = item.text();

                IgnoredSuggestionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.add_ignored_suggestion(
                        command_text.to_string(),
                        SuggestionType::ShellCommand,
                        ctx,
                    );
                });

                // Refresh the history suggestions menu and keep it open
                if matches!(
                    self.suggestions_mode_model.as_ref(ctx).mode(),
                    InputSuggestionsMode::HistoryUp { .. }
                ) {
                    let history = if self.model.lock().shared_session_status().is_executor() {
                        self.shared_session_history(ctx)
                    } else {
                        self.command_history_suggestions(ctx)
                    };
                    let original_buffer = if let InputSuggestionsMode::HistoryUp {
                        original_buffer,
                        ..
                    } = self.suggestions_mode_model.as_ref(ctx).mode()
                    {
                        original_buffer.clone()
                    } else {
                        String::new()
                    };

                    let matches =
                        InputSuggestions::history_prefix_search(&original_buffer, history);
                    self.input_suggestions
                        .update(ctx, move |input_suggestions, ctx| {
                            input_suggestions.set_history_matches(matches, ctx);
                        });
                }
            }
        }
    }

    /// Resets the SelectedWorkflowState back to the original workflow, with its original arguments. This
    /// is useful when the command does not match the original workflow.
    fn reset_workflow_state(&mut self, env_vars: Option<SyncId>, ctx: &mut ViewContext<Input>) {
        // We want to also initially clear the stored selected env var.
        self.clear_selected_env_var_collection();

        if let Some(state) = self.workflows_state.selected_workflow_state.take() {
            self.insert_workflow_into_input(
                state.workflow_type,
                state.workflow_source,
                state.workflow_selection_source,
                None,
                env_vars,
                true,
                ctx,
            )
        }

        ctx.notify();
    }

    fn confirm_suggestion(&mut self, suggestion: &str, ctx: &mut ViewContext<Input>) -> bool {
        self.confirm_suggestion_internal(suggestion, Executing::No, ctx)
    }

    fn confirm_and_execute_suggestion(
        &mut self,
        suggestion: &str,
        ctx: &mut ViewContext<Input>,
    ) -> bool {
        self.confirm_suggestion_internal(suggestion, Executing::Yes, ctx)
    }

    /// Handles suggestion confirmation behaviour in editor and returns true if suggestions menu should be closed
    /// For CompletionSuggestions, inserts suggestion into editor. For HistoryUp, no action since "select" populates buffer.
    /// Closed branch should never be executed (does not use the input suggestions panel).
    fn confirm_suggestion_internal(
        &mut self,
        suggestion: &str,
        executing: Executing,
        ctx: &mut ViewContext<Input>,
    ) -> bool {
        match self.suggestions_mode_model.as_ref(ctx).mode() {
            InputSuggestionsMode::Closed => false,
            InputSuggestionsMode::HistoryUp { .. } => true,
            InputSuggestionsMode::CompletionSuggestions {
                replacement_start, ..
            } => {
                self.insert_completion_result_into_editor(
                    suggestion,
                    *replacement_start,
                    executing,
                    ctx,
                );
                true
            }
            InputSuggestionsMode::StaticWorkflowEnumSuggestions {
                selected_ranges, ..
            }
            | InputSuggestionsMode::DynamicWorkflowEnumSuggestions {
                selected_ranges, ..
            } => {
                let selected_ranges = selected_ranges.clone();
                self.editor.update(ctx, |editor, ctx| {
                    editor.select_and_replace(
                        suggestion,
                        selected_ranges.iter().cloned(),
                        PlainTextEditorViewAction::AcceptCompletionSuggestion,
                        ctx,
                    );
                });
                true
            }
        }
    }

    pub fn close_input_suggestions_and_restore_buffer(
        &mut self,
        should_focus_input: bool,
        should_restore_buffer_before_history_up: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if should_restore_buffer_before_history_up
            && let InputSuggestionsMode::HistoryUp {
                original_buffer,
                original_cursor_point,
                original_input_was_locked,
                original_input_type,
                ..
            } = self.suggestions_mode_model.as_ref(ctx).mode()
        {
            let original_buffer = original_buffer.clone();
            let original_cursor_point = *original_cursor_point;
            let original_input_was_locked = *original_input_was_locked;
            let original_input_type = *original_input_type;
            // If the user closes the input suggestions menu, we want to reset the AI input mode
            // to the exact same state it was originally, which includes the mode itself and
            // whether it was locked to that mode.
            self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                ai_input_model.set_input_config(
                    InputConfig {
                        input_type: original_input_type,
                        is_locked: original_input_was_locked,
                    },
                    original_buffer.is_empty(),
                    Some(InputTypeAutoDetectionSource::RestoreSavedConfig),
                    ctx,
                );
            });

            self.editor.update(ctx, |editor, ctx| {
                editor.set_buffer_text_ignoring_undo(&original_buffer, ctx);
                if let Some(original_cursor_point) = original_cursor_point {
                    editor.reset_selections_to_point(&original_cursor_point, ctx);
                }
            });
        }
        self.close_input_suggestions(/*should_focus_input=*/ should_focus_input, ctx);
    }

    pub fn close_input_suggestions(
        &mut self,
        should_focus_input: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        // If the input suggestions view is already closed, don't refocus the input box.
        if !self.suggestions_mode_model.as_ref(ctx).is_closed() {
            self.suggestions_mode_model.update(ctx, |m, ctx| {
                m.set_mode(InputSuggestionsMode::Closed, ctx);
            });

            if should_focus_input {
                self.focus_input_box(ctx);
                self.maybe_generate_autosuggestion(ctx);
            } else {
                ctx.notify();
            }
        }
    }

    pub fn clear_buffer_and_reset_undo_stack(&mut self, ctx: &mut ViewContext<Self>) {
        self.clear_cached_hint_text();
        self.exit_cloud_handoff_compose(ctx);
        self.editor.update(ctx, |view, ctx| {
            view.clear_buffer_and_reset_undo_stack(ctx);
        });
    }

    pub fn replace_buffer_content(&mut self, content: &str, ctx: &mut ViewContext<Self>) {
        self.editor.update(ctx, |view, ctx| {
            view.set_buffer_text(content, ctx);
        });
    }

    // Fill the input buffer with the provided text and auto-select all of the text
    // (so that it's easy to delete).
    pub fn prefill_buffer_and_select_all(&mut self, content: &str, ctx: &mut ViewContext<Self>) {
        let content = content.trim();
        if content.is_empty() {
            return;
        }

        self.editor.update(ctx, |editor, ctx| {
            editor.clear_autosuggestion(ctx);
            editor.set_buffer_text_ignoring_undo(content, ctx);
            editor.handle_action(&EditorAction::SelectAll, ctx);
        });
    }

    /// Appends text to the current buffer at the cursor position, preserving existing buffer content.
    pub fn append_to_buffer(&mut self, content: &str, ctx: &mut ViewContext<Self>) {
        self.system_insert(content, ctx);
    }

    pub fn insert_typeahead_text(
        &mut self,
        num_typeahead_chars_inserted: CharOffset,
        typeahead: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        self.editor.update(ctx, |view, ctx| {
            view.replace_first_n_characters(num_typeahead_chars_inserted, typeahead, ctx);
            view.move_to_buffer_end(ctx);
        });
    }

    pub fn focus_input_box(&self, ctx: &mut ViewContext<Self>) {
        if self.should_show_auth_secret_ftux(ctx)
            && let Some(ftux_view) = self.auth_secret_ftux_view().cloned()
        {
            ftux_view.update(ctx, |view, ctx| {
                view.focus_dropdown_editor(ctx);
            });
            return;
        }
        ctx.focus_self();
    }

    pub fn input_type(&self, app: &AppContext) -> InputType {
        self.ai_input_model.as_ref(app).input_type()
    }

    /// Close all overlays managed by the input view. Does not change what is focused.
    /// If should_restore_buffer_before_history_up is true, the buffer will be restored to the state it was in before the history up menu was opened.
    pub fn close_overlays(
        &mut self,
        should_restore_buffer_before_history_up: bool,
        ctx: &mut ViewContext<Input>,
    ) {
        self.close_input_suggestions_and_restore_buffer(
            false,
            should_restore_buffer_before_history_up,
            ctx,
        );
        self.clear_selected_workflow(ctx);
    }

    /// Closes any active suggestion mode UI when starting a new conversation.
    ///
    /// This leaves workflow info overlays open.
    fn close_suggestion_modes_for_new_conversation(&mut self, ctx: &mut ViewContext<Self>) {
        self.suggestions_mode_model.update(ctx, |model, ctx| {
            model.set_mode(InputSuggestionsMode::Closed, ctx);
        });
    }

    fn editor_up(&mut self, ctx: &mut ViewContext<Self>) {
        if self.should_show_auth_secret_ftux(ctx) {
            if let Some(ftux_view) = self.auth_secret_ftux_view().cloned() {
                ftux_view.update(ctx, |view, ctx| {
                    view.select_previous_in_dropdown(ctx);
                });
            }
            return;
        }

        if let Some(selector) = self.auth_secret_selector()
            && selector.as_ref(ctx).is_menu_open()
        {
            let selector = selector.clone();
            selector.update(ctx, |selector, ctx| {
                selector.select_previous(ctx);
            });
            return;
        }

        if self.is_editing_queued_prompt(ctx) {
            return;
        }

        // History and input suggestions are not available for
        // read-only viewers in a shared session
        if self.model.lock().shared_session_status().is_reader() {
            return;
        }

        // If the input suggestions menu is open, always cycle to the next option.
        if self.suggestions_mode_model.as_ref(ctx).is_visible() && self.can_query_history(ctx) {
            self.input_suggestions.update(ctx, |suggestions, ctx| {
                suggestions.select_prev(ctx);
            });
            return;
        }

        // Otherwise, check if the cursor is on the first row and open the
        // history up menu.
        let editor = self.editor.as_ref(ctx);
        if editor.single_cursor_on_first_row(ctx) {
            let history = if self.model.lock().shared_session_status().is_executor() {
                self.shared_session_history(ctx)
            } else {
                self.command_history_suggestions(ctx)
            };
            let original_buffer = self.editor.as_ref(ctx).buffer_text(ctx);

            let matches = InputSuggestions::history_prefix_search(&original_buffer, history);
            self.input_suggestions
                .update(ctx, move |input_suggestions, ctx| {
                    input_suggestions.set_history_matches(matches, ctx);
                });

            let original_cursor_point = self.editor.as_ref(ctx).single_cursor_to_point(ctx);
            let original_input_type = self.ai_input_model.as_ref(ctx).input_type();
            let original_input_was_locked = self.ai_input_model.as_ref(ctx).is_input_type_locked();
            self.suggestions_mode_model.update(ctx, |m, ctx| {
                m.set_mode(
                    InputSuggestionsMode::HistoryUp {
                        original_buffer,
                        original_cursor_point,
                        search_mode: HistorySearchMode::Prefix,
                        original_input_type,
                        original_input_was_locked,
                    },
                    ctx,
                );
            });

            ctx.notify();
            return;
        }
        // Finally, if we're neither scrolling through an existing suggestion
        // list nor entering the history mode, we move the cursor up.
        self.editor.update(ctx, |input, ctx| input.move_up(ctx));
    }

    // TODO - Implement PageUp functionality for input suggestions menu
    fn editor_page_up(&mut self, ctx: &mut ViewContext<Self>) {
        let event = self.editor.read(ctx, |editor, ctx| {
            TelemetryEvent::PageUpDownInEditorPressed {
                is_empty_editor: editor.is_empty(ctx),
                is_down: false,
            }
        });
        send_telemetry_from_ctx!(event, ctx);
        if self.suggestions_mode_model.as_ref(ctx).is_visible() {
            self.editor
                .update(ctx, |input, ctx| input.move_page_up(ctx));
        } else {
            ctx.emit(Event::PageUp);
        }
    }

    fn editor_escape(&mut self, ctx: &mut ViewContext<Self>) {
        let vim_mode = self.editor.as_ref(ctx).vim_mode(ctx);
        let has_attached_context = {
            let context_model = self.ai_context_model.as_ref(ctx);
            !context_model.pending_context_block_ids().is_empty()
                || context_model.pending_context_selected_text().is_some()
        };
        let should_escape_vim_before_dismissing = (vim_mode == Some(VimMode::Insert)
            && self.suggestions_mode_model.as_ref(ctx).is_history_up())
            || (vim_mode == Some(VimMode::Insert)
                && self.prefix_mode(ctx) == InputPrefixMode::CloudHandoff);

        if should_escape_vim_before_dismissing {
            self.editor.update(ctx, |editor, editor_ctx| {
                editor.handle_action(&EditorAction::VimEscape, editor_ctx);
            });
        } else if self.prefix_mode(ctx) == InputPrefixMode::CloudHandoff {
            self.exit_cloud_handoff_compose(ctx);
            ctx.notify();
        } else if self.suggestions_mode_model.as_ref(ctx).is_visible() {
            self.input_suggestions
                .update(ctx, |input_suggestions, ctx| {
                    input_suggestions.exit(true, ctx);
                });
        } else if self.workflows_state.selected_workflow_state.is_some() {
            self.clear_current_workflow(ctx);
        } else if !matches!(vim_mode, None | Some(VimMode::Normal)) {
            self.editor.update(ctx, |editor, editor_ctx| {
                editor.handle_action(&EditorAction::VimEscape, editor_ctx);
            });
        } else if FeatureFlag::AgentView.is_enabled()
            && self.agent_view_controller.as_ref(ctx).is_active()
            && has_attached_context
        {
            self.clear_attached_context(ctx);
        } else {
            if FeatureFlag::AgentView.is_enabled()
                && !self.agent_view_controller.as_ref(ctx).is_fullscreen()
            {
                if self.ai_input_model.as_ref(ctx).is_ai_input_enabled() {
                    // This implies the contents of the terminal input are autodetected as an agent
                    // prompt; overrides the autodetection by explicitly setting input mode back to
                    // terminal.
                    self.set_input_mode_terminal(false, ctx);
                }
            } else {
                self.set_input_mode_natural_language_detection(ctx);
            }
            ctx.emit(Event::Escape);
        }
    }

    /// Emits an `AgentModeAutodetectionFalsePositive` telemetry event if the current input text has
    /// been autodetected as AI input and the user manually toggled to shell.
    /// Also emits `AgentModeChangedInputType` if the user is part of the analytics experiment.
    ///
    /// This is intended to be called whenever the user manually toggles the input to new_input_type. Because the user is manually toggling
    /// back to shell mode after input has been autodetected as natural language, we infer that the
    /// current input text may not have been correctly classified as natural language.
    /// For users opted in to the analytics experiment, we collect the input buffer text whenever the input type is toggled
    /// in either direction.
    fn maybe_send_autodetection_telemetry_on_manual_toggle(
        &self,
        new_input_type: InputType,
        ctx: &mut ViewContext<Self>,
    ) {
        let input_buffer_text = self.buffer_text(ctx);
        let buffer_length = input_buffer_text.len();
        let input =
            should_collect_ai_ugc_telemetry(ctx, PrivacySettings::as_ref(ctx).is_telemetry_enabled)
                .then_some(input_buffer_text);
        let is_udi_enabled = InputSettings::as_ref(ctx).is_universal_developer_input_enabled(ctx);
        send_telemetry_from_ctx!(
            TelemetryEvent::AgentModeChangedInputType {
                input,
                buffer_length,
                is_manually_changed: true,
                new_input_type,
                active_block_id: self.model.lock().block_list().active_block_id().clone(),
                is_udi_enabled,
            },
            ctx
        );

        let ai_input_model = self.ai_input_model.as_ref(ctx);
        if matches!(new_input_type, InputType::Shell) && !ai_input_model.is_input_type_locked() {
            let current_input_text = self.buffer_text(ctx);
            if !current_input_text.is_empty() {
                let event_payload = if ChannelState::channel().is_dogfood() {
                    AgentModeAutoDetectionFalsePositivePayload::InternalDogfoodUsers {
                        input_text: current_input_text,
                    }
                } else {
                    AgentModeAutoDetectionFalsePositivePayload::ExternalUsers
                };
                send_telemetry_from_ctx!(
                    TelemetryEvent::AgentModePotentialAutoDetectionFalsePositive(event_payload),
                    ctx
                );
            }
        }
    }

    /// Takes the current collpased/expanded state of the info box and saves it to the user's settings so that last value can be
    /// reused the next time the user opens a workflow.
    fn update_workflows_info_box_expanded_setting(
        &mut self,
        ctx: &mut ViewContext<Self>,
        selected_workflow_state: &SelectedWorkflowState,
    ) {
        let info_box_expanded = selected_workflow_state
            .more_info_view
            .as_ref(ctx)
            .info_box_expanded;

        InputSettings::handle(ctx).update(ctx, |input_settings, ctx| {
            report_if_error!(
                input_settings
                    .workflows_box_expanded
                    .set_value(info_box_expanded, ctx)
            );
        });
    }

    fn clear_current_workflow(&mut self, ctx: &mut ViewContext<Input>) {
        // Whenever we clear the workflow we also want to clear the env vars
        self.clear_selected_env_var_collection();

        if let Some(state) = self.workflows_state.selected_workflow_state.take() {
            self.update_workflows_info_box_expanded_setting(ctx, &state);
        }
        self.editor
            .update(ctx, |editor, ctx| editor.clear_text_style_runs(ctx));
        ctx.notify();
    }

    fn editor_down(&mut self, ctx: &mut ViewContext<Self>) {
        if self.suggestions_mode_model.as_ref(ctx).is_visible() {
            if self.input_suggestions.as_ref(ctx).is_empty() {
                // arrow down on an empty suggestions means we should close it.
                self.close_input_suggestions_and_restore_buffer(true, true, ctx);
            } else {
                self.input_suggestions.update(ctx, |suggestions, ctx| {
                    suggestions.select_next(ctx);
                });
            }
        } else {
            self.editor.update(ctx, |editor, ctx| editor.move_down(ctx));
        }
    }

    // TODO - Implement PageDown functionality for input suggestions menu
    fn editor_page_down(&mut self, ctx: &mut ViewContext<Self>) {
        let event = self.editor.read(ctx, |editor, ctx| {
            TelemetryEvent::PageUpDownInEditorPressed {
                is_empty_editor: editor.is_empty(ctx),
                is_down: true,
            }
        });
        send_telemetry_from_ctx!(event, ctx);
        if self.suggestions_mode_model.as_ref(ctx).is_visible() {
            self.editor
                .update(ctx, |input, ctx| input.move_page_down(ctx));
        } else {
            ctx.emit(Event::PageDown);
        }
    }

    fn maybe_generate_autosuggestion(&mut self, ctx: &mut ViewContext<Self>) {
        let editor = self.editor.as_ref(ctx);

        let should_generate_autosuggestion = !editor.active_autosuggestion()
            && self.enable_autosuggestions_setting
            && !self.ai_input_model.as_ref(ctx).is_ai_input_enabled();

        if should_generate_autosuggestion {
            let buffer_text = editor.buffer_text(ctx);
            self.generate_autosuggestion_async(buffer_text, self.completer_data(), ctx)
        }
    }

    /// Asynchronously generate an autosuggestion to be inserted into the editor. First, reverse
    /// search the user's history to find a possible command that starts with the buffer text. If
    /// no commands are found, run the completer in a background thread to generate a result.
    pub fn generate_autosuggestion_async(
        &mut self,
        buffer_text: String,
        completer_data: CompleterData,
        ctx: &mut ViewContext<Self>,
    ) {
        if buffer_text.is_empty() {
            return;
        }

        let Some(session_id) = completer_data.active_block_session_id() else {
            return;
        };
        self.abort_latest_autosuggestion_future();

        let completion_context = completer_data.completion_session_context(ctx);
        let completion_session = completion_context
            .as_ref()
            .map(|completion_context| completion_context.session.clone());

        let reverse_chronological_potential_autosuggestions = History::as_ref(ctx)
            .commands(session_id)
            .map(|history_entries| {
                let working_dir = completer_data
                    .active_block_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.current_working_directory());
                find_potential_autosuggestions_from_history(
                    history_entries.into_iter(),
                    &buffer_text,
                    working_dir,
                )
            });

        let session_env_vars = self.sessions.read(ctx, |sessions, _| {
            sessions.get_env_vars_for_session(session_id)
        });
        // Get current ignored shell commands to filter during generation
        let ignored_suggestions = IgnoredSuggestionsModel::as_ref(ctx)
            .get_ignored_suggestions_for_type(SuggestionType::ShellCommand);
        #[cfg(feature = "local_fs")]
        let conn = self.conn.clone();
        // Resolve the last completed block's lazily-computed fields now, synchronously, since the
        // spawned future below doesn't have access to the terminal model to resolve them later.
        #[cfg(feature = "local_fs")]
        let last_user_block_completed_data =
            completer_data
                .last_user_block_completed
                .as_ref()
                .map(|block| {
                    (
                        block
                            .command
                            .get_with(|compute| {
                                let model = self.model.lock();
                                compute(model.block_list())
                            })
                            .to_owned(),
                        block
                            .serialized_block
                            .get_with(|compute| {
                                let model = self.model.lock();
                                compute(model.block_list())
                            })
                            .clone(),
                    )
                });
        let abort_handle = ctx
            .spawn_abortable(
                async move {
                    #[cfg(feature = "local_fs")]
                    // First, use rich history to find commands with a matching prefix that were run
                    // in a similar context, taking into account the most recent block run.
                    if let Some(conn) = conn
                        && let Some((last_command, last_serialized_block)) =
                            &last_user_block_completed_data
                    {
                        let similar_history_commands = {
                            let mut conn = conn.lock();
                            find_next_commands_from_similar_history(
                                &mut conn,
                                last_command,
                                &last_serialized_block.pwd,
                                last_serialized_block.exit_code,
                                last_serialized_block.shell_host.as_ref(),
                            )
                        };
                        if !similar_history_commands.is_empty() {
                            let mut history_next_command_counts = counter::Counter::<String>::new();
                            // Find the most likely next command after a similar context, out of those that have a matching prefix and aren't ignored.
                            for history_command in &similar_history_commands {
                                if history_command.command.starts_with(&buffer_text)
                                    && !ignored_suggestions.contains(&history_command.command)
                                {
                                    history_next_command_counts[&history_command.command] += 1;
                                }
                            }

                            for (most_likely_next_command, _) in
                                history_next_command_counts.k_most_common_ordered(5)
                            {
                                if is_command_valid(
                                    &most_likely_next_command,
                                    completion_context.as_ref(),
                                    session_env_vars.as_ref(),
                                )
                                .await
                                {
                                    return AutoSuggestionResult {
                                        buffer_text,
                                        autosuggestion_result: Some(
                                            most_likely_next_command.clone(),
                                        ),
                                    };
                                }
                            }
                        }
                    }

                    // If we have no suggestion from similar historical context, fallback to the most recent
                    // command with a matching prefix run in the same pwd (if exists, otherwise just most recent command anywhere with matching prefix).
                    for reverse_chronological_command in
                        reverse_chronological_potential_autosuggestions.unwrap_or_default()
                    {
                        if !ignored_suggestions.contains(&reverse_chronological_command.command)
                            && is_command_valid(
                                &reverse_chronological_command.command,
                                completion_context.as_ref(),
                                session_env_vars.as_ref(),
                            )
                            .await
                        {
                            return AutoSuggestionResult {
                                buffer_text,
                                autosuggestion_result: Some(reverse_chronological_command.command),
                            };
                        }
                    }

                    // If we have no command anywhere in history with a matching prefix, fallback to the first completer result.
                    let Some(completion_context) = completion_context else {
                        return AutoSuggestionResult {
                            buffer_text,
                            autosuggestion_result: None,
                        };
                    };
                    let completion_result = completer::suggestions(
                        buffer_text.as_str(),
                        buffer_text.len(),
                        session_env_vars.as_ref(),
                        CompleterOptions {
                            match_strategy: MatchStrategy::CaseSensitive,
                            fallback_strategy: CompletionsFallbackStrategy::FilePaths,
                            suggest_file_path_completions_only: false,
                            parse_quotes_as_literals: false,
                        },
                        &completion_context,
                    )
                    .await;

                    let autosuggestion = completion_result.and_then(|result| {
                        let replacement_span = result.replacement_span;
                        result
                            .suggestions
                            .into_iter()
                            .map(|s| {
                                // Reproduce the final buffer text with the autosuggestion since the
                                // completer only gives the replacement span of the suggestion.
                                format!(
                                    "{}{}",
                                    &buffer_text[..replacement_span.start()],
                                    s.replacement()
                                )
                            })
                            .find(|suggestion| !ignored_suggestions.contains(suggestion))
                    });

                    AutoSuggestionResult {
                        buffer_text,
                        autosuggestion_result: autosuggestion,
                    }
                },
                Self::on_autosuggestion_result,
                move |_, _| {
                    if let Some(session) = completion_session {
                        session.cancel_active_commands();
                    }
                },
            )
            .abort_handle();

        self.set_autosuggestion_future(abort_handle);
    }

    fn is_potential_expansion(
        token: &Spanned<String>,
        cursor_pos: usize,
        executing: Executing,
    ) -> bool {
        match executing {
            // Expansion was triggered by user entering the command to be executed.
            // To expand, cursor must be exactly at the end of the token.
            Executing::Yes => token.span().end() == cursor_pos,
            // Expansion was triggered by user pressing Space at the end of a token.
            // To expand, cursor must be one index after the end of the token.
            Executing::No => token.span().end() + 1 == cursor_pos,
        }
    }

    /// Gets the abbreviation and abbreviation value, or alias and alias value, given
    /// a command, if they exist. Will return None if the conditions for alias
    /// expansion are not met.
    fn get_valid_abbreviation_or_alias_for_expansion<'a>(
        &self,
        command: Option<&'a LiteCommand>,
        cursor_pos: usize,
        executing: Executing,
        session_context: &'a SessionContext,
        ctx: &mut ViewContext<Self>,
    ) -> Option<(&'a Spanned<String>, &'a str)> {
        // An alias must be the first token of a command
        let first_token = command?.parts.first()?;

        if !Self::is_potential_expansion(first_token, cursor_pos, executing) {
            return None;
        }

        // If there is an abbreviation, we expand it as long as we aren't executing.
        // In fish, an alias formatted like `ls=echo Hello && ls` would get expanded
        // twice if we also performed expansion on enter.
        if matches!(executing, Executing::No)
            && let Some(abbr_value) = session_context
                .session
                .abbreviation_value(&first_token.item)
        {
            return Some((first_token, abbr_value));
        }

        // We only expand aliases if the user has turned the setting on.
        if self.should_expand_aliases(ctx) {
            let alias_value = session_context.session.alias_value(&first_token.item)?;
            if !is_expandable_alias(&first_token.item, alias_value) {
                return None;
            }

            return Some((first_token, alias_value));
        }
        None
    }

    /// Function to check whether the previous token was a valid command abbreviation
    /// or alias and handle expansion. This should only be called after the user has
    /// entered a space into the input editor.
    fn run_expansion_on_space(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(expansion_info) = self.run_expansion_internal(Executing::No, ctx) {
            self.expand_alias(expansion_info.byte_range, &expansion_info.alias_value, ctx);
        }
    }

    /// Function that checks whether the current token was a valid command abbreviation
    /// or alias, and returns a String representing the input buffer with the expanded
    /// text. This should be called after the user has pressed Enter to execute the
    /// command.
    fn get_expanded_command_on_execute(&mut self, ctx: &mut ViewContext<Self>) -> Option<String> {
        self.run_expansion_internal(Executing::Yes, ctx)
            .and_then(|expansion_info| {
                let mut text = expansion_info.buffer_text;
                let is_valid_byte_range = text.is_char_boundary(expansion_info.byte_range.start)
                    && text.is_char_boundary(expansion_info.byte_range.end);
                is_valid_byte_range.then(|| {
                    text.replace_range(expansion_info.byte_range, &expansion_info.alias_value);
                    text
                })
            })
    }

    /// Helper function that handles whether there is a valid expansion based on
    /// the current input buffer and cursor position. Returns info needed to
    /// perform the expansion.
    fn run_expansion_internal(
        &mut self,
        executing: Executing,
        ctx: &mut ViewContext<Self>,
    ) -> Option<ExpansionInfo> {
        let session_context = self.completion_session_context(ctx)?;
        let editor = self.editor.as_ref(ctx);
        editor.single_cursor_to_point(ctx)?;
        let buffer_text = editor.buffer_text(ctx);
        let cursor_pos = editor.end_byte_index_of_last_selection(ctx);
        let command = command_at_cursor_position(
            buffer_text.as_str(),
            session_context.escape_char(),
            cursor_pos,
        );

        self.get_valid_abbreviation_or_alias_for_expansion(
            command.as_ref(),
            cursor_pos.as_usize(),
            executing,
            &session_context,
            ctx,
        )
        .map(|(alias, alias_value)| ExpansionInfo {
            alias_value: alias_value.into(),
            buffer_text,
            byte_range: alias.span().start()..cursor_pos.as_usize(),
        })
    }

    fn expand_alias(
        &mut self,
        replacement_range: Range<usize>,
        alias_value: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let alias_value_with_space = format!("{alias_value} ");
        self.editor.update(ctx, |input, ctx| {
            input.select_and_replace(
                &alias_value_with_space,
                [ByteOffset::from(replacement_range.start)
                    ..ByteOffset::from(replacement_range.end)],
                PlainTextEditorViewAction::ExpandAlias,
                ctx,
            );
        });
    }

    /// If at least one input is being synced, emit an event that other
    /// terminal views can decide to process based on their sync state.
    fn send_input_sync_event(&self, edit_origin: &EditOrigin, ctx: &mut ViewContext<Self>) {
        let is_syncing_inputs =
            SyncedInputState::as_ref(ctx).is_syncing_any_inputs(ctx.window_id());

        if is_syncing_inputs
                    // If the edit we're applying in `handle_editor_event`
                    //came from another synced terminal,
                    // don't emit a new event which would create a cycle
                    && *edit_origin != EditOrigin::SyncedTerminalInput
                    // Similarly, only emit an event from the session the user is typing in
                    && self.focus_handle.as_ref().is_none_or(|h| h.is_focused(ctx))
        {
            let buffer = self.editor.as_ref(ctx).buffer_text(ctx);
            ctx.emit(Event::SyncInput(
                SyncInputType::InputEditorContentsChanged {
                    contents: Arc::new(buffer),
                },
            ));
        }
    }

    fn handle_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        // We want to clear the token description hover on any editor action
        self.hide_x_ray(ctx);

        if !matches!(event, EditorEvent::InsertLastWordPrevCommand) {
            self.update_last_word_insertion_state();
        }

        match event {
            EditorEvent::Edited(edit_origin) => {
                // We should ideally be handling all `Edited` events, not just those that are
                // marked EditOrigin. However, we receive the notification that the block has
                // completed, in the same event we clear the input box per-command. Due to how
                // events are dispatched in the UI framework, we would receive an Edited event
                // immediately from clearing the input box. But we don't want that.
                // Only processing the user typed events should be good enough here.

                if matches!(
                    edit_origin,
                    EditOrigin::UserTyped | EditOrigin::UserInitiated
                ) {
                    self.model.lock().set_is_input_dirty(true);
                }

                if self.maybe_activate_cloud_handoff_prefix(edit_origin, ctx) {
                    return;
                }

                if *edit_origin == EditOrigin::UserTyped
                    && !ctx
                        .model(&self.input_render_state_model_handle)
                        .editor_modified_since_block_finished()
                {
                    self.input_render_state_model_handle.update(
                        ctx,
                        |input_render_state_model, _| {
                            input_render_state_model.set_editor_modified_since_block_finished(true);
                        },
                    );
                    ctx.notify();
                }

                let is_editor_empty = self.editor.as_ref(ctx).is_empty(ctx);
                if is_editor_empty != self.is_editor_empty_on_last_edit {
                    self.is_editor_empty_on_last_edit = is_editor_empty;
                    ctx.emit(Event::InputEmptyStateChanged {
                        is_empty: is_editor_empty,
                        reason: InputEmptyStateChangeReason::Edited,
                    });
                }

                let is_ai_input_enabled = self.ai_input_model.as_ref(ctx).is_ai_input_enabled();

                let mut short_circuit_highlighting = false;
                let mut check_alias_expansion = false;

                self.editor.read(ctx, |editor, editor_ctx| {
                    let last_action = editor.get_last_action(editor_ctx);
                    if Some(PlainTextEditorViewAction::Space) == last_action
                        && *edit_origin == EditOrigin::UserTyped
                    {
                        check_alias_expansion = true;
                    }

                    if SHORT_CIRCUIT_HIGHLIGHTING_ACTIONS.contains(&last_action) {
                        short_circuit_highlighting = true;
                    }
                });

                // Force AI mode if buffer contains any attachment patterns (blocks, drive objects, diffs)
                if AISettings::as_ref(ctx).is_any_ai_enabled(ctx) && edit_origin.is_user() {
                    let buffer_text = self.buffer_text(ctx);
                    if Self::buffer_contains_attachment_patterns(&buffer_text) {
                        self.ensure_agent_mode_for_ai_features(
                            false,
                            Some(InputTypeAutoDetectionSource::AttachmentForcedAi),
                            ctx,
                        );
                    }
                }

                if check_alias_expansion {
                    self.run_expansion_on_space(ctx);
                }

                // NLD autodetection is irrelevant in cloud mode v2 — the input is always AI.
                let should_run_ai_input_detection = if self.is_cloud_mode_input_v2_composing(ctx) {
                    false
                } else {
                    match edit_origin {
                        // Edits made by the local user should trigger autodetection, if
                        // it is enabled.
                        EditOrigin::UserInitiated
                        | EditOrigin::UserTyped
                        | EditOrigin::SyncedTerminalInput => self
                            .ai_input_model
                            .as_ref(ctx)
                            .should_run_input_autodetection(ctx),
                        // Remote edits from shared session viewers should trigger autodetection
                        // on the sharer's side, so that the sharer's input mode adjusts as viewers type.
                        EditOrigin::RemoteEdit => {
                            let is_sharer = self.model.lock().shared_session_status().is_sharer();
                            is_sharer
                                && self
                                    .ai_input_model
                                    .as_ref(ctx)
                                    .should_run_input_autodetection(ctx)
                        }
                        // System edits should never trigger autodetection.
                        EditOrigin::SystemEdit => false,
                    }
                };

                // Abort any autodetection work on the old buffer state.
                self.ai_input_model.update(ctx, |controller, _| {
                    controller.abort_in_progress_detection();
                });

                if self.should_apply_decorations(ctx)
                    || should_run_ai_input_detection
                    || is_ai_input_enabled
                {
                    let mut mode = InputBackgroundJobOptions::default();

                    if self.should_apply_decorations(ctx) {
                        mode = mode.with_command_decoration();
                    }

                    if should_run_ai_input_detection {
                        mode = mode.with_ai_input_detection();
                    }

                    if short_circuit_highlighting {
                        self.run_input_background_jobs(mode, ctx);
                    } else {
                        let _ = self.debounce_input_background_tx.try_send(mode);
                    }
                }

                let is_input_mode_locked = self.ai_input_model.as_ref(ctx).is_input_type_locked();
                let buffer_text = self.buffer_text(ctx);

                // If the last buffer didn't start with the AI input prefix and the current buffer does, then enable AI input.
                if FeatureFlag::AgentMode.is_enabled()
                    && !FeatureFlag::AgentView.is_enabled()
                    && AISettings::as_ref(ctx).is_any_ai_enabled(ctx)
                    && (!is_ai_input_enabled || !is_input_mode_locked)
                {
                    if buffer_text.starts_with(AI_INPUT_PREFIX)
                        && *edit_origin == EditOrigin::UserTyped
                    {
                        let last_buffer_text = self.editor.as_ref(ctx).last_buffer_text(ctx);

                        if !last_buffer_text.starts_with(AI_INPUT_PREFIX) {
                            // Remove the prefix from the editor contents.
                            let is_input_buffer_empty =
                                self.editor.update(ctx, |editor_view, ctx| {
                                    if let Some(query) =
                                        editor_view.buffer_text(ctx).strip_prefix(AI_INPUT_PREFIX)
                                    {
                                        editor_view.set_buffer_text(query, ctx);
                                    }
                                    editor_view.buffer_text(ctx).is_empty()
                                });

                            self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                                ai_input_model.set_input_config(
                                    InputConfig {
                                        input_type: InputType::AI,
                                        is_locked: true,
                                    },
                                    is_input_buffer_empty,
                                    Some(InputTypeAutoDetectionSource::AgentModePrefix),
                                    ctx,
                                );
                            });
                        }
                    } else if buffer_text.is_empty() && is_input_mode_locked {
                        self.ai_input_model.update(ctx, |input_model, ctx| {
                            input_model.set_input_config_for_classic_mode(
                                input_model
                                    .input_config()
                                    .unlocked_if_autodetection_enabled(false, ctx),
                                ctx,
                            );
                        });
                    }

                    ctx.notify();
                }

                let ai_settings = AISettings::as_ref(ctx);
                if FeatureFlag::AgentView.is_enabled()
                    && buffer_text.is_empty()
                    && self.prefix_mode(ctx) != InputPrefixMode::CloudHandoff
                {
                    let last_buffer_text = self.editor.as_ref(ctx).last_buffer_text(ctx);
                    let was_shell_mode_prefix_stripped =
                        last_buffer_text == TERMINAL_INPUT_PREFIX && buffer_text.is_empty();

                    let is_fullscreen_agent_view_active =
                        self.agent_view_controller.as_ref(ctx).is_fullscreen();
                    let current_input_config = self.ai_input_model.as_ref(ctx).input_config();

                    // We should re-enable autodetection if the user overrode an autodetection
                    // result:
                    // * In agent view, this means the user overrode a mis-classified shell command
                    //   to be an agent prompt.
                    // * In terminal view, this eans the user overrode a mis-classified agent prompt
                    //   to a terminal command.
                    let should_reenable_autodetection = (ai_settings
                        .is_ai_autodetection_enabled(ctx)
                        && is_fullscreen_agent_view_active
                        && current_input_config.is_ai()
                        && current_input_config.is_locked
                        && !was_shell_mode_prefix_stripped)
                        || (ai_settings.is_nld_in_terminal_enabled(ctx)
                            && !self.agent_view_controller.as_ref(ctx).is_active()
                            && current_input_config.is_shell()
                            && current_input_config.is_locked);
                    if should_reenable_autodetection {
                        self.ai_input_model.update(ctx, |input_model, ctx| {
                            input_model.enable_autodetection(input_model.input_type(), ctx);
                        });
                        ctx.notify();
                    }
                }

                // If the last buffer didn't start with the terminal input prefix and the current buffer does, then enable terminal input and lock it.
                let is_locked_shell_mode = !is_ai_input_enabled && is_input_mode_locked;
                let is_agent_view_active = self.agent_view_controller.as_ref(ctx).is_active();
                let is_agent_in_control_or_tagged_in = self
                    .model
                    .lock()
                    .block_list()
                    .active_block()
                    .is_agent_in_control_or_tagged_in();
                if FeatureFlag::AgentMode.is_enabled()
                    && !is_locked_shell_mode
                    && (!FeatureFlag::AgentView.is_enabled() || is_agent_view_active)
                    && !is_agent_in_control_or_tagged_in
                    && self.prefix_mode(ctx) != InputPrefixMode::CloudHandoff
                {
                    let buffer_text = self.buffer_text(ctx);
                    if buffer_text.starts_with(TERMINAL_INPUT_PREFIX)
                        && *edit_origin == EditOrigin::UserTyped
                    {
                        let last_buffer_text = self.editor.as_ref(ctx).last_buffer_text(ctx);

                        if !last_buffer_text.starts_with(TERMINAL_INPUT_PREFIX) {
                            // Remove the prefix from the editor contents.
                            let is_input_buffer_empty =
                                self.editor.update(ctx, |editor_view, ctx| {
                                    if let Some(command) = editor_view
                                        .buffer_text(ctx)
                                        .strip_prefix(TERMINAL_INPUT_PREFIX)
                                    {
                                        editor_view.set_buffer_text(command, ctx);
                                    }
                                    editor_view.buffer_text(ctx).is_empty()
                                });

                            self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                                ai_input_model.set_input_config(
                                    InputConfig {
                                        input_type: InputType::Shell,
                                        is_locked: true,
                                    },
                                    is_input_buffer_empty,
                                    Some(InputTypeAutoDetectionSource::ShellPrefix),
                                    ctx,
                                );
                            });
                        }
                    }
                    ctx.notify();
                }

                // We only sync on EditorEvent::Edited events because we're only
                // syncing terminal input editor contents, not the full
                // functionality of the terminal input in each blocklist
                // e.g., we don't want to sync EditorEvent::CmdUpOnFirstRow.
                self.send_input_sync_event(edit_origin, ctx);

                // Distinguish edits the completion system applied itself (accepting or
                // cycling through a candidate) from edits the user made (typing,
                // backspacing, pasting). System-applied edits are allowed to diverge from
                // the original completion query so that classic cycling keeps working;
                // user edits still have to be revalidated against that query.
                let is_user_edit = !matches!(
                    self.editor.as_ref(ctx).get_last_action(ctx),
                    Some(
                        PlainTextEditorViewAction::AcceptCompletionSuggestion
                            | PlainTextEditorViewAction::CycleCompletionSuggestion
                    )
                );

                let mode = self.suggestions_mode_model.as_ref(ctx).mode().clone();
                match &mode {
                    InputSuggestionsMode::CompletionSuggestions {
                        replacement_start,
                        buffer_text_original,
                        completion_results,
                        trigger,
                        ..
                    } => {
                        let replacement_start = *replacement_start;
                        let editor_text = self.buffer_text(ctx);
                        let cursor_position = self.start_byte_index_of_last_selection(ctx);
                        let current_word =
                            editor_text.get(replacement_start..cursor_position.as_usize());
                        let current_selected_item =
                            self.input_suggestions.as_ref(ctx).get_selected_item_text();
                        let selected_item_differs_from_current_word = current_selected_item
                            .zip(current_word)
                            .map(|(selected_item, current_word)| selected_item != current_word)
                            .unwrap_or(true);

                        // To support completions-as-you-type x classic completions,
                        // we need to make sure we don't recompute the completion results
                        // as the user cycles (which inserts into buffer and thus is treated
                        // as an edit). Thus, when using the two features together, we only
                        // recompute the result set if the selected item doesn't match the
                        // current word span.
                        let old_buffer_text_original = buffer_text_original.clone();
                        if *trigger == CompletionsTrigger::AsYouType
                            && (!self.is_classic_completions_enabled(ctx)
                                || (self.is_classic_completions_enabled(ctx)
                                    && selected_item_differs_from_current_word))
                        {
                            // For as-you-type completions, we recalculate suggestions rather than
                            // filtering, since typing could involve moving to a new parameter
                            // within a given command, rather than being a strict subset as is the
                            // case with manual tab completions.
                            self.open_completion_suggestions(CompletionsTrigger::AsYouType, ctx);
                            self.maybe_generate_autosuggestion(ctx);

                            // Since tab completions are async, we should close the
                            // menu if it's been some time and the menu still hasn't updated,
                            // otherwise the user will see an old completions menu even while
                            // the buffer text has changed. We wait with a delay so that way
                            // the menu doesn't close right away and open away right after if
                            // the completions finish quickly, since that causes a jittery UX.
                            let _ = ctx.spawn(
                                async move {
                                    warpui::r#async::Timer::after(Duration::from_millis(750)).await;
                                    old_buffer_text_original
                                },
                                move |input, old_buffer_text_original, ctx| {
                                    if let InputSuggestionsMode::CompletionSuggestions {
                                        buffer_text_original,
                                        ..
                                    } = input.suggestions_mode_model.as_ref(ctx).mode()
                                    {
                                        // The menu hasn't changed since last time so
                                        // close it for now. If the menu is truly delayed,
                                        // the completions callback will eventually open it.
                                        if old_buffer_text_original == *buffer_text_original {
                                            input.close_input_suggestions(true, ctx);
                                        }
                                    }
                                },
                            );
                        } else {
                            let buffer_text_original = buffer_text_original.clone();
                            let completion_results = completion_results.clone();
                            let should_close = self.update_tab_completion_menu(
                                replacement_start,
                                buffer_text_original.as_str(),
                                &completion_results,
                                is_user_edit,
                                ctx,
                            );
                            if should_close {
                                self.close_input_suggestions(
                                    /*should_focus_input=*/ true, ctx,
                                );
                            }
                        }
                    }
                    InputSuggestionsMode::StaticWorkflowEnumSuggestions {
                        cursor_point, ..
                    }
                    | InputSuggestionsMode::DynamicWorkflowEnumSuggestions {
                        cursor_point, ..
                    } => {
                        let cursor_point = *cursor_point;
                        let point = self.editor.as_ref(ctx).first_selection_end_to_point(ctx);
                        let should_close = point != cursor_point;

                        if should_close {
                            self.close_input_suggestions(/*should_focus_input=*/ true, ctx);
                        }
                    }
                    InputSuggestionsMode::HistoryUp { .. } => {
                        // In HistoryUp mode, we replace the buffer as options
                        // are selected.
                        // We also dismiss the suggestion menu if the buffer
                        // is edited such that it doesn't exactly match
                        // the selected suggestion.

                        if let Some(selected_text) =
                            self.input_suggestions.as_ref(ctx).get_selected_item_text()
                        {
                            if *selected_text.to_string()
                                == self.editor.as_ref(ctx).buffer_text(ctx)
                            {
                                return;
                            }

                            let has_active_ai_block =
                                self.model.lock().block_list().has_active_ai_block(ctx);
                            // We only focus the input if there is no active AI
                            // block. Otherwise, the input is incorrectly focused
                            // when executing an AI query from the history menu.
                            self.close_input_suggestions(
                                !has_active_ai_block, /*should_focus_input=*/
                                ctx,
                            );
                        }
                    }
                    InputSuggestionsMode::Closed => {
                        if !self.can_query_history(ctx) {
                            return;
                        }

                        let editor = self.editor.as_ref(ctx);
                        let buffer_text = editor.buffer_text(ctx);

                        self.maybe_generate_autosuggestion(ctx);

                        if buffer_text.is_empty()
                            && self.workflows_state.selected_workflow_state.is_some()
                        {
                            self.clear_current_workflow(ctx);
                        }

                        if self.should_show_completions_while_typing(ctx)
                            && matches!(edit_origin, EditOrigin::UserTyped)
                        {
                            self.open_completion_suggestions(CompletionsTrigger::AsYouType, ctx);
                        }
                    }
                }
            }
            EditorEvent::BufferReplaced => {
                let ai_input_model = self.ai_input_model.as_ref(ctx);
                if FeatureFlag::AgentMode.is_enabled()
                    && AISettings::as_ref(ctx).is_any_ai_enabled(ctx)
                    && !ai_input_model.is_ai_input_enabled()
                    && ai_input_model.is_input_type_locked()
                {
                    // If this edit effectively emptied the buffer and we're in shell mode,
                    // unlock the input so autodetection can kick in.
                    self.ai_input_model.update(ctx, |input_model, ctx| {
                        input_model.set_input_config_for_classic_mode(
                            input_model
                                .input_config()
                                .unlocked_if_autodetection_enabled(false, ctx),
                            ctx,
                        );
                    });
                    ctx.notify();
                }
            }
            EditorEvent::SelectionChanged => {
                let mode = self.suggestions_mode_model.as_ref(ctx).mode().clone();
                let is_completion_suggestions =
                    matches!(mode, InputSuggestionsMode::CompletionSuggestions { .. });
                if is_completion_suggestions && !self.cursor_positioned_for_completion(ctx) {
                    self.close_input_suggestions(/*should_focus_input=*/ true, ctx);
                } else {
                    match &mode {
                        InputSuggestionsMode::HistoryUp { .. } | InputSuggestionsMode::Closed => {}
                        InputSuggestionsMode::CompletionSuggestions {
                            replacement_start,
                            buffer_text_original,
                            completion_results,
                            ..
                        } => {
                            let replacement_start = *replacement_start;
                            let buffer_text_original = buffer_text_original.clone();
                            let completion_results = completion_results.clone();
                            // A selection change is a cursor move, not a buffer edit, so it
                            // never counts as a user edit for invalidation purposes.
                            let should_close = self.update_tab_completion_menu(
                                replacement_start,
                                buffer_text_original.as_str(),
                                &completion_results,
                                /*is_user_edit=*/ false,
                                ctx,
                            );

                            if should_close {
                                self.close_input_suggestions(
                                    /*should_focus_input=*/ true, ctx,
                                );
                            }
                        }
                        InputSuggestionsMode::StaticWorkflowEnumSuggestions {
                            cursor_point,
                            ..
                        }
                        | InputSuggestionsMode::DynamicWorkflowEnumSuggestions {
                            cursor_point,
                            ..
                        } => {
                            let cursor_point = *cursor_point;
                            let point = self.editor.as_ref(ctx).first_selection_end_to_point(ctx);
                            let should_close = point != cursor_point;

                            if should_close {
                                self.close_input_suggestions(
                                    /*should_focus_input=*/ true, ctx,
                                );
                            }
                        }
                    }
                }
            }
            EditorEvent::AutosuggestionAccepted {
                autosuggestion_type,
                ..
            } => {
                ctx.emit(Event::AutosuggestionAccepted);

                self.input_suggestions
                    .update(ctx, |input_suggestions, ctx| {
                        // We should not restore the buffer to the old state since we're accepting an autosuggestion from the new state.
                        input_suggestions.exit(false, ctx);
                    });
                match autosuggestion_type {
                    AutosuggestionType::Command => {
                        // Switch to shell input mode but preserve current lock state when accepting a command autosuggestion.
                        self.ai_input_model.update(ctx, |input_model, ctx| {
                            input_model.set_input_type(
                                InputType::Shell,
                                Some(InputTypeAutoDetectionSource::CommandAutosuggestionAccepted),
                                ctx,
                            );
                        });
                        // Count accepted command autosuggestions for the right-arrow hint.
                        InputSettings::handle(ctx).update(ctx, |input_settings, ctx| {
                            let current_count =
                                *input_settings.autosuggestion_accepted_count.value();
                            if current_count < MAX_TIMES_TO_SHOW_AUTOSUGGESTION_HINT {
                                let new_count = if current_count < 0 {
                                    // Note: there was a bug in the previous implementation of this method which would
                                    // cause it to overflow the i8 value to a negative value. In that case, we know
                                    // that the user has definitely accepted at _least_ 128 autosuggestions, so we can
                                    // set it to the maximum relevant value: MAX_TIMES_TO_SHOW_AUTOSUGGESTION_HINT
                                    MAX_TIMES_TO_SHOW_AUTOSUGGESTION_HINT
                                } else {
                                    current_count + 1
                                };

                                report_if_error!(
                                    input_settings
                                        .autosuggestion_accepted_count
                                        .set_value(new_count, ctx)
                                )
                            }
                        })
                    }
                    AutosuggestionType::AgentModeQuery { context_block_ids } => {
                        // Switch to AI input mode but preserve current lock state when accepting an Agent Mode query autosuggestion.
                        self.enter_ai_mode(
                            Some(InputTypeAutoDetectionSource::AgentQueryAutosuggestionAccepted),
                            ctx,
                        );
                        self.ai_context_model.update(ctx, |context_model, ctx| {
                            context_model.set_pending_context_block_ids(
                                context_block_ids.clone(),
                                true,
                                ctx,
                            )
                        });
                    }
                };
            }
            EditorEvent::Navigate(NavigationKey::Up) => {
                self.editor_up(ctx);
            }
            EditorEvent::Navigate(NavigationKey::Down) => {
                self.editor_down(ctx);
            }
            EditorEvent::Navigate(NavigationKey::PageUp) => {
                self.editor_page_up(ctx);
            }
            EditorEvent::Navigate(NavigationKey::PageDown) => {
                self.editor_page_down(ctx);
            }
            EditorEvent::Navigate(NavigationKey::Tab) => {
                self.input_tab(ctx);
            }
            EditorEvent::Navigate(NavigationKey::ShiftTab) => {
                self.input_shift_tab(ctx);
            }
            EditorEvent::Enter => self.input_enter(ctx),
            EditorEvent::CmdEnter => self.input_cmd_enter(ctx),
            EditorEvent::Escape => self.editor_escape(ctx),
            EditorEvent::CtrlC { cleared_buffer_len } => {
                self.close_input_suggestions(/*should_focus_input=*/ true, ctx);

                self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                    ai_input_model.set_input_config_for_classic_mode(
                        InputConfig {
                            input_type: InputType::Shell,
                            is_locked: true,
                        }
                        .unlocked_if_autodetection_enabled(false, ctx),
                        ctx,
                    );
                });
                ctx.emit(Event::CtrlC {
                    cleared_buffer_len: *cleared_buffer_len,
                });
            }
            EditorEvent::DeleteAllLeft => {
                if self.ai_input_model.as_ref(ctx).is_ai_input_enabled() {
                    let new_input_type = InputType::Shell;
                    self.maybe_send_autodetection_telemetry_on_manual_toggle(new_input_type, ctx);
                    self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                        ai_input_model.set_input_config_for_classic_mode(
                            InputConfig {
                                input_type: new_input_type,
                                is_locked: true,
                            }
                            .unlocked_if_autodetection_enabled(false, ctx),
                            ctx,
                        );
                    });
                } else if self.ai_input_model.as_ref(ctx).is_input_type_locked() {
                    let is_agent_view_fullscreen =
                        self.agent_view_controller.as_ref(ctx).is_fullscreen();
                    if is_agent_view_fullscreen {
                        self.exit_shell_mode_to_ai(ctx);
                    }
                }
            }
            EditorEvent::CmdUpOnFirstRow => ctx.emit(Event::SelectRecentBlocks { count: 1 }),
            EditorEvent::Copy => ctx.emit(Event::Copy),
            EditorEvent::UnhandledModifierKeyOnEditor(keystroke) => {
                ctx.emit(Event::UnhandledModifierKeyOnEditor(keystroke.clone()))
            }
            EditorEvent::ClearParentSelections => {
                ctx.emit(Event::ClearSelectionsWhenShellMode);
            }
            EditorEvent::HideXRay => {
                self.hide_x_ray(ctx);
            }
            EditorEvent::TryToShowXRay(token_at) => {
                if self.ai_input_model.as_ref(ctx).is_ai_input_enabled() {
                    // Don't show command x-ray for AI queries.
                    return;
                }

                match token_at {
                    CommandXRayAnchor::Cursor => {
                        let pos = self.start_byte_index_of_first_selection(ctx);
                        self.start_xray_at_offset(pos, CommandXRayTrigger::Keystroke, ctx);
                    }
                    CommandXRayAnchor::Hover(mouse_position) => {
                        if let Some(offset) = self.start_byte_index_at_point(mouse_position, ctx)
                            && !self.suggestions_mode_model.as_ref(ctx).is_visible()
                        {
                            self.start_xray_at_offset(offset, CommandXRayTrigger::Hover, ctx);
                        }
                    }
                }
            }
            EditorEvent::InsertLastWordPrevCommand => self.insert_last_word_previous_command(ctx),
            // For this particular view, the terminal Input, we ignore search direction because in
            // this context, search means search through History which isn't actually sensitive to
            // left/right direction.
            EditorEvent::Search { term, .. } => {
                ctx.emit(Event::ShowCommandSearch(CommandSearchOptions {
                    filter: Some(QueryFilter::History),
                    init_content: InitContent::Custom(term.clone().unwrap_or("".to_owned())),
                }));
            }
            EditorEvent::VimStatusUpdate => ctx.notify(),
            EditorEvent::BackspaceOnEmptyBuffer | EditorEvent::BackspaceAtBeginningOfBuffer => {
                self.handle_backspace_at_buffer_boundary(ctx);
            }
            EditorEvent::EmacsBindingUsed => {
                ctx.emit(Event::EmacsBindingUsed);
            }
            EditorEvent::UpdatePeers { operations } => {
                self.latest_buffer_operations.extend(operations.to_vec());

                // TODO (suraj): we might want to push down the buffer ID to the buffer
                // and have it returned as part of the event. That way, we aren't subject
                // to any skew of the block ID from the time the event is emitted (when the edit
                // is processed) to the time when we query the block ID (now).
                ctx.emit(Event::EditorUpdated {
                    block_id: self.model.lock().block_list().active_block_id().clone(),
                    operations: operations.clone(),
                })
            }
            EditorEvent::MiddleClickPaste => {
                ctx.emit(Event::InputFocusedFromMiddleClick);
            }
            EditorEvent::Focused => ctx.emit(Event::EditorFocused),
            EditorEvent::ProcessingAttachedImages(is_processing) => {
                self.set_is_processing_attached_images(*is_processing, ctx);
            }
            EditorEvent::Paste => {
                self.process_paste_event(ctx);
            }
            EditorEvent::DroppedImageFiles(image_filepaths) => {
                // Handle image processing from EditorView drag-and-drop
                let num_attached =
                    self.handle_pasted_or_dragdropped_image_filepaths(image_filepaths.clone(), ctx);

                // If any attachment failed, insert all dropped image paths as text. Apply the
                // same session-aware path transformation that the editor uses for dropped
                // non-image paths so the fallback matches the primary drop flow (e.g.
                // `/mnt/c/...` in a WSL session).
                if num_attached < image_filepaths.len() {
                    let shell_family = self.editor.read(ctx, |editor, _| editor.shell_family());
                    let converter = self
                        .active_session(ctx)
                        .as_deref()
                        .and_then(Session::windows_path_converter);
                    let transformed: Vec<String> = match converter {
                        Some(convert) => image_filepaths.iter().map(|p| convert(p)).collect(),
                        None => image_filepaths.clone(),
                    };
                    let paths_str =
                        warpui::clipboard_utils::escaped_paths_str(&transformed, shell_family);

                    self.editor.update(ctx, |editor, ctx| {
                        editor.user_insert(&paths_str, ctx);
                    });
                }
            }
            EditorEvent::IgnoreAutosuggestion { suggestion } => {
                IgnoredSuggestionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.add_ignored_suggestion(
                        suggestion.clone(),
                        SuggestionType::ShellCommand,
                        ctx,
                    );
                });

                self.editor.update(ctx, |editor, ctx| {
                    editor.clear_autosuggestion(ctx);
                });
            }
            _ => {}
        }
    }

    /// Process paste event by checking clipboard for images and handling appropriately.
    fn process_paste_event(&mut self, ctx: &mut ViewContext<Self>) {
        // Read from app clipboard
        let content = ctx.clipboard().read();

        // If AI is disabled, attachment isn't possible
        if !AISettings::as_ref(ctx).is_any_ai_enabled(ctx) {
            self.insert_clipboard_text_content(ctx, content);
            return;
        }

        // Shared session viewers cannot attach images unless in cloud mode
        let is_viewer = self.model.lock().shared_session_status().is_viewer();
        let is_cloud_mode_with_images = FeatureFlag::CloudModeImageContext.is_enabled()
            && self
                .ambient_agent_view_model()
                .is_some_and(|ambient_agent_model| {
                    ambient_agent_model.as_ref(ctx).is_ambient_agent()
                });
        if is_viewer && !is_cloud_mode_with_images {
            self.insert_clipboard_text_content(ctx, content);
            return;
        }

        // Check if we should insert clipboard text in advance
        let mut already_inserted_text = false;
        if warpui::clipboard::should_insert_text_on_paste(&content) {
            self.insert_clipboard_text_content(ctx, content.clone());
            already_inserted_text = true;
        }

        // Try to attach images
        // If any attachment fails, should_insert_text = true.
        let should_insert_text = if content.has_image_data() {
            // If we have image data, process the image data.
            self.handle_pasted_image_data(content.clone(), ctx) == 0
        } else if content.num_paths() > 0 {
            // Else, we check the pasted file paths for any images.
            let image_filepaths = warpui::clipboard_utils::get_image_filepaths_from_paths(
                content.paths.as_deref().unwrap_or(&[]),
            );
            let num_images_expected = image_filepaths.len();
            self.handle_pasted_or_dragdropped_image_filepaths(image_filepaths, ctx)
                < num_images_expected
        } else {
            true
        };

        // Fallback to inserting text
        if should_insert_text && !already_inserted_text {
            self.insert_clipboard_text_content(ctx, content);
        }
    }

    /// Insert clipboard text content (paths / plaintext)
    fn insert_clipboard_text_content(
        &self,
        ctx: &mut ViewContext<Self>,
        content: ClipboardContent,
    ) {
        let clipboard_content_str = self
            .editor
            .read(ctx, |editor, _| editor.clipboard_text_content(content));
        self.editor.update(ctx, |editor, ctx| {
            editor.user_initiated_insert(
                &clipboard_content_str,
                PlainTextEditorViewAction::Paste,
                ctx,
            );
        });
    }

    /// Check if we can attach on filepaths paste or drag-drop
    fn can_attach_on_filepaths_paste_or_dragdrop(&self, ctx: &mut ViewContext<Self>) -> bool {
        // Shared session viewers cannot attach images unless in cloud mode
        // with the CloudModeImageContext feature enabled.
        let is_viewer = self.model.lock().shared_session_status().is_viewer();
        let is_cloud_mode_with_images = FeatureFlag::CloudModeImageContext.is_enabled()
            && self
                .ambient_agent_view_model()
                .is_some_and(|ambient_agent_model| {
                    ambient_agent_model.as_ref(ctx).is_ambient_agent()
                });
        if is_viewer && !is_cloud_mode_with_images {
            return false;
        }

        let is_udi_enabled = InputSettings::as_ref(ctx).is_universal_developer_input_enabled(ctx);
        if !is_udi_enabled && !FeatureFlag::AgentView.is_enabled() {
            return false;
        }

        // Check if Agent Mode enabled, in active agent view, or if the buffer is empty
        // (if the buffer is empty, we assume that the user wants the images to be attached).
        let ai_input = self.ai_input_model.as_ref(ctx);
        let in_agent_mode = matches!(ai_input.input_type(), InputType::AI);
        let is_buffer_empty = self.buffer_text(ctx).is_empty();
        let in_active_agent_view = self.agent_view_controller.as_ref(ctx).is_active();
        in_agent_mode || is_buffer_empty || in_active_agent_view
    }

    /// Handle direct image data from clipboard (e.g., copied images). Returns number of images attached.
    fn handle_pasted_image_data(
        &mut self,
        clipboard_content: ClipboardContent,
        ctx: &mut ViewContext<Self>,
    ) -> usize {
        if self.check_image_limits_for_paste(1, ctx) == 0 {
            return 0;
        }

        if let Some(images) = clipboard_content.images {
            let best_image = CLIPBOARD_IMAGE_MIME_TYPES
                .iter()
                .find_map(|format| images.iter().find(|img| img.mime_type == *format));

            if let Some(image) = best_image {
                self.process_and_attach_clipboard_image(image.clone(), ctx);
                return 1;
            }
        }

        0
    }

    /// Handle pasted file paths that point to images for auto-attachment. Returns number of images attached.
    pub fn handle_pasted_or_dragdropped_image_filepaths(
        &mut self,
        image_filepaths: Vec<String>,
        ctx: &mut ViewContext<Self>,
    ) -> usize {
        // Return early if no image paths
        if image_filepaths.is_empty() {
            return 0;
        }

        if !self.can_attach_on_filepaths_paste_or_dragdrop(ctx) {
            return 0;
        }

        self.maybe_enter_agent_view_for_image_add(ctx);

        let num_images_to_attach = self.check_image_limits_for_paste(image_filepaths.len(), ctx);
        if num_images_to_attach == 0 {
            return 0;
        }

        let is_buffer_empty = self.buffer_text(ctx).is_empty();
        let in_active_agent_view = self.agent_view_controller.as_ref(ctx).is_active();
        if is_buffer_empty || in_active_agent_view {
            self.set_input_mode_agent(true, ctx);
            self.update_image_context_options(ctx);
        }

        let paths_to_process: Vec<String> = image_filepaths
            .into_iter()
            .take(num_images_to_attach)
            .collect();

        let num_paths = paths_to_process.len();
        self.editor.update(ctx, |editor, ctx| {
            editor.read_and_process_images_async(num_paths, paths_to_process, ctx);
        });
        num_paths
    }

    /// Convert clipboard image data to AttachedImage and attach to editor in Agent Mode.
    fn process_and_attach_clipboard_image(
        &mut self,
        image: ImageData,
        ctx: &mut ViewContext<Self>,
    ) {
        self.maybe_enter_agent_view_for_image_add(ctx);

        // Switch to AI mode with block-level lock, unless already AI-mode-locked
        if !self.is_locked_in_ai_mode(ctx) {
            self.set_input_mode_agent(true, ctx);
            self.update_image_context_options(ctx);
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let ext = match image.mime_type.as_str() {
            "image/png" => "png",
            "image/jpeg" | "image/jpg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "img",
        };

        // Use preserved filename if available, otherwise generate fallback name
        let file_name = if let Some(original_filename) = &image.filename {
            original_filename.clone()
        } else {
            format!("pasted-image-{timestamp}.{ext}")
        };

        let attached_image = AttachedImageRawData {
            data: image.data,
            mime_type: image.mime_type,
            file_name,
        };

        self.editor.update(ctx, |editor, ctx| {
            editor.process_and_attach_images_as_ai_context(1, vec![attached_image], ctx);
        });
    }

    /// Enters agent view when adding images, unless Agent View is disabled,
    /// we're already in the agent view, or a long running command is in progress.
    fn maybe_enter_agent_view_for_image_add(&mut self, ctx: &mut ViewContext<Self>) {
        let is_in_long_running_command = self
            .model
            .lock()
            .block_list()
            .active_block()
            .is_active_and_long_running();
        if !FeatureFlag::AgentView.is_enabled()
            || self.agent_view_controller.as_ref(ctx).is_active()
            || is_in_long_running_command
        {
            return;
        }

        if let Err(e) = self.agent_view_controller.update(ctx, |controller, ctx| {
            controller.try_enter_agent_view(None, AgentViewEntryOrigin::ImageAdded, ctx)
        }) {
            log::warn!("Failed to enter agent view when adding images: {e}");
        }
    }

    /// Display an error toast for image paste operation failures.
    fn show_image_paste_error(&self, ctx: &mut ViewContext<Self>, message: String) {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_persistent_toast(DismissibleToast::error(message), window_id, ctx);
        });
    }

    /// Check attachment limits, return attachable count (shows toast for excess).
    fn check_image_limits_for_paste(
        &self,
        num_images_to_add: usize,
        ctx: &mut ViewContext<Self>,
    ) -> usize {
        let (num_images_attached, num_images_in_conversation) =
            self.editor.read(ctx, |editor, _| {
                (
                    editor.image_context_options.num_images_attached(),
                    editor.image_context_options.num_images_in_conversation(),
                )
            });

        // Calculate how many images we can add based on per-query limit
        let available_per_query = MAX_IMAGE_COUNT_FOR_QUERY.saturating_sub(num_images_attached);

        // Calculate how many images we can add based on per-conversation limit
        let total_images_current = num_images_attached + num_images_in_conversation;
        let available_per_conversation =
            MAX_IMAGES_PER_CONVERSATION.saturating_sub(total_images_current);

        // Take the more restrictive limit
        let max_attachable = available_per_query.min(available_per_conversation);

        // Determine how many we can actually attach
        let images_to_attach = num_images_to_add.min(max_attachable);
        let excess_images = num_images_to_add.saturating_sub(images_to_attach);

        // Show toast for excess images if any
        if excess_images > 0 {
            let (limit_name, limit_value) = if available_per_query < available_per_conversation {
                ("per query", MAX_IMAGE_COUNT_FOR_QUERY)
            } else {
                ("per conversation", MAX_IMAGES_PER_CONVERSATION)
            };

            let message = if excess_images == 1 {
                format!("1 image wasn't attached - limit is {limit_value} images {limit_name}.")
            } else {
                format!(
                    "{excess_images} images weren't attached - limit is {limit_value} images {limit_name}."
                )
            };
            self.show_image_paste_error(ctx, message);
        }

        images_to_attach
    }

    pub fn set_is_processing_attached_images(
        &mut self,
        is_processing_attached_images: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.is_processing_attached_images = is_processing_attached_images;
        self.update_image_context_options(ctx);
        ctx.notify();
    }

    /// Handles backspace at the buffer boundary (empty buffer or cursor at
    /// position 0). Covers prefix-mode exit (`&` and `!`) and legacy
    /// classic-mode AI icon toggling in a single function.
    fn handle_backspace_at_buffer_boundary(&mut self, ctx: &mut ViewContext<Self>) {
        match self.prefix_mode(ctx) {
            InputPrefixMode::CloudHandoff => {
                self.exit_cloud_handoff_compose(ctx);
                ctx.notify();
                return;
            }
            InputPrefixMode::Shell => {
                if self.agent_view_controller.as_ref(ctx).is_fullscreen() {
                    self.exit_shell_mode_to_ai(ctx);
                    ctx.notify();
                }
                return;
            }
            InputPrefixMode::None => {}
        }

        // Legacy classic-mode AI icon toggling (only reachable when Agent View
        // is inactive and no prefix mode is set).
        if !self.ai_input_model.as_ref(ctx).is_ai_input_enabled() {
            return;
        }

        let is_udi_enabled = InputSettings::as_ref(ctx).is_universal_developer_input_enabled(ctx);
        if is_udi_enabled {
            return;
        }

        // When the agent view is active, the classic-mode AI icon toggling and follow-up clearing
        // logic below does not apply.
        if FeatureFlag::AgentView.is_enabled() && self.agent_view_controller.as_ref(ctx).is_active()
        {
            return;
        }

        // If we have an AI follow up icon, backspace should clear that icon.
        if self
            .ai_context_model
            .as_ref(ctx)
            .is_targeting_existing_conversation(ctx)
        {
            self.ai_context_model.update(ctx, |ai_context_model, ctx| {
                ai_context_model.set_pending_query_state_for_new_conversation(
                    // This origin is unused in this codepath, which doesn't get called when
                    // AgentView is enabled.
                    AgentViewEntryOrigin::Input {
                        was_prompt_autodetected: false,
                    },
                    ctx,
                );
            });
        } else {
            // Otherwise backspace away the AI icon.
            let new_input_type = self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                let new_input_config = ai_input_model.input_config().with_toggled_type().locked();
                let new_input_type = new_input_config.input_type;
                ai_input_model.set_input_config_for_classic_mode(new_input_config, ctx);
                new_input_type
            });
            self.maybe_send_autodetection_telemetry_on_manual_toggle(new_input_type, ctx);
        }
    }

    /// Updates the tab completion menu given the current text of the editor and location of the
    /// cursor. Returns whether the input suggestions should be closed.
    ///
    /// If the original text is still within the buffer up to where the cursor is, we filter the
    /// suggestions to only show the suggestions that match the current word. If the original text
    /// is _not_ within the buffer up to the cursor, we close the input suggestions.
    fn update_tab_completion_menu(
        &self,
        replacement_start: usize,
        buffer_text_original: &str,
        completion_results: &SuggestionResults,
        is_user_edit: bool,
        ctx: &mut ViewContext<Input>,
    ) -> bool {
        let editor_text = self.editor.as_ref(ctx).buffer_text(ctx);
        let cursor_position = self.start_byte_index_of_last_selection(ctx);
        let text_up_to_cursor = &editor_text[0..cursor_position.as_usize()];

        // If the cursor position is before the start of the replacement span,
        // then we should definitely close the menu.
        if cursor_position.as_usize() < replacement_start {
            return true;
        }

        // If the buffer no longer starts with the original buffer text,
        // then we should close the completion menu because the result set
        // was based on a different query.
        //
        // Classic completions get an exemption from this check, but only for
        // system-applied edits: when the completion system cycles through fuzzy
        // matches it rewrites the buffer to each candidate, and the text up to the
        // cursor may no longer start with the original buffer text. Keeping the
        // result set alive in that case is what lets cycling work.
        //
        // A user edit (typing, backspacing, pasting) that diverges from the original
        // query must still invalidate the result set. Otherwise a Tab followed by
        // Backspace past the replacement boundary would leave stale suggestions on
        // screen (and an empty prefix would re-show the entire original result set).
        if !text_up_to_cursor.starts_with(buffer_text_original)
            && (!self.is_classic_completions_enabled(ctx) || is_user_edit)
        {
            // Close the input suggestions since the buffer was edited to no longer
            // contain the text that triggered tab completion.
            true
        } else {
            // The current word is everything from the start of the replacement to the
            // cursor
            let current_word = &editor_text[replacement_start..cursor_position.as_usize()];

            if self.is_classic_completions_enabled(ctx) {
                let current_selected_item =
                    self.input_suggestions.as_ref(ctx).get_selected_item_text();
                if current_selected_item.is_some_and(|selected| selected == current_word) {
                    // If we're in classic completion mode and the selected item is equal
                    // to the current word, then we should keep the menu open; the user is cycling.
                    // We early-return because we don't want to filter the menu based on the
                    // selected item.
                    return false;
                }
            }

            // If the user continues to type with the tab suggestions open, we perform a
            // prefix search on the original results to filter the suggestions.
            let should_close = self.input_suggestions.update(ctx, |suggestions, ctx| {
                suggestions.prefix_search_for_tab_completion(
                    current_word,
                    completion_results,
                    TabCompletionsPreselectOption::Unchanged,
                    ctx,
                );

                // We should close the menu if there aren't any results
                // after filtering.
                suggestions.items().is_empty()
            });

            ctx.notify();
            should_close
        }
    }

    fn clear_screen(&mut self, ctx: &mut ViewContext<Self>) {
        self.model.lock().clear_visible_screen();
        ctx.notify();
    }

    /// Attempts to write the EOT (End-of-Transmission) char to the PTY, which is canonically mapped
    /// to Ctrl-D. If successful, the session is terminated.
    fn ctrl_d(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(Event::CtrlD);
    }

    fn ctrl_r(&mut self, ctx: &mut ViewContext<Self>) {
        if self.suggestions_mode_model.as_ref(ctx).is_history_up() {
            // Iterate through menu if we're already in history substring mode and
            // the user hits ctrl-r.
            self.input_suggestions
                .update(ctx, |input_suggestions, ctx| {
                    input_suggestions.select_prev(ctx);
                });
        } else if !self.ai_input_model.as_ref(ctx).is_ai_input_enabled() {
            self.fuzzy_history_search(ctx);
        }
    }

    fn fuzzy_history_search(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.can_query_history(ctx) {
            return;
        }

        self.focus_input_box(ctx);

        let editor = self.editor.as_ref(ctx);

        let original_cursor_point = editor.single_cursor_to_point(ctx);

        let original_buffer = editor.buffer_text(ctx);
        let original_input_type = self.ai_input_model.as_ref(ctx).input_type();
        let original_input_was_locked = self.ai_input_model.as_ref(ctx).is_input_type_locked();
        self.suggestions_mode_model.update(ctx, |m, ctx| {
            m.set_mode(
                InputSuggestionsMode::HistoryUp {
                    original_buffer,
                    original_cursor_point,
                    search_mode: HistorySearchMode::Fuzzy,
                    original_input_type,
                    original_input_was_locked,
                },
                ctx,
            );
        });

        ctx.notify();
    }

    pub fn on_session_share_joined(
        &mut self,
        replica_id: ReplicaId,
        presence_manager: ModelHandle<PresenceManager>,
        ctx: &mut ViewContext<Self>,
    ) {
        // Shared session history model should only be set if we are a viewer
        debug_assert!(self.model.lock().shared_session_status().is_viewer());
        self.set_shared_session_presence_manager(presence_manager);

        // Set the history model which is only available for a shared session viewer.
        let history_model = ctx.add_model(|_| SharedSessionHistoryModel::new());
        self.shared_session_input_state = Some(SharedSessionInputState {
            history_model,
            pending_command_execution_request: None,
        });

        // Reinitializing for the server replica ID empties the buffer. During cloud setup the
        // sharer's input sync is skipped, so preserve the viewer's in-progress follow-up instead
        // of discarding it.
        let cloud_setup_pre_first_exchange = FeatureFlag::CloudModeSetupV2.is_enabled()
            && is_cloud_agent_pre_first_exchange(
                self.ambient_agent_view_model(),
                &self.agent_view_controller,
                &self.model.lock(),
                ctx,
            );
        let preserved_text = if cloud_setup_pre_first_exchange {
            self.editor().as_ref(ctx).buffer_text(ctx)
        } else {
            String::new()
        };
        self.editor().update(ctx, |editor, ctx| {
            editor.reinitialize_buffer(Some(replica_id), ctx);
            if !preserved_text.is_empty() {
                editor.set_buffer_text(&preserved_text, ctx);
            }
        });
    }

    /// Returns a collection of history entries that are shell commands from
    /// the shared session (run on the sharer's machine).
    fn shared_session_history<'b>(&'b self, ctx: &'b ViewContext<Self>) -> Vec<&'b HistoryEntry> {
        let Some(history_model) = self
            .shared_session_input_state
            .as_ref()
            .map(|state| state.history_model.clone())
        else {
            return Vec::new();
        };

        // TODO: append viewer's local shell history
        history_model.as_ref(ctx).entries().collect()
    }

    /// Returns local command history ordered from oldest to most recent.
    fn command_history_suggestions<'a>(
        &'a self,
        ctx: &'a ViewContext<Self>,
    ) -> Vec<&'a HistoryEntry> {
        History::as_ref(ctx).up_arrow_suggestions(self.active_block_session_id(), ctx)
    }

    fn update_last_word_insertion_state(&mut self) {
        // If an `InsertLastWordPrevCommand` action is received, its handler method will set
        // `is_latest_editor_event` on `self.last_word_insertion` to true, marking the following
        // EditorEvent (buffer edited) received is from this insertion.
        //
        // Any other editor event means the following "last word" insert is not consecutive, so
        // index is reset - the following insert will insert last word from most recent command
        // in history, index 0 (After that, a consecutive insertion would increment to index 1,
        // last word of second last command in history).
        //
        // If the last event was a last word insertion, we increment the
        // `insert_command_from_history_index` on `self.last_word_insertion` to indicate
        // consecutive inserts may be made (if so, insert from next earlier command in history).
        // We then set `is_latest_editor_event` to false for the following editor event; if another
        // last word insertion occurs, it is responsible for re-setting this boolean to true.
        if self.last_word_insertion.is_latest_editor_event {
            self.last_word_insertion.insert_command_from_history_index += 1;
            self.last_word_insertion.is_latest_editor_event = false;
        } else {
            self.last_word_insertion.insert_command_from_history_index = 0;
        }
    }

    fn history_commands<'b>(&self, ctx: &'b ViewContext<Input>) -> Vec<&'b HistoryEntry> {
        self.active_block_session_id()
            .map_or_else(Vec::new, |session_id| {
                History::as_ref(ctx)
                    .commands(session_id)
                    .unwrap_or_default()
            })
    }

    fn insert_last_word_previous_command(&mut self, ctx: &mut ViewContext<Input>) {
        if let Some(word_to_insert) = self.get_last_word_of_command_in_history(
            self.last_word_insertion.insert_command_from_history_index,
            ctx,
        ) {
            self.editor.update(ctx, |editor, ctx| {
                editor.insert_selected_text_to_buffer_ignoring_undo(&word_to_insert, ctx);
            });

            self.last_word_insertion.is_latest_editor_event = true;
        }
    }

    fn get_last_word_of_command_in_history(
        &mut self,
        command_history_index: usize,
        ctx: &mut ViewContext<Input>,
    ) -> Option<String> {
        let commands = self.history_commands(ctx);
        if commands.is_empty() {
            return None;
        }

        let view_command_idx = commands.len().saturating_sub(1 + command_history_index);
        let view_command = commands[view_command_idx];

        let last_word = view_command
            .command
            .rsplit_once(' ')
            .map(|(_, last_word)| last_word)
            .unwrap_or(&view_command.command);

        Some(last_word.to_string())
    }

    /// We only want to show the completions while typing menu when the cursor is
    /// positioned at the end of the buffer text
    fn is_cursor_in_valid_position_for_completions_while_typing(
        &self,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let editor = self.editor.as_ref(ctx);
        editor.single_cursor_at_buffer_end(false /* respect_line_cap */, ctx)
    }

    fn should_show_completions_while_typing(&self, ctx: &mut ViewContext<Self>) -> bool {
        let editor = self.editor.as_ref(ctx);
        let buffer_text = editor.buffer_text(ctx);

        self.is_completions_while_typing_turned_on(ctx)
            && (!self.ai_input_model.as_ref(ctx).is_ai_input_enabled()
                || should_show_completions_in_ai_input(&buffer_text))
            && buffer_text.len() >= MIN_BUFFER_LEN_TO_SHOW_COMPLETIONS_WHILE_TYPING
            && self.is_cursor_in_valid_position_for_completions_while_typing(ctx)
    }

    fn is_completions_while_typing_turned_on(&self, app: &AppContext) -> bool {
        *InputSettings::as_ref(app)
            .completions_open_while_typing
            .value()
    }

    fn is_classic_completions_enabled(&self, ctx: &AppContext) -> bool {
        (FeatureFlag::ClassicCompletions.is_enabled()
            && *InputSettings::as_ref(ctx).classic_completions_mode)
            || FeatureFlag::ForceClassicCompletions.is_enabled()
    }

    fn should_expand_aliases(&self, ctx: &mut ViewContext<Self>) -> bool {
        // Never expand aliases when in AI input mode, regardless of the setting.
        if self.ai_input_model.as_ref(ctx).input_type().is_ai() {
            return false;
        }
        *AliasExpansionSettings::as_ref(ctx)
            .alias_expansion_enabled
            .value()
    }

    fn open_completion_suggestions(
        &mut self,
        completions_trigger: CompletionsTrigger,
        ctx: &mut ViewContext<Self>,
    ) {
        let editor = self.editor.as_ref(ctx);
        let buffer_text = editor.buffer_text(ctx);

        let is_command_grid_active = {
            let model = self.model.lock();
            !model.is_alt_screen_active()
                && model.block_list().active_block().is_command_grid_active()
        };

        // If the cursor is in a valid completion position, go into CompletionSuggestions mode
        if is_command_grid_active && self.can_query_history(ctx) {
            let matcher = MatchStrategy::Fuzzy;

            if let Some(completion_context) = self.completion_session_context(ctx) {
                let cursor_position = self.start_byte_index_of_last_selection(ctx);
                let before_cursor_text = buffer_text[..cursor_position.as_usize()].to_owned();
                let editor_model = self.editor.read(ctx, |view, ctx| view.snapshot_model(ctx));

                self.run_completions_async(
                    before_cursor_text,
                    matcher,
                    completions_trigger,
                    editor_model,
                    cursor_position,
                    completion_context,
                    ctx,
                );
            }
        }
    }

    /// _Asynchronously_ generates completions by calling into the completer.
    #[allow(clippy::too_many_arguments)]
    fn run_completions_async(
        &mut self,
        before_cursor_text: String,
        matcher: MatchStrategy,
        completions_trigger: CompletionsTrigger,
        editor_snapshot: EditorSnapshot,
        cursor_position: ByteOffset,
        completion_context: SessionContext,
        ctx: &mut ViewContext<'_, Input>,
    ) {
        let buffer_text = self.buffer_text(ctx);
        let input_type = self.ai_input_model.as_ref(ctx).input_type();

        let comp_sources = {
            let input_settings = InputSettings::as_ref(ctx);
            resolve_completion_sources(
                FeatureFlag::NativeShellCompletions.is_enabled(),
                input_type.is_ai(),
                buffer_text.contains('\n'),
                completions_trigger,
                *input_settings.warp_completions_enabled,
                *input_settings.native_shell_completions_enabled,
            )
        };

        let fallback_strategy = {
            let use_native_shell_completions = comp_sources.uses_native();
            match completions_trigger {
                CompletionsTrigger::Keybinding if !use_native_shell_completions => {
                    CompletionsFallbackStrategy::FilePaths
                }
                _ => CompletionsFallbackStrategy::None,
            }
        };

        if self.is_completions_while_typing_turned_on(ctx)
            && let Some(last_abort_handle) = self.completions_abort_handle.take()
        {
            last_abort_handle.abort();
        }

        // Don't trigger completions if the last character typed is whitespace, in AI input mode.
        // The user is likely typing in a natural language word at this point, not a filepath.
        if input_type.is_ai()
            && completions_trigger == CompletionsTrigger::AsYouType
            && before_cursor_text.ends_with(char::is_whitespace)
        {
            return;
        }

        let Some(session_id) = self.completer_data().active_block_session_id() else {
            return;
        };
        let session_env_vars = self.sessions.read(ctx, |sessions, _| {
            sessions.get_env_vars_for_session(session_id)
        });

        let cursor_position = cursor_position.as_usize();

        if comp_sources == CompletionSources::None {
            if let Some(last_abort_handle) = self.completions_abort_handle.take() {
                last_abort_handle.abort();
            }
            return;
        }

        if comp_sources == CompletionSources::WarpThenNative {
            let completion_session = completion_context.session.clone();
            let abort_handle = ctx
                .spawn_abortable(
                    async move {
                        let spec_suggestions = completer::suggestions(
                            before_cursor_text.as_str(),
                            cursor_position,
                            session_env_vars.as_ref(),
                            CompleterOptions {
                                match_strategy: matcher,
                                fallback_strategy,
                                suggest_file_path_completions_only: input_type.is_ai(),
                                parse_quotes_as_literals: input_type.is_ai(),
                            },
                            &completion_context,
                        )
                        .await;
                        (
                            spec_suggestions,
                            completions_trigger,
                            editor_snapshot,
                            completion_context,
                            session_env_vars,
                        )
                    },
                    move |input,
                          (
                        spec_suggestions,
                        completions_trigger,
                        editor_snapshot,
                        completion_context,
                        session_env_vars,
                    ),
                          ctx| {
                        let bundled_specs_empty = match &spec_suggestions {
                            Some(spec_suggestions) => spec_suggestions.suggestions.is_empty(),
                            None => true,
                        };
                        if bundled_specs_empty {
                            // Phase two: the bundled specs produced nothing, so ask the shell now.
                            input.dispatch_native_shell_completions(
                                buffer_text,
                                cursor_position,
                                matcher,
                                completion_context,
                                session_env_vars,
                                completions_trigger,
                                editor_snapshot,
                                ctx,
                            );
                        } else {
                            // A bundled spec won; the shell is never asked.
                            input.handle_completion_suggestions_results(
                                spec_suggestions,
                                completions_trigger,
                                editor_snapshot,
                                ctx,
                            );
                        }
                    },
                    move |_, _| {
                        completion_session.cancel_active_commands();
                    },
                )
                .abort_handle();
            self.completions_abort_handle = Some(abort_handle);
            return;
        }

        let dispatch_native_up_front = comp_sources == CompletionSources::NativeOnly;
        let native_results_fut = if dispatch_native_up_front {
            // If we're using native shell completions, construct a future that
            // will be resolved with any completions data provided by the shell.
            let (results_tx, results_rx) = async_channel::unbounded();
            ctx.dispatch_typed_action(&TerminalAction::RunNativeShellCompletions {
                buffer_text: buffer_text[0..cursor_position].to_owned(),
                results_tx,
            });
            async move { results_rx.recv().await.ok() }.boxed()
        } else {
            // If not, we can immediately say that there are no completion
            // results from the shell.
            futures::future::ready(None).boxed()
        };

        let completion_session = completion_context.session.clone();

        let abort_handle = ctx
            .spawn_abortable(
                async move {
                    if comp_sources == CompletionSources::NativeOnly {
                        let native_suggestions =
                            native_results_fut
                                .await
                                .map(|(results, shell_replacement_span)| {
                                    native_shell_suggestion_results(
                                        results,
                                        shell_replacement_span,
                                        &buffer_text,
                                        cursor_position,
                                    )
                                });
                        let suggestions = match native_suggestions {
                            Some(suggestions) if suggestions.suggestions.is_empty() => {
                                completer::suggestions(
                                    before_cursor_text.as_str(),
                                    cursor_position,
                                    session_env_vars.as_ref(),
                                    CompleterOptions {
                                        match_strategy: matcher,
                                        fallback_strategy: CompletionsFallbackStrategy::FilePaths,
                                        suggest_file_path_completions_only: true,
                                        parse_quotes_as_literals: false,
                                    },
                                    &completion_context,
                                )
                                .await
                            }
                            suggestions => suggestions,
                        };
                        return (suggestions, completions_trigger, editor_snapshot);
                    }
                    let suggestions = completer::suggestions(
                        before_cursor_text.as_str(),
                        cursor_position,
                        session_env_vars.as_ref(),
                        CompleterOptions {
                            match_strategy: matcher,
                            fallback_strategy,
                            suggest_file_path_completions_only: input_type.is_ai(),
                            parse_quotes_as_literals: input_type.is_ai(),
                        },
                        &completion_context,
                    )
                    .await;

                    let suggestions = match suggestions {
                        Some(s) if !s.suggestions.is_empty() => Some(s),
                        _ => native_results_fut
                            .await
                            .map(|(results, shell_replacement_span)| {
                                native_shell_suggestion_results(
                                    results,
                                    shell_replacement_span,
                                    &buffer_text,
                                    cursor_position,
                                )
                            }),
                    };

                    (suggestions, completions_trigger, editor_snapshot)
                },
                |input, (suggestions, completions_trigger, editor_model), ctx| {
                    input.handle_completion_suggestions_results(
                        suggestions,
                        completions_trigger,
                        editor_model,
                        ctx,
                    )
                },
                move |_, _| {
                    completion_session.cancel_active_commands();
                },
            )
            .abort_handle();

        self.completions_abort_handle = Some(abort_handle);
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_native_shell_completions(
        &mut self,
        buffer_text: String,
        cursor_position: usize,
        matcher: MatchStrategy,
        completion_context: SessionContext,
        session_env_vars: Option<HashMap<String, String>>,
        completions_trigger: CompletionsTrigger,
        editor_snapshot: EditorSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        // If the buffer moved on while the spec pass ran, this request is stale.
        let current_editor_model = self
            .editor
            .read(ctx, |editor, ctx| editor.snapshot_model(ctx));
        let buffer_text_now = self.editor.as_ref(ctx).buffer_text(ctx);
        if buffer_text_now != editor_snapshot.text()
            || current_editor_model.selections() != editor_snapshot.selections()
        {
            return;
        }

        let (results_tx, results_rx) = async_channel::unbounded();
        ctx.dispatch_typed_action(&TerminalAction::RunNativeShellCompletions {
            buffer_text: buffer_text[0..cursor_position].to_owned(),
            results_tx,
        });
        let completion_session = completion_context.session.clone();

        let abort_handle = ctx
            .spawn_abortable(
                async move {
                    let native_suggestions = results_rx.recv().await.ok().map(|(results, span)| {
                        native_shell_suggestion_results(
                            results,
                            span,
                            &buffer_text,
                            cursor_position,
                        )
                    });
                    let suggestions = match native_suggestions {
                        Some(suggestions) if suggestions.suggestions.is_empty() => {
                            completer::suggestions(
                                &buffer_text[..cursor_position],
                                cursor_position,
                                session_env_vars.as_ref(),
                                CompleterOptions {
                                    match_strategy: matcher,
                                    fallback_strategy: CompletionsFallbackStrategy::FilePaths,
                                    suggest_file_path_completions_only: true,
                                    parse_quotes_as_literals: false,
                                },
                                &completion_context,
                            )
                            .await
                        }
                        suggestions => suggestions,
                    };
                    (suggestions, completions_trigger, editor_snapshot)
                },
                |input, (suggestions, completions_trigger, editor_model), ctx| {
                    input.handle_completion_suggestions_results(
                        suggestions,
                        completions_trigger,
                        editor_model,
                        ctx,
                    );
                },
                move |_, _| {
                    completion_session.cancel_active_commands();
                },
            )
            .abort_handle();
        self.completions_abort_handle = Some(abort_handle);
    }

    /// Asynchronously generates dynamic enum suggestions.
    fn get_enum_suggestions_async(
        &mut self,
        command: String,
        editor_snapshot: EditorSnapshot,
        ctx: &mut ViewContext<'_, Input>,
    ) {
        if let Some(completion_context) = self.completion_session_context(ctx) {
            self.suggestions_mode_model.update(ctx, |m, ctx| {
                m.set_dynamic_enum_status(DynamicEnumSuggestionStatus::Pending, ctx);
            });
            let abort_handle = ctx
                .spawn(
                    async move {
                        let variants = super::dynamic_enum_suggestions::run_dynamic_enum_command(
                            command.as_str(),
                            &completion_context,
                        )
                        .await;

                        (variants, editor_snapshot)
                    },
                    move |input, (variants, editor_model), ctx| {
                        input.handle_enum_completion_results(variants, editor_model, ctx);
                    },
                )
                .abort_handle();

            self.completions_abort_handle = Some(abort_handle);
            ctx.notify();
        }
    }

    /// When the command finishes running, update the input suggestions menu with the suggestions.
    fn handle_enum_completion_results(
        &mut self,
        results: anyhow::Result<Vec<String>>,
        editor_snapshot_when_completer_was_ran: EditorSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        let current_editor_model = self
            .editor
            .read(ctx, |editor, ctx| editor.snapshot_model(ctx));

        let buffer_text = self.editor.as_ref(ctx).buffer_text(ctx);
        // If the editor has changed since the completions trigger was hit-- noop since the
        // suggestions are no longer valid. Note that we purposely ignore attributes such as text
        // styles for the purposes of this check (we only care about the buffer text content and
        // the cursor selections state).
        if buffer_text != editor_snapshot_when_completer_was_ran.text()
            || current_editor_model.selections()
                != editor_snapshot_when_completer_was_ran.selections()
        {
            return;
        }

        let (variants, status) = match results {
            Ok(variants) => (variants, DynamicEnumSuggestionStatus::Success),
            Err(e) => {
                log::warn!("Failed to generate dynamic enum suggestions: {e:?}");
                (vec![], DynamicEnumSuggestionStatus::Failure)
            }
        };

        self.input_suggestions.update(ctx, |input, ctx| {
            input.set_enum_variants(variants.clone(), ctx);
        });

        if let InputSuggestionsMode::DynamicWorkflowEnumSuggestions {
            menu_position,
            selected_ranges,
            cursor_point,
            command,
            ..
        } = self.suggestions_mode_model.as_ref(ctx).mode()
        {
            let updated_mode = InputSuggestionsMode::DynamicWorkflowEnumSuggestions {
                dynamic_enum_status: status,
                suggestions: variants,
                menu_position: *menu_position,
                selected_ranges: selected_ranges.clone(),
                cursor_point: *cursor_point,
                command: command.clone(),
            };
            self.suggestions_mode_model.update(ctx, |model, ctx| {
                model.set_mode(updated_mode, ctx);
            });
        }

        ctx.notify();
    }

    fn path_separators(&self, ctx: &AppContext) -> PathSeparators {
        self.active_session(ctx)
            .map(|session| session.path_separators())
            .unwrap_or(PathSeparators::for_os())
    }

    /// Returns the buffer point that the tab completion menu should be positioned relative to.
    /// If None, the menu should be positioned relative to the cursor.
    ///
    /// In regular completions mode, we want to dock the completions menu at the cursor.
    ///
    /// In classic completions mode, we want to dock the completions menu at the start of
    /// the replacement span*. This ensures that the menu doesn't jump around as the cursor
    /// moves when the user cycles through items in the menu.
    /// * The one edge case is when we're completing a file path. In this case, the menu
    ///   should be docked at the end of the last directory in the replacement span.
    ///   This is because the replacement span will include the entire file path.
    ///   For example, if the user types "cd app/D" and one of the completion display result is
    ///   "Documents", then the replacement span will be for "app/D" and the replacement will
    ///   be "app/Documents".
    fn tab_completions_menu_position(
        &self,
        results: &SuggestionResults,
        buffer_text_original: &str,
        ctx: &AppContext,
    ) -> Option<BufferPoint> {
        // In regular mode, the menu should be positioned at the cursor.
        if !self.is_classic_completions_enabled(ctx) {
            return None;
        }

        // Note: the replacement span is in terms of byte offsets.
        // But these byte offsets should correspond to valid char offsets.
        let start = results.replacement_span.start();
        let end = results.replacement_span.end();

        let all_results_are_file_completions = results
            .suggestions
            .iter()
            .all(|s| s.suggestion.file_type.is_some());

        let offset = if all_results_are_file_completions {
            // If all the results are file completions, let's find the last slash in the replacement
            // span and dock the completions menu right after it. We do this because the replacement
            // span of file path completions is relative to the beginning of the file path. For
            // example, if the user types "cd app/D" and one of the completion display result is
            // "Documents", then the replacement span will be for "app/D" and the replacement will
            // be "app/Documents".
            buffer_text_original
                .get(0..end)
                .and_then(|s| s.rfind(self.path_separators(ctx).all))
                .map(|i| i + 1)
                .unwrap_or(start)
        } else {
            start
        };

        let point = self
            .editor
            .as_ref(ctx)
            .point_for_offset(ByteOffset::from(offset), ctx);
        point.ok()
    }

    fn handle_completion_suggestions_results(
        &mut self,
        results: Option<SuggestionResults>,
        completions_trigger: CompletionsTrigger,
        editor_snapshot_when_completer_was_ran: EditorSnapshot,
        ctx: &mut ViewContext<Self>,
    ) {
        let current_editor_model = self
            .editor
            .read(ctx, |editor, ctx| editor.snapshot_model(ctx));

        let buffer_text = self.editor.as_ref(ctx).buffer_text(ctx);
        // If the editor has changed since the completions trigger was hit-- noop since the
        // suggestions are no longer valid. Note that we purposely ignore attributes such as text
        // styles for the purposes of this check (we only care about the buffer text content and
        // the cursor selections state).
        if buffer_text != editor_snapshot_when_completer_was_ran.text()
            || current_editor_model.selections()
                != editor_snapshot_when_completer_was_ran.selections()
        {
            return;
        }

        match results {
            None => {
                // It's necessary to specifically set to closed in the case where we first
                // opened the tab menu and then keep typing
                self.suggestions_mode_model.update(ctx, |m, ctx| {
                    m.set_mode(InputSuggestionsMode::Closed, ctx);
                });
            }
            Some(results) if results.suggestions.is_empty() => {
                self.suggestions_mode_model.update(ctx, |m, ctx| {
                    m.set_mode(InputSuggestionsMode::Closed, ctx);
                });
            }
            Some(results) => {
                let query = results.replacement_span.slice(&buffer_text);
                let buffer_text_original = buffer_text
                    [0..self.start_byte_index_of_last_selection(ctx).as_usize()]
                    .to_string();
                let decision = if completions_trigger == CompletionsTrigger::Keybinding {
                    results.explicit_tab_completion(query, self.path_separators(ctx).all)
                } else {
                    ExplicitTabCompletion::Open {
                        suggestions: results
                            .prepare_for_query(query, self.path_separators(ctx).all),
                        replacement_span: results.replacement_span,
                    }
                };
                let prepared_suggestions: Vec<PreparedSuggestion> = match decision {
                    ExplicitTabCompletion::NoAction => {
                        self.suggestions_mode_model.update(ctx, |model, ctx| {
                            model.set_mode(InputSuggestionsMode::Closed, ctx);
                        });
                        ctx.notify();
                        return;
                    }
                    ExplicitTabCompletion::InsertSingle {
                        suggestion,
                        replacement_span,
                    } => {
                        self.insert_completion_result_into_editor(
                            &suggestion.suggestion.replacement,
                            replacement_span.start(),
                            Executing::No,
                            ctx,
                        );
                        ctx.notify();
                        return;
                    }
                    ExplicitTabCompletion::InsertCommonPrefixAndOpen {
                        common_prefix,
                        suggestions,
                        replacement_span,
                    } => {
                        self.insert_completion_prefix_into_editor(
                            ctx,
                            &common_prefix,
                            replacement_span.start(),
                        );
                        suggestions
                    }
                    ExplicitTabCompletion::Open { suggestions, .. } => suggestions,
                };

                // If not using completions as you type, then
                // clear any autosuggestions when tab completions are open.
                // The autosuggestion will be repopulated when the menu is closed.
                // We don't do this for completions as you type because the user would
                // otherwise hardly see autosuggestons.
                if FeatureFlag::RemoveAutosuggestionDuringTabCompletions.is_enabled()
                    && !self.is_completions_while_typing_turned_on(ctx)
                {
                    self.editor.update(ctx, |view, ctx| {
                        view.clear_autosuggestion(ctx);
                    });
                }

                // Decide where to render the tab completion menu.
                // If we're rendering it at a specific position, let's make sure
                // that position exists in the position cache.
                let position =
                    self.tab_completions_menu_position(&results, &buffer_text_original, ctx);
                let menu_position = if let Some(position) = position {
                    self.editor.update(ctx, |editor, ctx| {
                        editor.cache_buffer_point(
                            position,
                            COMPLETIONS_START_OF_REPLACEMENT_SPAN_POSITION_ID,
                            ctx,
                        );
                    });
                    TabCompletionsMenuPosition::AtStartOfReplacementSpan
                } else {
                    TabCompletionsMenuPosition::AtLastCursor
                };

                self.suggestions_mode_model.update(ctx, |model, ctx| {
                    model.set_mode(
                        InputSuggestionsMode::CompletionSuggestions {
                            replacement_start: results.replacement_span.start(),
                            buffer_text_original,
                            completion_results: results.clone(),
                            trigger: completions_trigger,
                            menu_position,
                        },
                        ctx,
                    );
                });

                let preselect_option = if self.is_classic_completions_enabled(ctx) {
                    TabCompletionsPreselectOption::Unselected
                } else {
                    TabCompletionsPreselectOption::First
                };

                self.input_suggestions
                    .update(ctx, |input_suggestions, ctx| {
                        input_suggestions.set_prepared_tab_completions(
                            prepared_suggestions,
                            preselect_option,
                            ctx,
                        );
                    });
            }
        }
        ctx.notify();
    }

    /// Replace the replacement with the common completion prefix. Note that completion prefix
    /// itself is not the completion result so we don't add a space.
    fn insert_completion_prefix_into_editor(
        &mut self,
        ctx: &mut ViewContext<Input>,
        completion_prefix: &str,
        replacement_start: usize,
    ) {
        let completion_prefix = strip_control_characters(completion_prefix);
        self.editor.update(ctx, |input, ctx| {
            let cursor_end_offset = input.end_byte_index_of_last_selection(ctx);
            input.select_and_replace(
                &completion_prefix,
                [ByteOffset::from(replacement_start)..cursor_end_offset],
                PlainTextEditorViewAction::AcceptCompletionSuggestion,
                ctx,
            );
        });
    }

    /// Replace the replacement with the completion result and potentially add a space after.
    fn insert_completion_result_into_editor(
        &mut self,
        completion_result: &str,
        replacement_start: usize,
        executing: Executing,
        ctx: &mut ViewContext<Input>,
    ) {
        let completion_result = strip_control_characters(completion_result);
        let is_completions_as_you_type_enabled = self.is_completions_while_typing_turned_on(ctx);
        self.editor.update(ctx, |input, ctx| {
            let cursor_end_offset = input.end_byte_index_of_last_selection(ctx);

            // Add a space to the end if the end of the selection/replacement is at the end of the
            // buffer and the completion result doesn't end with a slash or an equals sign. A
            // trailing slash means more of a path follows; a trailing `=` (e.g. `--color=`) means a
            // value follows directly, as shells' own `-S '='` completions do.
            // If completions as you type is turned on and classic completions is off, then
            // _don't_ add a space.
            let is_classic_completions_enabled = self.is_classic_completions_enabled(ctx);
            let replacement: Cow<str> = if (!is_completions_as_you_type_enabled
                || is_classic_completions_enabled)
                && cursor_end_offset.as_usize() == input.buffer_text(ctx).len()
                && !completion_result.ends_with(self.path_separators(ctx).main)
                && !completion_result.ends_with('=')
                && executing == Executing::No
            {
                format!("{completion_result} ").into()
            } else {
                completion_result.clone()
            };

            input.select_and_replace(
                &replacement,
                [ByteOffset::from(replacement_start)..cursor_end_offset],
                PlainTextEditorViewAction::AcceptCompletionSuggestion,
                ctx,
            );
        });
    }

    /// Whether the editor is in a state where we should tab complete instead of indenting text
    /// within the editor.
    /// The editor is considered in a state where we should tab complete if:
    ///     1) The buffer text is not empty.
    ///     2) The user is not actively selecting.
    ///     3) There is only a single selection and that selection does not take up the entire
    ///        buffer.
    fn cursor_positioned_for_completion(&self, ctx: &mut ViewContext<Self>) -> bool {
        let input = self.editor.as_ref(ctx);
        let buffer_text = input.buffer_text(ctx);

        // We can show the completion menu when there is a single cursor selection
        // and we aren't actively selecting.
        !buffer_text.trim_start().is_empty()
            && !input.is_selecting(ctx)
            && input.num_selections(ctx) == 1
            && !input.any_selections_span_entire_buffer(ctx)
    }

    /// Returns the index of the argument our cursor is currently on, if there is one,
    /// as well as any style runs computed for reuse in `highlight_selected_workflow_argument`
    fn get_current_argument(
        &self,
        ctx: &ViewContext<Self>,
    ) -> (Option<WorkflowArgumentIndex>, Vec<Range<ByteOffset>>) {
        // If we aren't in a workflow, return
        let Some(workflow_state) = &self.workflows_state.selected_workflow_state else {
            report_error!(anyhow::anyhow!(
                "Tried to get the current argument when no workflow is loaded into the input",
            ));
            return (None, Vec::new());
        };

        let cursor_position = self
            .editor
            .as_ref(ctx)
            .end_byte_index_of_last_selection(ctx);

        // Get the highlighted text style ranges, which are used to determine where the workflow arguments are
        let text_style_ranges = self.get_text_style_ranges_for_workflow(ctx);

        // Find a text range that contains the cursor position
        let highlight_index = text_style_ranges
            .iter()
            .position(|range| range.contains(&cursor_position));

        // Find the argument that corresponds with this highlight index
        let arg_index = highlight_index.and_then(|index| {
            workflow_state
                .argument_index_to_highlight_index
                .iter()
                .find(|(_, highlight)| highlight.contains(&index))
                .map(|(arg_index, _)| *arg_index)
        });

        (arg_index, text_style_ranges)
    }

    fn input_shift_tab(&mut self, ctx: &mut ViewContext<Self>) {
        // In CompletionSuggestions mode, shift tab moves to the previous selection.
        if let InputSuggestionsMode::CompletionSuggestions { .. } =
            self.suggestions_mode_model.as_ref(ctx).mode()
        {
            self.input_suggestions.update(ctx, |suggestions, ctx| {
                suggestions.select_prev(ctx);
            });
            return;
        }

        if let Some(workflows_info_view) = &self
            .workflows_state
            .selected_workflow_state
            .as_ref()
            .map(|state| &state.more_info_view)
        {
            // Get the index of the argument we are currently selecting, if it exists
            let (current_argument, text_style_ranges) = self.get_current_argument(ctx);

            workflows_info_view.update(ctx, |info_view, ctx| {
                // If we are selecting an argument, open that one
                if let Some(index) = current_argument {
                    info_view.selected_workflow_state.set_argument_index(index);
                }
                // If we were in history suggestion mode, select the first argument
                else if matches!(
                    self.suggestions_mode_model.as_ref(ctx).mode(),
                    InputSuggestionsMode::HistoryUp { .. }
                ) {
                    info_view
                        .selected_workflow_state
                        .set_argument_index(0.into());
                }
                // Otherwise, continue to cycle arguments
                else {
                    info_view.selected_workflow_state.increment_argument_index();
                }

                ctx.notify();
            });

            self.highlight_selected_workflow_argument(text_style_ranges, ctx);

            if let Some(a11y_text) = self.selected_workflow_a11y_text(ctx) {
                ctx.emit_a11y_content(AccessibilityContent::new_without_help(
                    a11y_text,
                    WarpA11yRole::UserAction,
                ));
            }
        } else {
            self.editor.update(ctx, |input, ctx| input.unindent(ctx));
        }
    }

    pub fn completion_session_context(&self, ctx: &AppContext) -> Option<SessionContext> {
        self.active_block_session_id()
            .and_then(|active_block_session_id| {
                let current_session = self.sessions.as_ref(ctx).get(active_block_session_id);
                let pwd = self
                    .active_block_metadata
                    .as_ref()
                    .and_then(BlockMetadata::current_working_directory)
                    .map(str::to_owned);

                current_session.zip(pwd).map(|(current_session, pwd)| {
                    // TODO(abhishek): Ideally, BlockMetadata::current_working_directory should directly
                    // return a TypedPathBuf. This shouldn't happen here in the view.
                    let current_working_directory =
                        current_session.convert_directory_to_typed_path_buf(pwd);
                    SessionContext::new(
                        current_session,
                        CommandRegistry::global_instance(),
                        current_working_directory,
                    )
                })
            })
    }

    pub fn active_session(&self, ctx: &AppContext) -> Option<Arc<Session>> {
        self.active_block_session_id()
            .and_then(|active_block_session_id| {
                self.sessions.as_ref(ctx).get(active_block_session_id)
            })
    }

    fn hide_x_ray(&mut self, ctx: &mut ViewContext<Self>) {
        if self.command_x_ray_description.take().is_some() {
            self.editor.update(ctx, |editor, ctx| {
                editor.clear_command_x_ray();
                ctx.notify();
            });
            ctx.notify();
        }
    }

    fn start_xray_at_offset(
        &mut self,
        pos: ByteOffset,
        trigger: CommandXRayTrigger,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(completion_context) = self.completion_session_context(ctx) {
            let buffer_text = self.buffer_text(ctx);
            let _ =
                ctx.spawn(
                    async move {
                        completer::describe(buffer_text.as_str(), pos, &completion_context).await
                    },
                    |input, description, ctx| {
                        input.show_xray(description, trigger, ctx);
                    },
                );
        }
    }

    fn show_xray(
        &mut self,
        description: Option<Description>,
        trigger: CommandXRayTrigger,
        ctx: &mut ViewContext<'_, Self>,
    ) {
        let description = description.map(Arc::new);
        self.command_x_ray_description.clone_from(&description);
        if let Some(description) = description {
            if trigger == CommandXRayTrigger::Keystroke {
                ctx.emit_a11y_content(AccessibilityContent::new_without_help(
                    description.a11y_text(),
                    WarpA11yRole::UserAction,
                ));
            }
            ctx.notify();
            self.editor.update(ctx, move |editor, ctx| {
                editor.set_command_x_ray(description);
                ctx.notify();
            });
        }
        ctx.notify();
    }

    fn active_block_session_id(&self) -> Option<SessionId> {
        self.active_block_metadata
            .as_ref()
            .and_then(BlockMetadata::session_id)
    }

    /// Handles a tab keypress from the editor.
    ///
    /// "Tab" is the default trigger to open the completion suggestions menu, but this may be
    /// overridden in settings. If the completion suggestions menu is already open, tab and
    /// shift-tab are used to select the next and previous suggestion, respectively -- this is not
    /// overridable; note that even if "open completion suggestions menu" is rebound to a non-tab
    /// key, tab and shift-tab are still used to navigate within the menu once it is open.
    ///
    /// If tab is not bound to "open completion suggestions menu" nor is the suggestions menu
    /// already open, inserts a tab char into the input editor.
    fn input_tab(&mut self, ctx: &mut ViewContext<Self>) {
        // We have to manually check if "tab" is bound to
        // `InputAction::MaybeOpenCompletionSuggestions` here because the child `EditorView`
        // handles the actual tab keypress event -- the handler method attached to the
        // `EditableBinding` for `MaybeOpenCompletionSuggestions` is not called when the
        // binding is tab because the UI framework dictates that only one View may receive a
        // keypress event.
        let is_tab_bound_to_open_completions =
            bindings::keybinding_name_to_keystroke(OPEN_COMPLETIONS_KEYBINDING_NAME, ctx)
                .map(|keystroke| keystroke.key == "tab")
                .unwrap_or_default();

        let replacement_start_opt = if let InputSuggestionsMode::CompletionSuggestions {
            replacement_start,
            ..
        } = self.suggestions_mode_model.as_ref(ctx).mode()
        {
            Some(*replacement_start)
        } else {
            None
        };
        if let Some(replacement_start) = replacement_start_opt {
            // The completions menu is already open, in which there are two cases.
            // Case 1: There is a common prefix amongst filtered suggestions that we could fill; so
            //         we fill it in buffer.
            // Case 2: Else, tab should move to next option.
            let (common_prefix_of_filtered_suggestions, is_single_prefix_suggestion) =
                self.input_suggestions.read(ctx, |suggestions, _| {
                    // Ignore fuzzy matches when calculating longest common
                    // prefix of suggestions. So even if there are fuzzy
                    // matches, we can find a common prefix and try to insert it.
                    let suggestion_texts = suggestions
                        .items()
                        .iter()
                        .filter(|item| {
                            matches!(
                                item.match_type(),
                                MatchType::Prefix {
                                    is_case_sensitive: true
                                } | MatchType::Exact {
                                    is_case_sensitive: true
                                }
                            )
                        })
                        .map(|item| item.text())
                        .collect_vec();
                    let num_suggestions = suggestion_texts.len();
                    (
                        longest_common_prefix(suggestion_texts).map(|x| x.to_owned()),
                        num_suggestions == 1,
                    )
                });
            if let Some(common_prefix) = common_prefix_of_filtered_suggestions {
                let input_text = self.editor.as_ref(ctx).buffer_text(ctx);
                // Determine the current word in the editor that will be replaced by the tab
                // completion. We use the start index of the selection since the completer only sees
                // the text up to the start of the selection when generating completion results.
                let current_word = &input_text
                    [replacement_start..self.start_byte_index_of_last_selection(ctx).as_usize()];

                // Insert the common prefix if it is longer than what the user has currently typed
                // This check is necessary because the suggestions are case-insensitive, while the
                // common prefix logic is necessarily case-sensitive. That can lead to the common
                // prefix being shorter, causing confusing behavior where the input is shortened.
                // Also, we check if the replacement
                if common_prefix.len() > current_word.len()
                    && common_prefix.starts_with(current_word)
                {
                    self.insert_completion_prefix_into_editor(
                        ctx,
                        &common_prefix,
                        replacement_start,
                    );
                    // If there was only a single completion remaining and we just inserted it into the editor,
                    // close the completions menu.
                    if is_single_prefix_suggestion {
                        self.close_input_suggestions(true, ctx)
                    }
                    return;
                }
            }
            self.input_suggestions.update(ctx, |suggestions, ctx| {
                suggestions.select_next(ctx);
            });
        } else if matches!(
            self.suggestions_mode_model.as_ref(ctx).mode(),
            InputSuggestionsMode::StaticWorkflowEnumSuggestions { .. }
                | InputSuggestionsMode::DynamicWorkflowEnumSuggestions { .. }
        ) {
            self.input_suggestions.update(ctx, |suggestions, ctx| {
                suggestions.select_next(ctx);
            });
        } else if is_tab_bound_to_open_completions && self.cursor_positioned_for_completion(ctx) {
            self.open_completion_suggestions(CompletionsTrigger::Keybinding, ctx);
        } else {
            // Otherwise, pass the tab down to the editor
            self.editor.update(ctx, |input, ctx| input.handle_tab(ctx));
        }
    }

    /// Opens the completion suggestions menu if the cursor is in a valid position to generate
    /// suggestions and the menu is not already open.
    ///
    /// This is called when [`InputAction::MaybeOpenCompletionSuggestions`] is bound to a non-tab
    /// key; tab is the default binding. This is _not_ called when the binding is set to the
    /// default ("tab") because the tab keypress event is actually handled by the child
    /// [`Editor`] view, so the tab event is never actually propagated to this input view. Instead,
    /// the logic to open the completions menu when tab bound is implemented in
    /// [`Self::input_tab()`], which is called when the editor emits an
    /// `EditorEvent::Navigate(NavigationKey::Tab)`.
    ///
    /// Ultimately this weirdness is due to limitations in the UI framework preventing multiple
    /// `View`s from handling/responding to the same `Event`.
    fn maybe_open_completion_suggestions(&mut self, ctx: &mut ViewContext<Self>) {
        if !matches!(
            self.suggestions_mode_model.as_ref(ctx).mode(),
            InputSuggestionsMode::CompletionSuggestions { .. },
        ) && self.cursor_positioned_for_completion(ctx)
        {
            self.open_completion_suggestions(CompletionsTrigger::Keybinding, ctx);
        }
    }

    #[cfg(test)]
    fn user_insert(&mut self, text: &str, ctx: &mut ViewContext<Self>) -> bool {
        self.insert_internal(text, EditOrigin::UserTyped, ctx)
    }

    pub fn user_replace_editor_text(&mut self, text: &str, ctx: &mut ViewContext<Self>) -> bool {
        self.editor.update(ctx, |editor, ctx| {
            editor.select_all(ctx);
        });
        self.insert_internal(text, EditOrigin::UserTyped, ctx)
    }

    // It's the responsibility of the caller to ensure that the text submitted here
    // should be inputted into the input area (i.e. arrow keys should not be
    // included in the string).
    pub fn system_insert(&mut self, text: &str, ctx: &mut ViewContext<Self>) -> bool {
        self.insert_internal(text, EditOrigin::UserInitiated, ctx)
    }

    pub fn has_pending_command(&self) -> bool {
        self.has_pending_command
    }

    pub fn set_pending_command(&mut self, exec: &str, ctx: &mut ViewContext<Self>) {
        self.has_pending_command = true;
        self.system_insert(exec, ctx);
    }

    fn should_enter_accept_completion_suggestion(&self, app: &AppContext) -> bool {
        let InputSuggestionsMode::CompletionSuggestions {
            replacement_start, ..
        } = self.suggestions_mode_model.as_ref(app).mode()
        else {
            return false;
        };
        let completions_while_typing = self.is_completions_while_typing_turned_on(app);
        let selected_item = self.input_suggestions.as_ref(app).get_selected_item_text();

        // If classic completions is enabled, accept the suggestion if an item is selected.
        if self.is_classic_completions_enabled(app) {
            return self
                .input_suggestions
                .as_ref(app)
                .get_selected_item()
                .is_some();
        }
        // If completions as you type is disabled, accept the suggestion if an item is selected.
        if !completions_while_typing {
            return selected_item.is_some();
        }

        let path_separators = self.path_separators(app).all;

        // At this point, we know completions as you type is enabled and classic completions
        // is disabled. Accept the completion unless the buffer already matches the selected item
        // (in which case, just execute the command).
        let current_buffer_text = self.editor.as_ref(app).buffer_text(app);
        selected_item.is_none_or(|selected_item| {
            let Some(replacement) = &current_buffer_text.get(*replacement_start..) else {
                report_error!("Failed to get replacement range in current buffer text");
                return true;
            };
            if replacement == &selected_item {
                return false;
            }
            let Some(no_slash) = selected_item.strip_suffix(path_separators) else {
                return true;
            };
            replacement != &no_slash
        })
    }

    /// Determines whether to insert a newline in the buffer instead of executing a command
    /// when enter is pressed.
    fn should_insert_newline_on_enter(&self, ctx: &AppContext) -> bool {
        let editor = self.editor.as_ref(ctx);
        let shell_family = editor.shell_family();
        editor.chars_preceding_selections(ctx).any(|chars| {
            let mut preceding_chars = chars.rev();
            while let Some(c) = preceding_chars.next() {
                match shell_family {
                    Some(ShellFamily::PowerShell) => {
                        if c == '`' {
                            // Kind of a quirk, but PowerShell only inserts a
                            // newline after a backtick if the character preceding
                            // the backtick is whitespace.
                            if let Some(c) = preceding_chars.next()
                                && !c.is_ascii_whitespace()
                            {
                                return false;
                            }
                            return true;
                        }
                    }
                    Some(ShellFamily::Posix) | None => {
                        if c == '\\' {
                            // Continue if there are more \ characters
                            if let Some(c) = preceding_chars.next()
                                && c == '\\'
                            {
                                continue;
                            }
                            // Odd number of \ characters
                            return true;
                        }
                    }
                }
                return false;
            }
            false
        })
    }

    pub(crate) fn initiate_create_new_project(
        &mut self,
        ai_query: String,
        ctx: &mut ViewContext<Self>,
    ) {
        if FeatureFlag::AgentView.is_enabled()
            && !self.agent_view_controller.as_ref(ctx).is_active()
        {
            self.agent_view_controller.update(ctx, |controller, ctx| {
                let _ =
                    controller.try_enter_agent_view(None, AgentViewEntryOrigin::ProjectEntry, ctx);
            });
        }
        self.ai_controller.update(ctx, move |controller, ctx| {
            controller.send_create_new_project_request(ai_query, ctx)
        });
    }

    pub(crate) fn initiate_clone_repository(&mut self, url: String, ctx: &mut ViewContext<Self>) {
        if FeatureFlag::AgentView.is_enabled()
            && !self.agent_view_controller.as_ref(ctx).is_active()
        {
            self.agent_view_controller.update(ctx, |controller, ctx| {
                let _ =
                    controller.try_enter_agent_view(None, AgentViewEntryOrigin::ProjectEntry, ctx);
            });
        }
        self.ai_controller.update(ctx, move |controller, ctx| {
            controller.send_slash_command_request(SlashCommandRequest::CloneRepository { url }, ctx)
        });
    }

    /// Handles the user's 'Enter' keypress.
    ///
    /// Depending on input state, this method may either execute a command, accept an input
    /// suggestion, or add a newline to the input buffer contents.  If there is an active and long
    /// running command, exits early and does nothing. This method should not be callable if there
    /// is an active and long running command; in such a state, the enter keypress should be
    /// handled by the ongoing process corresponding to the active/long running command.
    pub(crate) fn input_enter(&mut self, ctx: &mut ViewContext<Self>) {
        let command = self.editor.as_ref(ctx).buffer_text(ctx);

        if self.should_insert_newline_on_enter(ctx) {
            self.editor.update(ctx, |editor, ctx| {
                editor.user_initiated_insert("\n", PlainTextEditorViewAction::NewLine, ctx)
            });
        } else if self
            .queued_prompts_panel
            .as_ref()
            .is_some_and(|panel| panel.as_ref(ctx).enter_sends_queued_prompt(ctx))
        {
            // An empty-buffer Enter sends the top queued row, mirroring its send-now button.
            // The locked initial cloud-mode head row is not sendable, so Enter does nothing
            // while it sits at the head of the queue.
            let conversation_id =
                BlocklistAIHistoryModel::as_ref(ctx).active_conversation_id(self.terminal_view_id);
            let top_row = conversation_id.and_then(|conversation_id| {
                QueuedQueryModel::as_ref(ctx)
                    .queue(conversation_id)
                    .first()
                    .filter(|row| !row.is_locked())
                    .map(|row| (row.id(), row.text().to_owned(), row.is_command()))
            });
            if let (Some(conversation_id), Some((query_id, text, is_command))) =
                (conversation_id, top_row)
            {
                self.send_queued_row_immediately(
                    conversation_id,
                    query_id,
                    text,
                    is_command,
                    QueuedPromptSendNowTrigger::EnterOnEmptyInput,
                    ctx,
                );
            }
            return;
        } else if self.maybe_launch_cloud_handoff_request(ctx)
            || self.maybe_queue_input_for_in_progress_conversation(ctx)
            || self.maybe_queue_input_during_cloud_setup(ctx)
        {
            return;
        } else if matches!(
            self.suggestions_mode_model.as_ref(ctx).mode(),
            InputSuggestionsMode::CompletionSuggestions { .. }
        ) && self.should_enter_accept_completion_suggestion(ctx)
        {
            self.input_suggestions.update(ctx, |suggestions, ctx| {
                suggestions.confirm(ctx);
            })
        } else if matches!(
            self.suggestions_mode_model.as_ref(ctx).mode(),
            InputSuggestionsMode::StaticWorkflowEnumSuggestions { .. }
                | InputSuggestionsMode::DynamicWorkflowEnumSuggestions { .. }
        ) {
            self.input_suggestions.update(ctx, |suggestions, ctx| {
                suggestions.confirm(ctx);
            });
        } else if FeatureFlag::CloudModeSetupV2.is_enabled()
            && is_cloud_agent_pre_first_exchange(
                self.ambient_agent_view_model(),
                &self.agent_view_controller,
                &self.model.lock(),
                ctx,
            )
        {
            // During cloud-mode setup, non-queued submissions (e.g. third-party harness runs that
            // don't queue) are dropped rather than sent as live prompts the sharer can't accept.
            return;
        } else if FeatureFlag::AgentMode.is_enabled()
            && AISettings::as_ref(ctx).is_any_ai_enabled(ctx)
            && (self.ai_input_model.as_ref(ctx).is_ai_input_enabled()
                || self.is_cloud_mode_input_v2_composing(ctx))
        {
            // Check if we're configuring an ambient agent and spawn it instead of submitting a regular AI query.
            if self
                .ambient_agent_view_model()
                .is_some_and(|ambient_agent_model| {
                    ambient_agent_model
                        .as_ref(ctx)
                        .is_configuring_ambient_agent()
                })
            {
                let team_required = UserWorkspaces::as_ref(ctx).cloud_agents_require_team();
                let has_enabled_harness = !FeatureFlag::AgentHarness.is_enabled()
                    || HarnessAvailabilityModel::as_ref(ctx).has_any_enabled_harness();
                let blocker_message =
                    match cloud_agent_start_blocker(team_required, has_enabled_harness) {
                        Some(CloudAgentStartBlocker::TeamRequired) => {
                            Some(CLOUD_AGENT_TEAM_REQUIRED_MESSAGE.to_string())
                        }
                        Some(CloudAgentStartBlocker::NoEnabledHarnesses) => Some(
                            "No agent harnesses are available. Contact your team admin."
                                .to_string(),
                        ),
                        None => None,
                    };
                if let Some(message) = blocker_message {
                    let window_id = ctx.window_id();
                    ToastStack::handle(ctx).update(ctx, |ts, ctx| {
                        ts.add_ephemeral_toast(DismissibleToast::error(message), window_id, ctx);
                    });
                    return;
                }

                let prompt = command.trim().to_owned();
                if prompt.is_empty() {
                    return;
                }

                if self.is_cloud_mode_input_v2_composing(ctx)
                    && let Some(ambient_agent_view_model) = self.ambient_agent_view_model()
                {
                    let needs_env_modal = ambient_agent_view_model
                        .as_ref(ctx)
                        .selected_environment_id()
                        .is_none();
                    if needs_env_modal {
                        ctx.emit(Event::OpenCloudModeV2EnvironmentCreationModal);
                        return;
                    }
                }

                #[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
                let attachments = self
                    .collect_cloud_launch_attachments(ctx)
                    .request_attachments;
                #[cfg(not(all(feature = "local_fs", not(target_family = "wasm"))))]
                let attachments = vec![];

                self.emit_input_buffer_submitted_telemetry(ctx);

                // Clear the buffer and pending attachments after collecting them.
                self.editor.update(ctx, |editor, ctx| {
                    editor.clear_buffer(ctx);
                });
                self.ai_context_model.update(ctx, |context_model, ctx| {
                    context_model.clear_pending_attachments(ctx);
                });

                if let Some(ambient_agent_view_model) = self.ambient_agent_view_model() {
                    let scope = UserWorkspaces::as_ref(ctx).team_context_for_operation(ctx);
                    ambient_agent_view_model.update(ctx, |state, ctx| {
                        state.spawn_agent(prompt, attachments, &scope, ctx);
                    });
                }
                return;
            }

            self.submit_ai_query_with_routing(ctx);
        } else {
            let command = self.get_command(ctx);
            if !self.try_execute_command(&command, ctx) {
                return;
            }
            self.emit_input_buffer_submitted_telemetry(ctx);

            if FeatureFlag::AgentMode.is_enabled()
                && AISettings::as_ref(ctx).is_ai_autodetection_enabled(ctx)
            {
                self.ai_input_model.update(ctx, |input, ctx| {
                    input.abort_in_progress_detection();

                    // The default input state after executing a shell command is Shell mode with
                    // autodetection enabled.
                    input.set_input_config_for_classic_mode(
                        InputConfig {
                            input_type: InputType::Shell,
                            is_locked: true,
                        }
                        .unlocked_if_autodetection_enabled(false, ctx),
                        ctx,
                    );
                });
            }

            // Cancel actively streaming conversations if we're able to run the command.
            // This is possible in persistent input mode.
            self.ai_controller.update(ctx, |controller, ctx| {
                let active_conversation_id = BlocklistAIHistoryModel::as_ref(ctx)
                    .active_conversation(self.terminal_view_id)
                    .filter(|conversation| conversation.status().is_in_progress())
                    .map(|conversation| conversation.id());
                if let Some(active_conversation_id) = active_conversation_id {
                    controller.cancel_conversation_progress(
                        active_conversation_id,
                        CancellationReason::UserCommandExecuted,
                        ctx,
                    );
                }
            });

            self.ai_input_model.update(ctx, |model, ctx| {
                model.handle_input_buffer_submitted(ctx);
            });

            if SyncedInputState::as_ref(ctx).is_syncing_any_inputs(ctx.window_id()) {
                ctx.emit(Event::SyncInput(SyncInputType::RanCommand));
            }

            self.model.lock().set_is_input_dirty(false);
        }

        AISettings::handle(ctx).update(ctx, |ai_settings, ctx| {
            // Don't show the quota banner once a user has run a command or AI query.
            ai_settings.mark_quota_banner_as_dismissed(ctx);
            ctx.notify();
        });
    }

    fn input_cmd_enter(&mut self, ctx: &mut ViewContext<Self>) {
        // NaturalLanguageCommandSearch has its own `cmd+enter` behaviour, not expected to execute here
        let mode = self.suggestions_mode_model.as_ref(ctx).mode().clone();
        match &mode {
            InputSuggestionsMode::CompletionSuggestions { .. }
            | InputSuggestionsMode::HistoryUp { .. }
                // If FeatureFlag::AgentView is enabled, cmd-enter should unconditionally enter the
                // agent view with the current buffer contents as agent input.
                //
                // I'm (ZB) not even sure what this legacy behavior is for, because if you have any
                // selected completion or history suggestion, that suggestion has already been
                // inserted into the buffer so enter (without cmd- prefix) would directly execute
                // it anyway.
                if !FeatureFlag::AgentView.is_enabled() =>
            {
                self.input_suggestions.update(ctx, |suggestions, ctx| {
                    suggestions.confirm_and_execute(ctx);
                });
            }
            InputSuggestionsMode::DynamicWorkflowEnumSuggestions {
                dynamic_enum_status: DynamicEnumSuggestionStatus::Unapproved,
                command,
                ..
            } => {
                let editor_model = self.editor.read(ctx, |view, ctx| view.snapshot_model(ctx));
                self.get_enum_suggestions_async(command.clone(), editor_model, ctx);
            }
            _ => {
                // In cloud mode (ambient agent), Cmd+Enter should exit cloud mode entirely and start a
                // new *local* agent conversation in the root terminal. This should work whether the
                // buffer is empty (blank convo) or non-empty (prefill draft, but don't auto-send).
                if self
                    .ambient_agent_view_model()
                    .is_some_and(|ambient_agent_model| {
                        ambient_agent_model.as_ref(ctx).is_ambient_agent()
                    })
                {
                    let mut draft = self.editor.as_ref(ctx).buffer_text(ctx);
                    // Normalize draft for empty-checks and for prefill.
                    draft.truncate(draft.trim_end().len());

                    let is_empty = draft.trim().is_empty();
                    ctx.emit(Event::ExitCloudModeAndStartLocalAgent {
                        initial_prompt: (!is_empty).then_some(draft),
                    });
                    return;
                }

                // Cmd+Enter is not a local-submit gesture (Enter is), so only route the
                // remote/cloud cases here.
                self.maybe_route_ai_query_to_remote_target(ctx);
            }
        }
    }

    /// Re-submits a queued prompt through the correct handler (slash, skill, or regular AI query),
    /// without touching the input buffer or triggering NLD / autosuggestion side-effects.
    ///
    /// Cancels the in-flight stream first so slash/skill paths don't trip the in-flight assertion.
    /// `is_for_same_conversation: true` keeps the conversation status `InProgress` so the warping
    /// indicator stays visible.
    pub(crate) fn submit_queued_prompt(
        &mut self,
        prompt: String,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        ctx: &mut ViewContext<Self>,
    ) {
        self.ai_controller.update(ctx, |controller, ctx| {
            controller.cancel_conversation_progress(
                conversation_id,
                CancellationReason::FollowUpSubmitted {
                    is_for_same_conversation: true,
                },
                ctx,
            );
        });

        // A fired queued row always belongs to the existing conversation that finished, so we
        // submit into that conversation directly rather than re-deriving from the current UI
        // selection (which may point at a different conversation the user navigated to).
        self.ai_controller.update(ctx, move |controller, ctx| {
            controller.send_queued_user_query_in_conversation(
                prompt,
                conversation_id,
                None,
                query_id,
                ctx,
            );
        });

        ctx.emit(Event::ExecuteAIQuery);
    }

    /// Submits `prompt` immediately as a regular (non-queued) user query — the same controller
    /// path `submit_ai_query` uses for a typed-and-entered prompt. Used by the `/queue`
    /// not-in-progress fallback and the legacy pending-user-query submission paths, which are
    /// immediate sends (not queued-row fires) and therefore reset their live staging.
    pub(crate) fn submit_user_query_now(&mut self, prompt: String, ctx: &mut ViewContext<Self>) {
        if let Some(conversation_id) = self
            .ai_context_model
            .as_ref(ctx)
            .selected_conversation_id(ctx)
        {
            self.ai_controller.update(ctx, move |controller, ctx| {
                controller.send_user_query_in_conversation(prompt, conversation_id, None, ctx);
            });
        } else {
            self.ai_controller.update(ctx, move |controller, ctx| {
                controller.send_user_query_in_new_conversation(
                    prompt,
                    None,
                    EntrypointType::UserInitiated,
                    None,
                    ctx,
                );
            });
        }

        ctx.emit(Event::ExecuteAIQuery);
    }

    /// Routes a popped queued prompt to the correct submission path for the active pane,
    /// without touching the editor buffer or freezing the input. Queue draining must not
    /// borrow the user-initiated submission UI (loading indicator, buffer replace), because
    /// the queue panel itself is already the "this prompt is in flight" affordance and the
    /// user may be typing a different prompt locally.
    pub(crate) fn submit_queued_prompt_for_active_pane(
        &mut self,
        prompt: String,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        ctx: &mut ViewContext<Self>,
    ) {
        // Cloud follow-up path: the cloud run has ended an execution and the next queued
        // prompt should start a new one. Wins over the viewer path because the old shared
        // session is no longer live to receive a SendAgentPrompt.
        let is_ready_for_cloud_followup =
            self.ambient_agent_view_model()
                .is_some_and(|ambient_agent_model| {
                    ambient_agent_model
                        .as_ref(ctx)
                        .is_ready_for_cloud_followup_prompt()
                });

        if is_ready_for_cloud_followup {
            // Cloud follow-up does not support attachments; a queued row's attachments are dropped
            // when the row is removed after dispatch.
            let drops_attachments = !QueuedQueryModel::as_ref(ctx)
                .attachments_for(conversation_id, query_id)
                .is_empty();
            if drops_attachments {
                log::warn!(
                    "Dropping attachments on a queued cloud follow-up prompt; cloud follow-up does not support attachments"
                );
            }
            ctx.emit(Event::SubmitCloudFollowup { prompt });
            return;
        }

        // Shared-session viewer path (covers an in-flight cloud run from the owner's client).
        // Send the prompt straight to the sharer via Event::SendAgentPrompt, carrying the queued
        // row's own attachments (uploaded when supported). When the user's editor is empty we
        // also surface the standard `"<prompt> ◌"` loading affordance so the queued submission has
        // visible feedback while the sharer ack flight is in flight; the
        // `NetworkEvent::AgentPromptRequestInFlight` -> `unfreeze_and_clear_agent_input` hop will
        // clear it once the sharer acknowledges receipt. If the user has typed something locally,
        // we leave the buffer alone so their in-progress prompt is not clobbered.
        if self.model.lock().shared_session_status().is_viewer() {
            let server_conversation_token = BlocklistAIHistoryModel::as_ref(ctx)
                .conversation(&conversation_id)
                .and_then(|conv| conv.server_conversation_token().cloned())
                .and_then(|token| {
                    token
                        .as_str()
                        .parse()
                        .ok()
                        .map(ServerConversationToken::from_uuid)
                });

            // Split the firing row's stored attachments into images/files for upload.
            let mut images: Vec<ImageContext> = Vec::new();
            let mut files: Vec<PendingFile> = Vec::new();
            for attachment in
                QueuedQueryModel::as_ref(ctx).attachments_for(conversation_id, query_id)
            {
                match attachment {
                    PendingAttachment::Image(image) => images.push(image.clone()),
                    PendingAttachment::File(file) => files.push(file.clone()),
                }
            }

            if self.editor.as_ref(ctx).buffer_text(ctx).is_empty() {
                self.freeze_input_in_loading_state_with_text(&prompt, ctx);
            }
            let queued_query_retry = QueuedQueryModel::as_ref(ctx)
                .queue(conversation_id)
                .iter()
                .enumerate()
                .find(|(_, query)| query.id() == query_id)
                .map(|(index, query)| (conversation_id, index, query.clone()));
            self.upload_and_send_viewer_prompt(
                server_conversation_token,
                prompt,
                vec![],
                images,
                files,
                queued_query_retry,
                ctx,
            );
            return;
        }

        // Local Agent Mode path.
        self.submit_queued_prompt(prompt, conversation_id, query_id, ctx);
    }

    /// Queues the current input instead of submitting it when the active conversation is
    /// busy and queueing is in effect for it. Returns true when the input was queued, in
    /// which case the caller should skip normal submission.
    fn maybe_queue_input_for_in_progress_conversation(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if !FeatureFlag::QueueSlashCommand.is_enabled() {
            return false;
        }

        // A shell-mode submission queues as a command; an AI-mode submission queues as a prompt.
        // Command queueing is gated on the V2 surface.
        let is_command = !self.ai_input_model.as_ref(ctx).is_ai_input_enabled();
        if is_command && !FeatureFlag::QueuedPromptsV2.is_enabled() {
            return false;
        }

        let Some(conversation_id) = self
            .ai_context_model
            .as_ref(ctx)
            .selected_conversation_id(ctx)
        else {
            return false;
        };

        let is_summarizing = BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .is_some_and(|c| c.is_summarizing());
        // Summarization only routes a prompt into the queued-prompts panel under QueuedPromptsV2;
        // with the flag off, only the auto-queue toggle queues (pre-V2 behavior).
        let queue_for_summarize = is_summarizing && FeatureFlag::QueuedPromptsV2.is_enabled();

        let queue_model = QueuedQueryModel::as_ref(ctx);
        let queue_head_allows_lrc = match queue_model.queue(conversation_id).first() {
            Some(row) => matches!(
                row.origin(),
                QueuedQueryOrigin::LrcAutoQueue | QueuedQueryOrigin::PendingLrcAutoQueue
            ),
            None => true,
        };
        let queue_enabled = {
            let terminal_model = self.model.lock();
            queue_model.is_queue_next_prompt_enabled(
                conversation_id,
                terminal_model.block_list().active_block(),
                ctx,
            )
        };

        // True when the LRC branch is the effective enabler (queueing would be off outside
        // the command) and the current queue head can fire at command finish too.
        let queued_for_lrc = queue_enabled
            && !queue_model.is_queue_next_prompt_toggle_enabled(conversation_id)
            && queue_head_allows_lrc;

        // When queue mode is not normally active but an agent-requested run_shell_command
        // action is still pending (snapshot not yet fired), queue as PendingLrcAutoQueue
        // to prevent the CliAgentUserQuery / LRC snapshot race.
        let queued_for_pending_lrc = !queue_enabled && !queue_for_summarize && !is_command && {
            let pending_action_id = {
                let terminal_model = self.model.lock();
                let active_block = terminal_model.block_list().active_block();
                if active_block.is_active_and_long_running() && !active_block.is_agent_monitoring()
                {
                    active_block.requested_command_action_id().cloned()
                } else {
                    None
                }
            };
            pending_action_id.as_ref().is_some_and(|action_id| {
                self.ai_action_model
                    .as_ref(ctx)
                    .is_shell_command_action_pending(action_id, conversation_id)
            })
        };

        if !queue_enabled && !queue_for_summarize && !queued_for_pending_lrc {
            return false;
        }

        let conversation_in_progress = BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .is_some_and(|c| {
                !c.is_empty() && (c.status().is_in_progress() || c.status().is_blocked())
            });
        // While a drained queued command is running the agent is idle, but the queue must keep
        // accepting rows so FIFO order is preserved (PRODUCT §14).
        let command_in_flight =
            QueuedQueryModel::as_ref(ctx).has_command_in_flight(conversation_id);
        if !conversation_in_progress && !command_in_flight {
            return false;
        }

        let prompt = self.editor.as_ref(ctx).buffer_text(ctx);
        if prompt.is_empty() {
            return false;
        }

        self.ai_input_model.update(ctx, |model, ctx| {
            model.handle_input_buffer_submitted(ctx);
        });
        self.emit_input_buffer_submitted_telemetry(ctx);
        self.editor.update(ctx, |editor, ctx| {
            editor.clear_buffer(ctx);
        });

        // PendingLrcAutoQueue rows are locked until the snapshot fires; LrcAutoQueue
        // rows auto-fire when the command completes. Command rows use AutoQueueToggle.
        let origin = if queued_for_pending_lrc {
            QueuedQueryOrigin::PendingLrcAutoQueue
        } else if queued_for_lrc && !is_command {
            QueuedQueryOrigin::LrcAutoQueue
        } else {
            QueuedQueryOrigin::AutoQueueToggle
        };
        // Commands carry no attachments; only prompts consume the pending attachments.
        let query = if is_command {
            QueuedQuery::new_command(prompt, origin)
        } else {
            let attachments = self.ai_context_model.update(ctx, |context_model, ctx| {
                context_model.take_pending_attachments(ctx)
            });
            QueuedQuery::new_with_attachments(prompt, origin, attachments)
        };
        QueuedQueryModel::handle(ctx)
            .update(ctx, |model, ctx| model.append(conversation_id, query, ctx));

        true
    }

    /// Queues the current input on cloud-mode panes that are provisioned but not
    /// currently running (e.g. between cloud executions). Returns true and clears the
    /// editor when the input is captured so the caller skips the normal submission
    /// path. Only active when `QueuedPromptsV2` is enabled.
    fn maybe_queue_input_during_cloud_setup(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        if !FeatureFlag::QueuedPromptsV2.is_enabled() {
            return false;
        }

        // Third-party (non-Oz) harnesses don't support prompt queueing, so leave the input
        // alone; the submission then falls through to being blocked during setup.
        let is_third_party_harness =
            self.ambient_agent_view_model()
                .is_some_and(|ambient_agent_model| {
                    ambient_agent_model.as_ref(ctx).is_third_party_harness()
                });
        let should_queue = !is_third_party_harness
            && is_cloud_agent_pre_first_exchange(
                self.ambient_agent_view_model(),
                &self.agent_view_controller,
                &self.model.lock(),
                ctx,
            );
        if !should_queue {
            return false;
        }

        let Some(conversation_id) = self
            .ai_context_model
            .as_ref(ctx)
            .selected_conversation_id(ctx)
        else {
            return false;
        };

        let prompt = self.editor.as_ref(ctx).buffer_text(ctx);
        let prompt = prompt.trim().to_owned();
        if prompt.is_empty() {
            return false;
        }
        self.emit_input_buffer_submitted_telemetry(ctx);

        self.editor.update(ctx, |editor, ctx| {
            editor.clear_buffer(ctx);
        });
        let attachments = self.ai_context_model.update(ctx, |context_model, ctx| {
            context_model.take_pending_attachments(ctx)
        });
        QueuedQueryModel::handle(ctx).update(ctx, |model, ctx| {
            model.append(
                conversation_id,
                QueuedQuery::new_with_attachments(
                    prompt,
                    QueuedQueryOrigin::AutoQueueToggle,
                    attachments,
                ),
                ctx,
            );
        });

        true
    }

    /// Submit the input buffer contents as an AI query to continue the conversation locally on the
    /// machine. This is the local case of [`Self::submit_ai_query_with_routing`]; prefer calling
    /// that so cloud/remote panes are routed correctly.
    fn submit_ai_query_local(&mut self, ctx: &mut ViewContext<Self>) {
        self.editor.update(ctx, |editor, ctx| {
            editor.abort_attached_images_future_handle(ctx);
        });

        // Cloud/remote follow-up routing (live viewer, new cloud VM, stale or read-only) is handled
        // by `submit_ai_query_with_routing` / `maybe_route_ai_query_to_remote_target` before this point,
        // so this method only performs local submission.

        // If the agent view is inactive but the current input is detected as AI, submitting
        // this query triggers entering the agent view.
        if FeatureFlag::AgentView.is_enabled()
            && !self.agent_view_controller.as_ref(ctx).is_active()
        {
            let prompt = self.editor.as_ref(ctx).buffer_text(ctx);
            let prompt = prompt.trim().to_owned();
            // Don't enter the agent view if input is autodetected as AI but the input is empty.
            //
            // This may happen because the input mode must be set to either shell or agent, and
            // when the buffer cleared the input remains in whichever mode it was in previously
            // until new input is entered.
            if prompt.is_empty() {
                return;
            }
            ctx.emit(Event::EnterAgentView {
                initial_prompt: Some(prompt),
                conversation_id: None,
                origin: AgentViewEntryOrigin::Input {
                    was_prompt_autodetected: !self
                        .ai_input_model
                        .as_ref(ctx)
                        .is_input_type_locked(),
                },
            });
            return;
        }

        let alert_blocks_ai = {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            let scope = user_workspaces.team_context_for_view(ctx);
            PromptAlertView::does_alert_block_ai_requests(&scope, ctx)
        };
        if alert_blocks_ai {
            AIRequestUsageModel::handle(ctx).update(ctx, |usage_model, ctx| {
                // Rate limit requests to fetch the user's AI usage if triggered by enter
                // keypress.
                const USAGE_LIMIT_UPDATE_REQUEST_RATE_LIMIT: Duration = Duration::from_secs(10);

                let last_update_time = usage_model.last_update_time();
                if last_update_time
                    .is_some_and(|time| time.elapsed() >= USAGE_LIMIT_UPDATE_REQUEST_RATE_LIMIT)
                    || last_update_time.is_none()
                {
                    usage_model.refresh_request_usage_async(ctx);
                }
            });

            return;
        }

        let ai_query = self.editor.as_ref(ctx).buffer_text(ctx);
        // We don't send AI requests with empty queries, even if the context is non-empty. We
        // also don't send a query when the input (query plus context) is over the length limit.
        // If we haven't calculated the input length, we optimistically as if it is within the
        // limit. We always check the length before sending making the API request.
        if ai_query.is_empty() {
            return;
        }

        IgnoredSuggestionsModel::handle(ctx).update(ctx, |model, ctx| {
            model.remove_ignored_suggestion(ai_query.clone(), SuggestionType::AIQuery, ctx);
        });
        self.emit_input_buffer_submitted_telemetry(ctx);

        self.ai_input_model.update(ctx, |model, ctx| {
            model.handle_input_buffer_submitted(ctx);
        });

        if let Some(conversation_id) = self
            .ai_context_model
            .as_ref(ctx)
            .selected_conversation_id(ctx)
        {
            self.ai_controller.update(ctx, move |controller, ctx| {
                controller.send_user_query_in_conversation(ai_query, conversation_id, None, ctx)
            });
        } else {
            self.ai_controller.update(ctx, move |controller, ctx| {
                controller.send_user_query_in_new_conversation(
                    ai_query,
                    None,
                    EntrypointType::UserInitiated,
                    None,
                    ctx,
                );
            });
        }

        ctx.emit(Event::ExecuteAIQuery);

        if let Some(workflow_state) = self.workflows_state.selected_workflow_state.as_ref()
            && let WorkflowType::Cloud(workflow) = &workflow_state.workflow_type
        {
            send_telemetry_from_ctx!(
                TelemetryEvent::ExecutedWarpDrivePrompt {
                    id: workflow.id.into_server().map(Into::into),
                    selection_source: workflow_state.workflow_selection_source,
                },
                ctx
            );

            UpdateManager::handle(ctx).update(ctx, move |update_manager, ctx| {
                update_manager.record_object_action(
                    workflow.cloud_object_type_and_id(),
                    ObjectActionType::Execute,
                    None,
                    ctx,
                )
            });
        }
    }

    /// Send the given query to the session sharer for them to execute on their machine.
    /// Returns false if the query should be run locally instead of being sent to the sharer
    /// (which is the case for slash commands like fork and fork-and-compact).
    fn submit_viewer_ai_query(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        let prompt = self.editor.as_ref(ctx).buffer_text(ctx);
        if prompt.is_empty() {
            return true;
        }

        // We're committed to sending the prompt, so finalize any in-flight image-attachment
        // processing. This drops images that haven't finished processing; already-processed ones
        // are collected as pending context below. (Local-action slash commands returned above.)
        self.emit_input_buffer_submitted_telemetry(ctx);
        self.editor.update(ctx, |editor, ctx| {
            editor.abort_attached_images_future_handle(ctx);
        });

        // Freeze the editor and put it in a loading state
        self.freeze_input_in_loading_state(ctx);

        // Look up the conversation's server token from the conversation metadata.
        let selected_conv_id = self
            .ai_context_model
            .as_ref(ctx)
            .selected_conversation_id(ctx);
        let server_conversation_token = selected_conv_id
            .and_then(|id| {
                BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&id)
                    .and_then(|conv| conv.server_conversation_token().cloned())
            })
            .and_then(|token| {
                token
                    .as_str()
                    .parse()
                    .ok()
                    .map(ServerConversationToken::from_uuid)
            });

        // Collect block/selected-text references from the context model.
        let attachments: Vec<AgentAttachment> = self
            .ai_context_model
            .as_ref(ctx)
            .pending_context(ctx, true, None)
            .into_iter()
            .filter_map(|context| match context {
                AIAgentContext::Block(block) => Some(AgentAttachment::BlockReference {
                    block_id: block.id.into(),
                }),
                AIAgentContext::SelectedText(text) => {
                    Some(AgentAttachment::PlainText { content: text })
                }
                // For now, only AgentAttachment context is supported.
                // TODO: Add support for other context types.
                _ => None,
            })
            .collect();

        let pending_images: Vec<_> = self
            .ai_context_model
            .as_ref(ctx)
            .pending_images()
            .into_iter()
            .cloned()
            .collect();
        let pending_files: Vec<_> = self
            .ai_context_model
            .as_ref(ctx)
            .pending_files()
            .into_iter()
            .cloned()
            .collect();

        self.upload_and_send_viewer_prompt(
            server_conversation_token,
            prompt,
            attachments,
            pending_images,
            pending_files,
            None,
            ctx,
        );

        true
    }

    /// Upload pending attachments to the task definition before emitting the text-only cloud
    /// follow-up event. `SubmitCloudFollowup` only carries the prompt text, so this helper owns
    /// the prompt and attachment payloads until the async upload either succeeds and submits the
    /// prompt or fails and restores the input. A new VM execution downloads these task attachments
    /// during startup.
    fn upload_files_then_submit_cloud_followup(
        &mut self,
        task_id: crate::ai::ambient_agents::AmbientAgentTaskId,
        prompt: String,
        pending_attachments: Vec<PendingAttachment>,
        ctx: &mut ViewContext<Self>,
    ) -> SpawnedFutureHandle {
        let ai_client = ServerApiProvider::as_ref(ctx).get_ai_client();
        let server_api = ServerApiProvider::as_ref(ctx).get();

        ctx.spawn(
            async move {
                // Fail fast: any per-attachment failure (decode, size-limit, or HTTP) is fatal
                // on the VM-down follow-up path; the user can fix the attachment and retry.
                let outcomes = upload_pending_attachments_to_task(
                    ai_client,
                    server_api,
                    task_id,
                    pending_attachments,
                )
                .await
                .map_err(|e| format!("Failed to prepare attachment uploads: {e:#}"))?;
                for outcome in &outcomes {
                    if let TaskAttachmentUploadOutcome::Failed { error, .. } = outcome {
                        return Err(error.clone());
                    }
                }
                Ok::<(), String>(())
            },
            move |input, result, ctx| {
                if let Err(error) = result {
                    input.restore_cloud_followup_input_after_upload_failure(&prompt, ctx);
                    let window_id = ctx.window_id();
                    ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                        toast_stack.add_ephemeral_toast(
                            DismissibleToast::error(format!("Couldn't upload attachment: {error}")),
                            window_id,
                            ctx,
                        );
                    });
                    return;
                }

                input.ai_context_model.update(ctx, |context_model, ctx| {
                    context_model.clear_pending_attachments(ctx);
                });
                ctx.emit(Event::SubmitCloudFollowup { prompt });
            },
        )
    }

    fn emit_input_buffer_submitted_telemetry(&self, ctx: &mut ViewContext<Self>) {
        let input_model = self.ai_input_model.as_ref(ctx);
        let block_id = self.model.lock().active_block_id().clone();
        send_telemetry_from_ctx!(
            TelemetryEvent::InputBufferSubmitted {
                input_type: input_model.input_type(),
                is_locked: input_model.is_input_type_locked(),
                input_type_decision_source: input_model.last_ai_autodetection_source(),
                was_lock_set_with_empty_buffer: input_model.was_lock_set_with_empty_buffer(),
                block_id,
            },
            ctx
        );
    }

    /// Uploads `images`/`files` (when the cloud pane supports it) and emits `Event::SendAgentPrompt`
    /// with the resulting attachments. Shared by the immediate viewer submission and the queued
    /// viewer drain so both go through the identical upload-then-send path.
    #[allow(clippy::too_many_arguments)]
    fn upload_and_send_viewer_prompt(
        &mut self,
        server_conversation_token: Option<ServerConversationToken>,
        prompt: String,
        base_attachments: Vec<AgentAttachment>,
        images: Vec<ImageContext>,
        files: Vec<PendingFile>,
        queued_query_retry: Option<(AIConversationId, usize, QueuedQuery)>,
        ctx: &mut ViewContext<Self>,
    ) {
        let ambient_agent_task_id = self
            .ambient_agent_view_model()
            .and_then(|ambient_agent_model| ambient_agent_model.as_ref(ctx).task_id());
        let has_uploads = (!images.is_empty() || !files.is_empty())
            && FeatureFlag::CloudModeImageContext.is_enabled();

        if let Some(task_id) = ambient_agent_task_id.filter(|_| has_uploads) {
            // Upload files first, then send prompt with file references in callback
            Self::upload_files_then_send_prompt(
                task_id,
                server_conversation_token,
                prompt,
                base_attachments,
                &images,
                &files,
                queued_query_retry,
                ctx,
            );
        } else {
            // No files to upload, send prompt immediately
            if !images.is_empty() || !files.is_empty() {
                log::warn!("Cannot upload files: no task_id available");
            }
            ctx.emit(Event::SendAgentPrompt {
                server_conversation_token,
                prompt,
                attachments: base_attachments,
            });
        }
    }

    /// Uploads image and file attachments to GCS via presigned URLs, then emits `SendAgentPrompt`
    /// with the resulting `FileReference` attachments appended.
    #[allow(clippy::too_many_arguments)]
    fn upload_files_then_send_prompt(
        task_id: crate::ai::ambient_agents::AmbientAgentTaskId,
        server_conversation_token: Option<
            session_sharing_protocol::common::ServerConversationToken,
        >,
        prompt: String,
        base_attachments: Vec<AgentAttachment>,
        pending_images: &[crate::ai::agent::ImageContext],
        pending_files: &[crate::ai::blocklist::PendingFile],
        queued_query_retry: Option<(AIConversationId, usize, QueuedQuery)>,
        ctx: &mut ViewContext<Self>,
    ) {
        let ai_client = ServerApiProvider::as_ref(ctx).get_ai_client();
        let server_api = ServerApiProvider::as_ref(ctx).get();

        // Combine images and files into a unified list so
        // `upload_pending_attachments_to_task` can handle both kinds uniformly.
        let pending_attachments: Vec<PendingAttachment> = pending_images
            .iter()
            .cloned()
            .map(PendingAttachment::Image)
            .chain(pending_files.iter().cloned().map(PendingAttachment::File))
            .collect();
        let pending_count = pending_attachments.len();

        ctx.spawn(
            async move {
                // Best-effort: continue with successful uploads even when individual
                // attachments fail (decode / size-limit / HTTP). Return `None` only when
                // the prepare call fails entirely (maps to the "too many attachments" toast).
                let outcomes = match upload_pending_attachments_to_task(
                    ai_client,
                    server_api,
                    task_id,
                    pending_attachments,
                )
                .await
                {
                    Ok(outcomes) => outcomes,
                    Err(e) => {
                        log::error!(
                            "Failed to prepare attachment uploads for task {task_id}: {e:#}"
                        );
                        return None;
                    }
                };

                let mut uploaded = Vec::new();
                for outcome in outcomes {
                    match outcome {
                        TaskAttachmentUploadOutcome::Uploaded {
                            attachment_id,
                            file_name,
                        } => {
                            uploaded.push(AgentAttachment::FileReference {
                                attachment_id,
                                file_name,
                            });
                        }
                        TaskAttachmentUploadOutcome::Failed { file_name, error } => {
                            log::warn!("Failed to upload attachment {file_name}: {error}");
                        }
                    }
                }

                if uploaded.len() < pending_count {
                    log::warn!(
                        "Only {}/{} attachments uploaded successfully",
                        uploaded.len(),
                        pending_count
                    );
                }

                Some(uploaded)
            },
            move |input, maybe_uploaded, ctx| {
                let is_queued_prompt = queued_query_retry.is_some();
                let uploaded_files = match maybe_uploaded {
                    Some(uploaded_files) => uploaded_files,
                    None => {
                        if let Some((conversation_id, insert_index, query)) = queued_query_retry {
                            QueuedQueryModel::handle(ctx).update(ctx, |model, ctx| {
                                model.restore_fired_row(conversation_id, insert_index, query, ctx);
                            });
                        }
                        // Prepare request failed (e.g. attachment limit exceeded).
                        // Keep pending attachments so the user can retry, unfreeze input,
                        // and show an error toast.
                        input.unfreeze_agent_input(false, ctx);
                        let window_id = ctx.window_id();
                        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                            toast_stack.add_ephemeral_toast(
                                DismissibleToast::error(
                                    "Too many attachments for this conversation.".to_string(),
                                ),
                                window_id,
                                ctx,
                            );
                        });
                        return;
                    }
                };

                if !is_queued_prompt {
                    // Upload succeeded — clear pending attachments now.
                    input.ai_context_model.update(ctx, |context_model, ctx| {
                        context_model.clear_pending_attachments(ctx);
                    });
                }

                let mut all_attachments = base_attachments;
                all_attachments.extend(uploaded_files);

                ctx.emit(Event::SendAgentPrompt {
                    server_conversation_token,
                    prompt,
                    attachments: all_attachments,
                });
            },
        );
    }

    /// Returns true if toggling the input mode is disabled.
    fn is_input_mode_toggle_disabled(&self, ctx: &ViewContext<Self>) -> bool {
        // Don't allow input mode changes for:
        // - read-only viewers in shared sessions.
        // - long-running commands with an agent tagged in or in control.
        // - local -> cloud handoff prompts (these must be agent mode prompts)
        let terminal_model = self.model.lock();
        let active_block = terminal_model.block_list().active_block();
        terminal_model.shared_session_status().is_reader()
            || active_block.is_agent_in_control_or_tagged_in()
            || self.prefix_mode(ctx) == InputPrefixMode::CloudHandoff
    }

    /// Set input mode to natural language detection (auto-detection)
    pub fn set_input_mode_natural_language_detection(&mut self, ctx: &mut ViewContext<Self>) {
        if self.is_input_mode_toggle_disabled(ctx) {
            return;
        }

        let is_autodetection_enabled = AISettings::as_ref(ctx).is_ai_autodetection_enabled(ctx);

        if !is_autodetection_enabled {
            return;
        }

        let buffer_text = self.editor.as_ref(ctx).buffer_text(ctx);

        self.ai_input_model.update(ctx, |ai_input_model, ctx| {
            // If we're already configured to do autodetection, there's nothing to do here.
            if ai_input_model.should_run_input_autodetection(ctx) {
                return;
            }

            // Update the input mode to remove any locks and re-enable autodetection.
            // If the buffer is empty, this returns the input mode to the default.
            let input_type = if buffer_text.is_empty() {
                InputType::default()
            } else {
                ai_input_model.input_config().input_type
            };
            ai_input_model.enable_autodetection(input_type, ctx);
        });

        // If the buffer is non-empty, we should kick off the autodetection process, in case the
        // classification doesn't match the previous locked mode.
        if !buffer_text.is_empty()
            && let Some(completion_context) = self.completion_session_context(ctx)
        {
            let ai_input_model = self.ai_input_model.clone();

            ctx.spawn(
                async move {
                    (
                        parse_current_commands_and_tokens(buffer_text, &completion_context).await,
                        completion_context,
                    )
                },
                move |_input, (parsed_tokens, completion_context), ctx| {
                    let session_id = completion_context.session.id();
                    ai_input_model.update(ctx, |model, ctx| {
                        model.detect_and_set_input_type(
                            parsed_tokens,
                            completion_context,
                            Some(session_id),
                            ctx,
                        );
                    });
                },
            );
        }
    }

    /// Set input mode to Agent Mode (AI input)
    pub fn set_input_mode_agent(
        &mut self,
        ensure_input_is_focused: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        // Don't allow input mode changes for read-only viewers in shared sessions
        if self.model.lock().shared_session_status().is_reader() {
            return;
        }

        let is_input_buffer_empty = self.editor.as_ref(ctx).buffer_text(ctx).is_empty();

        // When AgentView is enabled, reverting to AI mode in an active agent view with an empty
        // buffer should unlock (re-enable autodetection) - semantically like clearing the "!".
        //
        // If there is a pending image / file attachment, do NOT unlock. The user's intent is
        // unambiguously "talk to the agent"; letting the classifier flip the input back to
        // shell mode would be a bug.
        let has_locking_attachment = self.ai_context_model.as_ref(ctx).has_locking_attachment();
        let should_unlock = FeatureFlag::AgentView.is_enabled()
            && self.agent_view_controller.as_ref(ctx).is_fullscreen()
            && is_input_buffer_empty
            && AISettings::as_ref(ctx).is_ai_autodetection_enabled(ctx)
            && !has_locking_attachment;

        if should_unlock {
            self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                ai_input_model.enable_autodetection(InputType::AI, ctx);
            });
        } else {
            self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                let new_config = InputConfig {
                    input_type: InputType::AI,
                    is_locked: true,
                };
                let decision_source = if has_locking_attachment {
                    InputTypeAutoDetectionSource::AttachmentForcedAi
                } else {
                    InputTypeAutoDetectionSource::ManualToggle
                };
                ai_input_model.set_input_config(
                    new_config,
                    is_input_buffer_empty,
                    Some(decision_source),
                    ctx,
                );
            });
        }

        if ensure_input_is_focused {
            self.focus_input_box(ctx);
        }
    }

    /// Set input mode to Terminal Mode (shell command input)
    pub fn set_input_mode_terminal(&mut self, steal_focus: bool, ctx: &mut ViewContext<Self>) {
        if self.is_input_mode_toggle_disabled(ctx) {
            return;
        }

        let is_input_buffer_empty = self.editor.as_ref(ctx).buffer_text(ctx).is_empty();
        self.ai_input_model.update(ctx, |ai_input_model, ctx| {
            let new_config = InputConfig {
                input_type: InputType::Shell,
                is_locked: true,
            };
            ai_input_model.set_input_config(
                new_config,
                is_input_buffer_empty,
                Some(InputTypeAutoDetectionSource::ManualToggle),
                ctx,
            );
        });

        if steal_focus {
            self.focus_input_box(ctx);
        }
    }

    /// Applies an input config update from an external source (e.g., session sharing).
    pub fn apply_external_input_config_update(
        &mut self,
        config: InputConfig,
        ctx: &mut ViewContext<Self>,
    ) {
        // do nothing if the config is the same as the current config
        if config == self.ai_input_model.as_ref(ctx).input_config() {
            return;
        }

        let is_input_buffer_empty = self.editor.as_ref(ctx).buffer_text(ctx).is_empty();
        self.ai_input_model.update(ctx, |model, ctx| {
            model.set_input_config(
                config,
                is_input_buffer_empty,
                Some(InputTypeAutoDetectionSource::SessionSharingApply),
                ctx,
            );
        });
    }

    /// Exits `!` shell mode by switching back to AI mode, respecting the agent view's
    /// autodetection setting.
    fn exit_shell_mode_to_ai(&mut self, ctx: &mut ViewContext<Self>) {
        let new_config = InputConfig {
            input_type: InputType::AI,
            is_locked: true,
        }
        .unlocked_if_autodetection_enabled(true, ctx);
        self.ai_input_model.update(ctx, |ai_input_model, ctx| {
            ai_input_model.set_input_config(
                new_config,
                true,
                Some(InputTypeAutoDetectionSource::ShellPrefix),
                ctx,
            );
        });
    }

    /// Returns true if the input is locked in AI mode
    fn is_locked_in_ai_mode(&self, ctx: &ViewContext<Self>) -> bool {
        let ai_input_model = self.ai_input_model.as_ref(ctx);
        ai_input_model.is_input_type_locked() && ai_input_model.input_type().is_ai()
    }

    fn get_command(&mut self, ctx: &mut ViewContext<Self>) -> String {
        // Expand valid abbreviations or aliases, if any
        if let Some(expanded_command) = self.get_expanded_command_on_execute(ctx) {
            return expanded_command;
        }
        self.editor.as_ref(ctx).buffer_text(ctx)
    }

    /// Inserts the given text into the input buffer. Note that this requires a TerminalModel lock
    /// because when not in Agent Mode, we clear all active selections when inserting text into the
    /// editor! Any upstream caller should NOT be holding a lock on the TerminalModel when calling
    /// this method, to avoid a deadlock.
    fn insert_internal(
        &mut self,
        text: &str,
        edit_origin: EditOrigin,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if matches!(edit_origin, EditOrigin::UserTyped) {
            self.model.lock().set_is_input_dirty(true);
        }
        // If not in Agent Mode, clear any active text selections in the blocklist when inserting
        // new text. Note that the TerminalModel lock is instantly dropped after this expression,
        // since it's stored in a temporary variable.
        //
        // When `FeatureFlag::AgentView` is enabled, blocks are attachable as AI context in terminal
        // mode. Selections are preserved so they can be attached to the query when entering the
        // agent view.
        if !self.ai_input_model.as_ref(ctx).is_ai_input_enabled()
            && !FeatureFlag::AgentView.is_enabled()
        {
            self.model.lock().block_list_mut().clear_selection();
        }

        ctx.focus(&self.editor);
        self.editor.update(ctx, |editor, ctx| match edit_origin {
            EditOrigin::UserTyped => editor.user_insert(text, ctx),
            EditOrigin::UserInitiated => {
                editor.user_initiated_insert(text, PlainTextEditorViewAction::SystemInsert, ctx)
            }
            EditOrigin::SystemEdit => {
                editor.system_insert(text, PlainTextEditorViewAction::SystemInsert, ctx)
            }
            EditOrigin::SyncedTerminalInput | EditOrigin::RemoteEdit => (),
        });
        ctx.notify();
        true
    }

    /// Returns the operations for any edits made to the latest buffer.
    pub fn latest_buffer_operations(&self) -> impl Iterator<Item = &CrdtOperation> {
        self.latest_buffer_operations.iter()
    }

    /// Applies the `operations` if the block ID of this buffer
    /// is equal to `block_id`. Otherwise, queues up these operations
    /// to be processed eventually when the block IDs are equal.
    pub fn process_remote_edits(
        &mut self,
        block_id: &BlockId,
        operations: Vec<CrdtOperation>,
        ctx: &mut ViewContext<Self>,
    ) {
        // We check the `block_id` against the cached latest block ID
        // rather than the latest terminal model state because the terminal
        // model can be updated off of the main thread. This can cause
        // scenarios where the terminal model has a new active block ID but
        // we haven't processed block completed events yet.
        //
        // Although we're checking against a potentially old block ID here,
        // we'll flush the right ops when we handle the block completed events.
        if block_id == &self.deferred_remote_operations.latest_block_id {
            self.editor.update(ctx, |editor, ctx| {
                editor.apply_remote_operations(operations, ctx);
            });
        } else {
            self.deferred_remote_operations
                .defer(block_id.clone(), operations);
        }
    }

    /// Updates the latest block ID to be equal to the latest block ID known to the terminal model
    /// and flushes any previously-deferred operations for this new block ID.
    pub fn refresh_deferred_remote_operations(&mut self, ctx: &mut ViewContext<Self>) {
        let latest_block_id = self.model.lock().block_list().active_block_id().clone();
        self.deferred_remote_operations.latest_block_id = latest_block_id;
        self.flush_deferred_remote_operations(ctx);
    }

    /// Flushes any deferred remote operations for the latest known block ID.
    fn flush_deferred_remote_operations(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(operations) = self.deferred_remote_operations.flush() {
            self.editor.update(ctx, |editor, ctx| {
                editor.apply_remote_operations(operations, ctx);
            });
        }
    }

    /// Resets state in the input box that depends on the block lifecycle.
    /// This is on a performance-sensitive path.
    ///
    /// If the newly created block is for an executed user command, the input buffer is cleared.
    pub fn handle_block_completed_event(
        &mut self,
        block_completed_event: BlockCompletedEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        // We clear the input box after executing a command here instead of where we
        // execute a command to avoid the input box flashing when its contents are
        // cleared. For the multiline input box case, this also caused contents to go
        // off the screen because we were forcing the long running command to be the same
        // size of the cleared input box.
        if let BlockType::User(user_block) = &block_completed_event.block_type {
            // During cloud-mode setup (before the first exchange) the cloud agent (sharer) runs
            // environment setup commands the viewer never requested. Each completed setup block
            // would otherwise reinitialize the buffer and wipe a follow-up the viewer is composing,
            // so skip the clear for that window.
            let cloud_setup_pre_first_exchange = FeatureFlag::CloudModeSetupV2.is_enabled()
                && is_cloud_agent_pre_first_exchange(
                    self.ambient_agent_view_model(),
                    &self.agent_view_controller,
                    &self.model.lock(),
                    ctx,
                );
            // Only clear the input buffer for user-executed commands, not agent-executed ones.
            let should_clear_buffer = !user_block.was_part_of_agent_interaction
                && !cloud_setup_pre_first_exchange
                && !self.has_queued_command_in_flight(ctx);
            let latest_block_id = self.model.lock().block_list().active_block_id().clone();
            // Prefer a prompt-chip restore (e.g. `cd`) over a shell-widget handoff restore.
            let completed_handoff = self
                .pending_shell_widget_handoff
                .take_if(|handoff| handoff.block_id == block_completed_event.block_id);
            if let Some(handoff) = &completed_handoff {
                self.model
                    .lock()
                    .block_list_mut()
                    .hide_block(&handoff.block_id);
            }
            let pending_input_restore = self
                .input_contents_before_prompt_chip_command
                .take()
                .or_else(|| {
                    completed_handoff
                        .as_ref()
                        .map(|handoff| handoff.restore_text().to_string())
                });

            if should_clear_buffer {
                // We want to reinitialize the buffer whenever a command is completed so that
                // state does not leak from buffer to buffer (e.g. edit history).
                if self.deferred_remote_operations.latest_block_id != latest_block_id {
                    self.deferred_remote_operations.latest_block_id = latest_block_id;
                    self.editor
                        .update(ctx, |editor, ctx| editor.reinitialize_buffer(None, ctx));
                    self.latest_buffer_operations = Vec::new();

                    // If we have a pending input restore (from a prompt chip command like cd, or
                    // a ctrl-r/ctrl-t external handoff), restore the input contents instead of
                    // leaving the buffer empty.
                    if let Some(restore_text) = pending_input_restore {
                        self.editor.update(ctx, |editor, ctx| {
                            editor.set_buffer_text(&restore_text, ctx);
                            if let Some(handoff) = &completed_handoff {
                                match (
                                    handoff.apply_mode,
                                    &handoff.selection,
                                    handoff.cursor_offset,
                                ) {
                                    (
                                        ShellWidgetApplyMode::Splice,
                                        Some(insertion),
                                        Some(cursor_offset),
                                    ) => editor.select_and_replace(
                                        insertion,
                                        [cursor_offset..cursor_offset],
                                        PlainTextEditorViewAction::InsertSelectedText,
                                        ctx,
                                    ),
                                    (_, None, Some(cursor_offset)) => editor
                                        .select_ranges_by_byte_offset(
                                            [cursor_offset..cursor_offset],
                                            ctx,
                                        ),
                                    (ShellWidgetApplyMode::Replace, Some(_), _)
                                    | (ShellWidgetApplyMode::Splice, Some(_), None)
                                    | (_, None, None) => {}
                                }
                            }
                        });
                        self.is_editor_empty_on_last_edit = false;
                    } else {
                        // This is the one place where buffer contents can change without an `Edit`
                        // -- this is because the buffer semantically isn't being edited, a new one is
                        // being constructed. We can guarantee in this case that the buffer was previously
                        // non-empty and should emit this event, because this code path is executed upon block
                        // completion in response to an executed command, though this guarantee is not explicitly
                        // enforced by the code.
                        self.is_editor_empty_on_last_edit = true;
                        ctx.emit(Event::InputEmptyStateChanged {
                            is_empty: true,
                            reason: InputEmptyStateChangeReason::UserCommandCompleted,
                        });
                    }
                }
            } else {
                // For agent-executed commands, still update the latest block ID but don't clear the buffer
                if self.deferred_remote_operations.latest_block_id != latest_block_id {
                    self.deferred_remote_operations.latest_block_id = latest_block_id;
                }
            }

            // Make sure the viewer's interaction state is correct based on their role.
            // We may have locked up their input if they tried to execute a command.
            if let SharedSessionStatus::ActiveViewer { role } =
                self.model.lock().shared_session_status()
            {
                self.editor.update(ctx, |editor, ctx| {
                    editor.set_interaction_state(role.into(), ctx);

                    // Also need to set the text colors back to normal.
                    let appearance: &Appearance = Appearance::as_ref(ctx);
                    editor.set_text_colors(TextColors::from_appearance(appearance), ctx);
                });

                if let Some(shared_session_input_state) = self.shared_session_input_state.as_mut() {
                    shared_session_input_state.pending_command_execution_request = None;
                };
            }

            // Update the segmented control disabled state based on the new state.
            self.universal_developer_input_button_bar
                .update(ctx, |button_bar, ctx| {
                    button_bar.update_segmented_control_disabled_state(ctx);
                });

            // Generate autosuggestion if the input is not empty (user had type-ahead).
            self.maybe_generate_autosuggestion(ctx);
        }

        self.input_render_state_model_handle
            .update(ctx, |input_render_state_model, _| {
                input_render_state_model.set_editor_modified_since_block_finished(false);
            });

        // Re-render for anything that depends on the block list (e.g. zero state AM chips).
        ctx.notify();
    }

    /// Performs any post-block completion processing that's relevant to the input.
    ///
    /// This is triggered after [`Self::handle_block_completed_event`] as
    /// the handling of the main block completed event is a sensitive path.
    pub fn handle_after_block_completed_event(
        &mut self,
        block: BlockType,
        ctx: &mut ViewContext<Self>,
    ) {
        if let BlockType::User(block_completed) = block {
            self.last_user_block_completed = Some(block_completed.clone());

            let is_in_fullscreen_agent_view =
                self.agent_view_controller.as_ref(ctx).is_fullscreen();
            self.ai_input_model.update(ctx, |ai_input_model, ctx| {
                // If the user has autodetection enabled, unlock the input mode.
                // Otherwise, keep it locked in the current mode.
                let new_config = ai_input_model
                    .input_config()
                    .unlocked_if_autodetection_enabled(is_in_fullscreen_agent_view, ctx);
                ai_input_model.set_input_config(new_config, false, None, ctx);
            });

            let viewing_shared_session = self.model.lock().shared_session_status().is_viewer();
            if viewing_shared_session {
                // As we switch to the new block ID, if there were any remote
                // edits that were pending for that block ID, we should flush them.
                // Today, we only expect this to be the case with session-sharing viewers.
                self.flush_deferred_remote_operations(ctx);

                // Update shared session history model
                match self
                    .shared_session_input_state
                    .as_ref()
                    .map(|state| state.history_model.clone())
                {
                    Some(shared_session_history_model) => {
                        let command = block_completed
                            .command
                            .get_with(|compute| {
                                let model = self.model.lock();
                                compute(model.block_list())
                            })
                            .to_owned();
                        let serialized_block =
                            block_completed.serialized_block.get_with(|compute| {
                                let model = self.model.lock();
                                compute(model.block_list())
                            });
                        shared_session_history_model.update(ctx, move |history_model, _ctx| {
                            history_model
                                .push(HistoryEntry::for_completed_block(command, serialized_block))
                        })
                    }
                    _ => {
                        log::warn!("Tried to access non-existent shared session history model")
                    }
                }
            }

            ctx.emit(Event::InputStateChanged(InputState::Enabled));
        } else if block.is_bootstrap_block()
            && self
                .model
                .lock()
                .block_list()
                .is_bootstrapping_precmd_done()
        {
            // When a bootstrap block is completed and the session is now
            // post-bootstrap, post-precmd, we know that the active block ID
            // is the block ID that we want to key input updates off of
            // (the block IDs during bootstrap are meaningless).
            self.refresh_deferred_remote_operations(ctx);

            // If the user typed ahead during bootstrap, the autosuggestion and
            // completions-as-you-type requests were silently skipped (history
            // wasn't queryable, session ID was absent). Now that bootstrap is
            // done, retry them so ghost text appears without the user having to
            // re-type.
            if !self.buffer_text(ctx).is_empty() {
                self.maybe_generate_autosuggestion(ctx);

                if self.should_show_completions_while_typing(ctx) {
                    self.open_completion_suggestions(CompletionsTrigger::AsYouType, ctx);
                }
            }
        }
    }

    /// 'Starts' the active block and sends its command bytes to the pty.
    ///
    /// Additionally, the executed command is recorded to history if appropriate.
    fn start_block_and_write_command_to_pty(
        &mut self,
        command: &str,
        source: CommandExecutionSource,
        should_add_command_to_history: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        start_trace!("command_execution:start");

        // Abort running completions since we're about to execute a command.
        if let Some(abort_handle) = self.completions_abort_handle.take() {
            abort_handle.abort();
        }
        self.abort_latest_autosuggestion_future();

        if let Some(future_handle) = self.decorations_future_handle.take() {
            future_handle.abort_handle().abort();
        }

        let session_id = self
            .active_block_session_id()
            .expect("session_id should be set (via bootstrap) before executing command");

        // If the SelectedWorkflowState is populated with a workflow, we count this as a workflow execution.
        let (workflow_id, workflow_command) = {
            match self.workflows_state.selected_workflow_state.as_ref() {
                Some(selected_workflow_state) => {
                    send_telemetry_from_ctx!(
                        TelemetryEvent::WorkflowExecuted(WorkflowTelemetryMetadata {
                            workflow_source: selected_workflow_state.workflow_source,
                            workflow_categories: selected_workflow_state
                                .workflow_type
                                .as_workflow()
                                .tags()
                                .cloned(),
                            workflow_selection_source: selected_workflow_state
                                .workflow_selection_source,
                            // This is only `Some()` for WarpDrive workflows; we don't track
                            // ID for execution of local workflows because they have no such
                            // unique ID.
                            workflow_id: selected_workflow_state.workflow_type.server_id(),
                            workflow_space: match &selected_workflow_state.workflow_type {
                                WorkflowType::Cloud(workflow) => Some(workflow.space(ctx).into()),
                                _ => None,
                            },
                            enum_ids: selected_workflow_state
                                .workflow_type
                                .as_workflow()
                                .get_server_enum_ids()
                        }),
                        ctx
                    );

                    let workflow_type = &selected_workflow_state.workflow_type;
                    let workflow_id = match workflow_type {
                        WorkflowType::Cloud(workflow) => Some(workflow.id),
                        _ => None,
                    };

                    // If the SelectedWorkflowState is populated, then we're always able to return the workflow command.
                    // The case where workflow_id = None but workflow_command = Some() is when it's a local workflow, which
                    // don't have ids and are tracked just by persisting the workflow contents. This is a little janky and would
                    // be fixed if we could identify all workflows under a unified id system, not just cloud ones.
                    (
                        workflow_id,
                        workflow_type
                            .as_workflow()
                            .command()
                            .map(|command| command.to_owned()),
                    )
                }
                None => (None, None),
            }
        };

        ctx.emit(Event::ExecuteCommand(Box::new(ExecuteCommandEvent {
            command: command.to_string(),
            workflow_id,
            session_id,
            workflow_command,
            should_add_command_to_history,
            source,
        })));
        end_trace!();
    }

    pub fn notify_and_notify_children(&self, ctx: &mut ViewContext<Self>) {
        ctx.notify();
        // The left notch may have been updated due to the prompt updating, in the case of
        // same-line prompt!
        self.editor.update(ctx, |_editor, ctx| {
            ctx.notify();
        });
    }

    /// Returns a tuple (prompt_text, rprompt_text).
    pub fn prompt_and_rprompt_text(&self, app: &AppContext) -> (String, Option<String>) {
        let model = self.model.lock();
        let appearance = Appearance::as_ref(app);
        let (lprompt_top, lprompt_bottom, rprompt) = self
            .prompt_render_helper
            .render_prompt(&model, appearance, app);
        // Separate this into a helper (follow-up PR?)

        let show_universal_developer_input = self.should_show_universal_developer_input(app);

        let lprompt_top_text = lprompt_top.map(|rendered| rendered.element.text(app));
        let lprompt_bottom_text = lprompt_bottom.map(|rendered| rendered.element.text(app));
        let rprompt_text = rprompt.map(|rendered| rendered.element.text(app));
        if should_render_prompt_on_same_line(show_universal_developer_input, &model, app) {
            if let Some(lprompt_top_text) = lprompt_top_text {
                (
                    lprompt_top_text + "\n" + &lprompt_bottom_text.unwrap_or_default(),
                    rprompt_text,
                )
            } else {
                (lprompt_bottom_text.unwrap_or_default(), rprompt_text)
            }
        } else {
            (lprompt_top_text.unwrap_or_default(), rprompt_text)
        }
    }

    pub fn create_prompt_elements(&self, app: &AppContext) -> SessionNavigationPromptElements {
        let model = self.model.lock();
        let block = self.prompt_render_helper.prompt_block(&model);
        let is_udi = InputSettings::as_ref(app).is_universal_developer_input_enabled(app);
        let mut prompt_elements = SessionNavigationPromptElements {
            ps1_prompt_grid: None,
            prompt_chip_snapshot: None,
        };

        if let Some(block) = block
            && !is_udi
            && block.honor_ps1()
            && model.block_list().is_bootstrapped()
        {
            // PS1 mode: capture the raw prompt grid so the command palette
            // can render it with full fidelity (CORE-1683).
            prompt_elements.ps1_prompt_grid = Some(block.prompt_grid().clone());
        }

        // Always capture a chip snapshot as the fallback prompt representation.
        // This covers both UDI mode and any edge cases where PS1 is not available
        // (e.g. not yet bootstrapped, block-level honor_ps1 mismatch).
        if prompt_elements.ps1_prompt_grid.is_none() {
            prompt_elements.prompt_chip_snapshot = Some(self.prompt_type.as_ref(app).snapshot(app));
        }
        prompt_elements
    }

    /// This function determines if the subshell flag should be in the input editor. The flag
    /// should show here if there are no blocks in the block list for this subshell session, which
    /// will be the case if no non-hidden blocks have been executed yet or the block list was
    /// cleared.
    fn get_subshell_flag_render_state(
        &self,
        model: &TerminalModel,
        spacing_is_compact: bool,
        app: &AppContext,
    ) -> Option<SubshellRenderState> {
        if spacing_is_compact {
            return None;
        }
        let session_id = self.active_block_session_id()?;
        let should_render = self
            .sessions
            .as_ref(app)
            .get(session_id)
            .and_then(|session| {
                session.subshell_info().as_ref().map(|info| {
                    if let Some(env_var_collection_name) = &info.env_var_collection_name {
                        Some(SubshellRenderState::Flag(SubshellSource::EnvVarCollection(
                            env_var_collection_name.to_owned(),
                        )))
                    } else {
                        info.spawning_command.split_whitespace().next().map(|exec| {
                            SubshellRenderState::Flag(SubshellSource::Command(exec.to_owned()))
                        })
                    }
                })
            })?;

        let block_list = model.block_list();
        let block_before_active_block = block_list
            .prev_non_hidden_block_from_index(block_list.active_block_index())
            .and_then(|index| block_list.block_at(index));

        match block_before_active_block {
            // If there is a block before the editor, and it belongs to this same subshell session,
            // the flag will be in the block list, and hence doesn't need to be in the editor.
            // Only extend the flag into the editor.
            Some(block) if block.session_id() == Some(session_id) => {
                Some(SubshellRenderState::Flagpole)
            }
            // Otherwise, this editor (the active block) is the first in this subshell session, and
            // we should show the flag here.
            _ => should_render,
        }
    }

    pub fn set_active_block_metadata(
        &mut self,
        active_block_metadata: BlockMetadata,
        is_after_in_band_command: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let active_session = active_block_metadata
            .session_id()
            .and_then(|session_id| self.sessions.as_ref(ctx).get(session_id));
        if let Some(session) = active_session {
            let transformer: Option<PathTransformerFn> = session
                .windows_path_converter()
                .map(|convert| Box::new(convert) as PathTransformerFn);
            self.editor.update(ctx, |editor, _| {
                editor.set_shell_family(session.shell().shell_type().into());
                editor.set_drag_drop_path_transformer(transformer);
            });
            self.input_suggestions.update(ctx, |input_suggestions, _| {
                input_suggestions.set_path_separators(session.path_separators());
            });
        }
        self.active_block_metadata = Some(active_block_metadata);

        // If needed, update the prompt display with the now-available session
        // context. In-band commands don't meaningfully change block metadata,
        // so only update prompt display chips if the previous block was not an
        // in-band command (i.e.: was probably a user-executed block).
        //
        // If we update the prompt display chips here, we can get into infinite
        // loops where we run an in-band command to compute an updated value for
        // a chip (e.g.: listing the files in the current directory), which
        // triggers another in-band command, etc. etc.
        if !is_after_in_band_command {
            self.update_prompt_display_chips(ctx);
        }
    }

    pub fn update_prompt_display_chips(&mut self, ctx: &mut ViewContext<Self>) {
        let session_context = self.completion_session_context(ctx);

        self.prompt_render_helper
            .prompt_view()
            .update(ctx, |prompt, prompt_ctx| {
                prompt.update_session_context(session_context.clone(), prompt_ctx);
            });

        self.agent_input_footer.update(ctx, |footer, footer_ctx| {
            footer.update_session_context(session_context, footer_ctx);
        });
    }

    pub fn update_repo_path(&mut self, repo_path: Option<PathBuf>, ctx: &mut ViewContext<Self>) {
        self.prompt_render_helper
            .prompt_view()
            .update(ctx, |prompt, prompt_ctx| {
                prompt.update_repo_path(repo_path.clone(), prompt_ctx);
            });

        self.agent_input_footer.update(ctx, |footer, footer_ctx| {
            footer.set_current_repo_path(repo_path.clone(), footer_ctx);
        });
    }

    fn active_session_path_if_local(&self, ctx: &ViewContext<Self>) -> Option<&Path> {
        self.active_block_session_id().and_then(|session_id| {
            let current_session = self.sessions.as_ref(ctx).get(session_id)?;
            if current_session.is_local() {
                self.active_block_metadata
                    .as_ref()
                    .and_then(BlockMetadata::current_working_directory)
                    .map(Path::new)
            } else {
                None
            }
        })
    }

    fn render_attachment_chips(&self, appearance: &Appearance) -> Option<Box<dyn Element>> {
        if self.attachment_chips.is_empty() {
            None
        } else {
            let chips = self
                .attachment_chips
                .iter()
                .map(|chip| self.render_attached_chip(chip, appearance));

            Some(
                Wrap::row()
                    .with_run_spacing(spacing::UDI_CHIP_MARGIN)
                    .with_main_axis_alignment(MainAxisAlignment::Start)
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_children(chips)
                    .finish(),
            )
        }
    }

    fn render_attached_chip(
        &self,
        chip: &AttachmentChip,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let delete_chip_index = chip.index;
        let close_button = appearance
            .ui_builder()
            .close_button(
                appearance.monospace_font_size(),
                chip.mouse_state_handle.clone(),
            )
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(TerminalAction::DeleteAttachment {
                    index: delete_chip_index,
                });
            })
            .finish();

        let icon = match chip.attachment_type {
            AttachmentType::Image => Icon::Image,
            AttachmentType::File => Icon::File,
        };

        let attachment_chip = Chip::new(
            chip.file_name.clone(),
            UiComponentStyles {
                margin: Some(Coords {
                    top: 0.,
                    bottom: 0.,
                    left: 0.,
                    right: 6.,
                }),
                font_family_id: Some(appearance.ui_font_family()),
                font_size: Some(appearance.monospace_font_size()),
                font_color: Some(blended_colors::text_main(
                    appearance.theme(),
                    appearance.theme().background(),
                )),
                border_width: Some(1.),
                border_color: Some(internal_colors::neutral_4(appearance.theme()).into()),
                border_radius: Some(CornerRadius::with_all(Radius::Pixels(5.))),
                ..Default::default()
            },
        )
        .with_icon(icon.to_warpui_icon(
            blended_colors::text_main(appearance.theme(), appearance.theme().background()).into(),
        ))
        .with_close_button(close_button)
        .build();

        if matches!(chip.attachment_type, AttachmentType::Image) {
            let preview_chip_index = chip.index;
            EventHandler::new(attachment_chip.finish())
                .on_left_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action(TerminalAction::OpenAttachmentLightbox {
                        index: preview_chip_index,
                    });
                    DispatchEventResult::StopPropagation
                })
                .finish()
        } else {
            attachment_chip.finish()
        }
    }

    fn render_input_box(
        &self,
        show_vim_status: bool,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        // Set editor height to be half of the terminal view height
        let editor_height = self.size_info(app).pane_height_px() / 2.0.into_pixels();

        // Round down editor height to be divisible by line height so we do not see partial lines
        let line_height = self
            .editor
            .as_ref(app)
            .line_height(app.font_cache(), appearance)
            .into_pixels();
        let editor_height_rounded_down =
            (editor_height / line_height).round().max(1.0.into_pixels()) * line_height;

        let terminal_settings = TerminalSettings::as_ref(app);
        let terminal_spacing =
            terminal_settings.terminal_input_spacing(appearance.line_height_ratio(), app);
        let mut bottom_padding = terminal_spacing.editor_bottom_padding;

        // When `FeatureFlag::AgentView` is enabled, always render with UDI-style spacing values,
        // regardless of terminal/agent mode or prompt setting.
        let is_udi_style_spacing =
            self.should_show_universal_developer_input(app) || FeatureFlag::AgentView.is_enabled();

        let is_compact_mode =
            matches!(terminal_settings.spacing_mode.value(), SpacingMode::Compact)
                && !is_udi_style_spacing;

        // In compact mode, allocate some extra padding for the Vim status bar.
        if is_compact_mode && show_vim_status {
            bottom_padding = VIM_STATUS_BAR_BOTTOM_PADDING;
        }

        if is_udi_style_spacing {
            bottom_padding = terminal_spacing.editor_bottom_padding - 4.;
        }

        let input_box = Container::new(
            ConstrainedBox::new(Clipped::new(ChildView::new(&self.editor).finish()).finish())
                .with_max_height(editor_height_rounded_down.as_f32())
                .finish(),
        )
        .with_padding_right(*TERMINAL_VIEW_PADDING_LEFT)
        .with_padding_bottom(bottom_padding)
        .finish();

        let input_editor_save_position_id = self.editor_save_position_id();
        SavePosition::new(
            EventHandler::new(input_box)
                .on_right_mouse_down(move |ctx, app, position, modifiers| {
                    if should_right_click_paste(modifiers.shift, app) {
                        // Same path as the `terminal:paste` keybinding, so escaped-path
                        // processing and CLI-agent image handling behave identically.
                        ctx.dispatch_typed_action(TerminalAction::Paste);
                        return DispatchEventResult::StopPropagation;
                    }

                    let input_rect = ctx
                        .element_position_by_id(input_editor_save_position_id.clone())
                        .expect("input editor position id should be saved");
                    let offset_position = position - input_rect.origin();
                    ctx.dispatch_typed_action(TerminalAction::OpenInputContextMenu {
                        position: offset_position,
                    });
                    DispatchEventResult::StopPropagation
                })
                .finish(),
            &self.editor_save_position_id(),
        )
        .finish()
    }

    /// Returns whether the buffer contains any attachment patterns (blocks, drive objects, or diffs).
    /// These patterns indicate the user is referencing context that requires AI mode.
    fn buffer_contains_attachment_patterns(buffer_text: &str) -> bool {
        BLOCK_CONTEXT_ATTACHMENT_REGEX.is_match(buffer_text)
            || DRIVE_OBJECT_ATTACHMENT_REGEX.is_match(buffer_text)
            || DIFF_HUNK_ATTACHMENT_REGEX.is_match(buffer_text)
    }

    /// Returns the SavePosition ID for the input.
    ///
    /// This may be used by parent views to position UI elements relative to the input.
    pub fn save_position_id(&self) -> String {
        format!("input_{}", self.view_id)
    }

    /// Returns the position ID for the input editor
    pub fn editor_save_position_id(&self) -> String {
        format!("input_editor_{}", self.view_id)
    }

    /// Returns the position ID for the (left) prompt.
    pub fn prompt_save_position_id(&self) -> String {
        format!("prompt_area_{}", self.view_id)
    }

    /// A save position for the bordered input alone,
    /// not including the status bar.
    pub fn status_free_input_save_position_id(&self) -> String {
        format!("status_free_input_{}", self.view_id)
    }

    /// Returns a reference to the universal developer input button bar, if it exists
    pub fn universal_developer_input_button_bar(
        &self,
    ) -> &ViewHandle<UniversalDeveloperInputButtonBar> {
        &self.universal_developer_input_button_bar
    }

    pub fn should_show_universal_developer_input(&self, app: &AppContext) -> bool {
        InputSettings::as_ref(app).is_universal_developer_input_enabled(app)
    }

    /// Returns whether the input box is currently pinned to the top of the screen.
    fn is_input_at_top(&self, model: &TerminalModel, ctx: &AppContext) -> bool {
        match InputModeSettings::as_ref(ctx).input_mode.value() {
            InputMode::PinnedToBottom => false,
            InputMode::PinnedToTop => true,
            InputMode::Waterfall => model.is_block_list_empty(),
        }
    }
}

impl Entity for Input {
    type Event = Event;
}

impl TypedActionView for Input {
    type Action = InputAction;

    fn action_accessibility_contents(
        &mut self,
        action: &InputAction,
        _: &mut ViewContext<Self>,
    ) -> ActionAccessibilityContent {
        match action {
            InputAction::FocusInputBox => {
                ActionAccessibilityContent::Custom(AccessibilityContent::new(
                    INPUT_A11Y_LABEL,
                    // TODO (a11y) use bindings from user settings
                    INPUT_A11Y_HELPER,
                    WarpA11yRole::TextareaRole,
                ))
            }
            _ => ActionAccessibilityContent::Empty,
        }
    }

    fn handle_action(&mut self, action: &InputAction, ctx: &mut ViewContext<Self>) {
        match action {
            InputAction::FocusInputBox => self.focus_input_box(ctx),
            InputAction::Up => self.editor_up(ctx),
            InputAction::PageUp => self.editor_page_up(ctx),
            InputAction::PageDown => self.editor_page_down(ctx),
            InputAction::CtrlD => self.ctrl_d(ctx),
            InputAction::CtrlR => self.ctrl_r(ctx),
            InputAction::ClearScreen => self.clear_screen(ctx),
            InputAction::MaybeOpenCompletionSuggestions => {
                self.maybe_open_completion_suggestions(ctx);
            }
            InputAction::HideWorkflowInfoCard => self.hide_workflows_info_box(ctx),
            InputAction::ToggleClassicCompletionsMode => {
                InputSettings::handle(ctx).update(ctx, |settings, ctx| {
                    if let Err(e) = settings.classic_completions_mode.toggle_and_save_value(ctx) {
                        log::warn!(
                            "Failed to toggle and save classic completions mode setting: {e}."
                        )
                    }
                });
            }
            InputAction::ToggleInputAutoDetection => {
                if let Ok(new_value) =
                    AISettings::handle(ctx).update(ctx, |ai_settings, model_ctx| {
                        ai_settings
                            .ai_autodetection_enabled_internal
                            .toggle_and_save_value(model_ctx)
                    })
                {
                    send_telemetry_from_ctx!(
                        TelemetryEvent::AgentModeToggleAutoDetectionSetting {
                            is_autodetection_enabled: new_value,
                            origin: AgentModeAutoDetectionSettingOrigin::Banner
                        },
                        ctx
                    );
                }
            }
            InputAction::EnableAutoDetection => {
                // Call the same logic that clicking the lightbulb icon triggers
                self.handle_universal_developer_input_button_bar_event(
                    &UniversalDeveloperInputButtonBarEvent::EnableAutoDetection,
                    ctx,
                );
            }
            InputAction::ToggleAgentViewShortcuts => {
                self.agent_shortcut_view_model.update(ctx, |model, ctx| {
                    if model.is_shortcut_view_open() {
                        model.hide_shortcut_view(ctx);
                    } else {
                        model.open_shortcut_view(ctx);
                    }
                });
            }
            InputAction::SetUDIHovered(is_hovered) => {
                self.universal_developer_input_button_bar
                    .update(ctx, |button_bar, ctx| {
                        button_bar.set_udi_hovered(*is_hovered, ctx);
                    });
            }
            InputAction::UpdateCompletionsMenuWidth(width) => {
                InputSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.completions_menu_width.set_value(*width, ctx));
                });
            }
            InputAction::UpdateCompletionsMenuHeight(height) => {
                InputSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.completions_menu_height.set_value(*height, ctx));
                });
            }
            InputAction::StartNewAgentConversation { origin } => {
                // Block starting a new conversation if the agent is in control of a long-running command
                if !self
                    .ai_context_model
                    .as_ref(ctx)
                    .can_start_new_conversation()
                {
                    let window_id = ctx.window_id();
                    ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                        toast_stack.add_ephemeral_toast(
                            DismissibleToast::error(
                                "Cannot start a new conversation while agent is monitoring a command.".to_string()
                            ),
                            window_id,
                            ctx,
                        );
                    });
                    return;
                }

                if FeatureFlag::AgentView.is_enabled() {
                    if let AgentViewEntryOrigin::Keybinding(keystroke) = origin {
                        let should_start_new_conversation =
                            self.agent_view_controller.update(ctx, |controller, ctx| {
                                controller.should_start_new_conversation_for_keystroke(
                                    keystroke.clone(),
                                    ctx,
                                )
                            });
                        if !should_start_new_conversation {
                            return;
                        }
                    }
                    ctx.emit(Event::EnterAgentView {
                        initial_prompt: None,
                        conversation_id: None,
                        origin: origin.clone(),
                    });
                } else if self.should_show_universal_developer_input(ctx) {
                    // Clear follow-up state (start a fresh conversation)
                    self.ai_context_model.update(ctx, |ai_context_model, ctx| {
                        ai_context_model
                            .set_pending_query_state_for_new_conversation(origin.clone(), ctx);
                    });
                    self.enter_ai_mode(
                        Some(InputTypeAutoDetectionSource::StartNewConversation),
                        ctx,
                    );
                }
            }
            InputAction::FigmaAddButtonClicked => {
                TemplatableMCPServerManager::handle(ctx).update(ctx, |manager, ctx| {
                    manager.install_figma_from_gallery(ctx);
                });
            }
            InputAction::FigmaEnableButtonClicked => {
                TemplatableMCPServerManager::handle(ctx).update(ctx, |manager, ctx| {
                    manager.enable_figma_mcp(ctx);
                });
            }
            InputAction::ClearAttachedContext => {
                self.clear_attached_context(ctx);
            }
            InputAction::ActivateCloudHandoff => {
                self.activate_cloud_handoff_compose(HandoffEntryPoint::Ampersand, ctx);
            }
        }
    }
}

impl View for Input {
    fn ui_name() -> &'static str {
        "Input"
    }

    fn accessibility_contents(&self, _: &AppContext) -> Option<AccessibilityContent> {
        Some(AccessibilityContent::new(
            INPUT_A11Y_LABEL,
            // TODO (a11y) use bindings from user settings
            INPUT_A11Y_HELPER,
            WarpA11yRole::TextareaRole,
        ))
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            if self.prompt_render_helper.has_open_chip_menu(ctx) {
                // Focus the PromptDisplay, which will in turn focus any open chip menu
                ctx.focus(self.prompt_render_helper.prompt_view());
            } else if self.agent_input_footer.as_ref(ctx).has_open_chip_menu(ctx) {
                // Focus the AgentInputFooter, which will in turn focus any open chip menu
                ctx.focus(&self.agent_input_footer);
            } else {
                ctx.focus(&self.editor);
                ctx.notify();
            }
            ctx.dispatch_typed_action(&PaneGroupAction::HandleFocusChange);
        }
    }

    fn keymap_context(&self, app: &AppContext) -> warpui::keymap::Context {
        let mut ctx = Self::default_keymap_context();
        let ai_settings = AISettings::as_ref(app);

        if self.ai_input_model.as_ref(app).is_ai_input_enabled() {
            ctx.set.insert("AIInput");
        }

        if InputSettings::as_ref(app).is_universal_developer_input_enabled(app) {
            ctx.set.insert("UniversalDeveloperInput");
        }

        if self.ai_input_model.as_ref(app).is_ai_input_enabled() {
            ctx.set.insert(flags::AGENT_MODE_INPUT);
        } else {
            ctx.set.insert(flags::TERMINAL_MODE_INPUT);
        }

        if self.ai_input_model.as_ref(app).is_input_type_locked() {
            ctx.set.insert(flags::LOCKED_INPUT);
        }

        // Keep Input's keymap context in sync with TerminalView's context for AgentView-related
        // bindings (e.g. cmd-i).
        if FeatureFlag::AgentView.is_enabled() {
            ctx.set.insert(flags::AGENT_VIEW_ENABLED);
            let agent_view_state = self.agent_view_controller.as_ref(app).agent_view_state();
            if agent_view_state.is_fullscreen() {
                ctx.set.insert(flags::ACTIVE_AGENT_VIEW);
            } else if agent_view_state.is_inline() {
                ctx.set.insert(flags::ACTIVE_INLINE_AGENT_VIEW);
            }
        }

        if self.buffer_text(app).is_empty() {
            ctx.set.insert(flags::EMPTY_INPUT_BUFFER);
        }

        if ai_settings.is_any_ai_enabled(app) {
            ctx.set.insert(flags::IS_ANY_AI_ENABLED);
        }

        if ai_settings.is_ai_autodetection_enabled(app) {
            ctx.set.insert(flags::AI_INPUT_AUTODETECTION_FLAG);
        }

        if let Some(workflow) = self.workflows_state.selected_workflow_state.clone()
            && workflow.should_show_more_info_view
        {
            ctx.set.insert("WorkflowInfoBox");
        }

        let is_v2_host_selector_open = self
            .host_selector()
            .is_some_and(|view| view.as_ref(app).is_menu_open());
        let is_v2_harness_selector_open = self
            .harness_selector()
            .is_some_and(|view| view.as_ref(app).is_menu_open());
        let is_v2_environment_selector_open = self
            .agent_input_footer
            .as_ref(app)
            .is_v2_environment_selector_open(app);
        if is_v2_host_selector_open
            || is_v2_harness_selector_open
            || is_v2_environment_selector_open
        {
            ctx.set.insert("AgentSelectorOpen");
        }

        if self.prompt_render_helper.has_open_chip_menu(app)
            || self.agent_input_footer.as_ref(app).has_open_chip_menu(app)
        {
            ctx.set.insert("PromptChipMenuOpen");
        }

        if BlocklistAIHistoryModel::as_ref(app)
            .all_live_conversations_for_terminal_surface(self.terminal_view_id)
            .any(|conversation| conversation.initial_user_query().is_some())
        {
            ctx.set.insert("ActiveAIConversationHasHistory");
        }

        if AppEditorSettings::as_ref(app).vim_mode_enabled() {
            ctx.set.insert("VimModeEnabled");
        }

        if let Some(VimMode::Normal) = self.editor.as_ref(app).vim_mode(app) {
            ctx.set.insert("VimNormalMode");
        }

        if self.is_editing_queued_prompt(app) {
            ctx.set.insert(QUEUED_PROMPT_INLINE_EDITOR_OPEN_CONTEXT);
        }
        let model_lock = self.model.lock();
        ctx.set
            .insert(model_lock.shared_session_status().as_keymap_context());
        if file_attach_allowed_for_shared_session(
            model_lock.shared_session_status(),
            self.ambient_agent_view_model(),
            app,
        ) {
            ctx.set.insert(CAN_ATTACH_FILE_KEY);
        }

        if model_lock
            .block_list()
            .active_block()
            .is_active_and_long_running()
        {
            ctx.set.insert("LongRunningCommand");
        }

        if model_lock.is_block_list_empty() {
            ctx.set.insert("TerminalView_EmptyBlockList");
        } else {
            ctx.set.insert("TerminalView_NonEmptyBlockList");
        }

        ctx
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let is_universal_input = self.should_show_universal_developer_input(app);
        let should_show_status_footer =
            self.ambient_agent_view_model()
                .is_some_and(|ambient_agent_model| {
                    ambient_agent_model.as_ref(app).should_show_status_footer()
                });

        if FeatureFlag::CloudMode.is_enabled() && should_show_status_footer {
            self.render_ambient_agent_status_footer(app)
        } else if FeatureFlag::AgentView.is_enabled()
            && self.agent_view_controller.as_ref(app).is_active()
        {
            self.render_agent_input(app)
        } else if FeatureFlag::AgentView.is_enabled()
            && !self.agent_view_controller.as_ref(app).is_active()
            && !should_render_ps1_prompt(&self.model.lock(), app)
        {
            self.render_terminal_input(app)
        } else if !FeatureFlag::AgentView.is_enabled() && is_universal_input {
            self.render_universal_developer_input(app)
        } else {
            self.render_classic_input(app)
        }
    }
}

impl Autosuggester for Input {
    fn on_autosuggestion_result(
        &mut self,
        result: AutoSuggestionResult,
        ctx: &mut ViewContext<Self>,
    ) {
        let buffer_text = result.buffer_text;
        if self.editor.as_ref(ctx).buffer_text(ctx) != buffer_text {
            return;
        }

        let autosuggestion_result_substring = result
            .autosuggestion_result
            .as_ref()
            .and_then(|result| result.strip_prefix(buffer_text.as_str()));

        if let Some(autosuggestion) = autosuggestion_result_substring {
            self.set_autosuggestion(autosuggestion, AutosuggestionType::Command, ctx);
        }
    }

    fn abort_latest_autosuggestion_future(&mut self) {
        if let Some(last_abort_handle) = self.autosuggestions_abort_handle.take() {
            last_abort_handle.abort();
        }
    }

    fn set_autosuggestion_future(&mut self, abort_handle: AbortHandle) {
        self.autosuggestions_abort_handle = Some(abort_handle);
    }
}

/// Returns an optional element to be rendered at the start of the editor buffer, almost like a
/// rich UI 'prefix'.
///
/// When AgentView is enabled, this is responsible for rendering the '!' shell mode indicator.
///
/// When Agent View is disabled, this renders the agent mode icon and optional follow-up icon when
/// classic input is enabled.
fn render_prefix_mode_indicator(
    prefix: &'static str,
    color: ColorU,
    appearance: &Appearance,
    em_width: f32,
    app: &AppContext,
) -> Box<dyn Element> {
    let indicator_size = ai_indicator_height(app);
    Container::new(
        ConstrainedBox::new(
            Align::new(
                Text::new(
                    prefix,
                    appearance.monospace_font_family(),
                    appearance.monospace_font_size(),
                )
                .with_color(color)
                .finish(),
            )
            .finish(),
        )
        .with_height(indicator_size)
        .with_width(indicator_size)
        .finish(),
    )
    .with_margin_right(em_width)
    .finish()
}
fn maybe_render_ai_input_indicators(
    ai_input_model: &ModelHandle<BlocklistAIInputModel>,
    ai_context_model: &ModelHandle<BlocklistAIContextModel>,
    agent_view_controller: &ModelHandle<AgentViewController>,
    handoff_compose_state: &ModelHandle<HandoffComposeState>,
    ai_follow_up_icon_mouse_state: MouseStateHandle,
    app: &AppContext,
) -> Option<Box<dyn Element>> {
    let ai_input_model = ai_input_model.as_ref(app);
    let appearance = Appearance::as_ref(app);
    let em_width = app.font_cache().em_width(
        appearance.monospace_font_family(),
        appearance.monospace_font_size(),
    );

    let is_agent_view_active = agent_view_controller.as_ref(app).is_fullscreen();
    let is_ai_input_enabled = ai_input_model.is_ai_input_enabled();
    let is_input_type_locked = ai_input_model.is_input_type_locked();

    // Show the `!` shell mode indicator when in locked shell mode inside the agent view.
    let is_locked_shell = !is_ai_input_enabled && is_input_type_locked;
    if handoff_compose_state.as_ref(app).is_active() && is_agent_view_active {
        return Some(render_prefix_mode_indicator(
            CLOUD_HANDOFF_INPUT_PREFIX,
            ai_brand_color(appearance.theme()),
            appearance,
            em_width,
            app,
        ));
    }

    if is_locked_shell && is_agent_view_active {
        return Some(render_prefix_mode_indicator(
            TERMINAL_INPUT_PREFIX,
            appearance.theme().ansi_fg_blue(),
            appearance,
            em_width,
            app,
        ));
    }

    if is_agent_view_active {
        return None;
    }

    if !ai_input_model.is_ai_input_enabled() || FeatureFlag::AgentView.is_enabled() {
        return None;
    }

    let is_universal_developer_input_enabled =
        InputSettings::as_ref(app).is_universal_developer_input_enabled(app);

    // If universal developer input is enabled, don't show any AI indicators
    if is_universal_developer_input_enabled {
        return None;
    }

    let ai_icon = render_ai_agent_mode_icon(
        app,
        AnsiColorIdentifier::Yellow.to_ansi_color(&appearance.theme().terminal_colors().normal),
    );

    let all_icons = if ai_context_model
        .as_ref(app)
        .is_targeting_existing_conversation(app)
    {
        let reply_icon = render_ai_follow_up_icon(ai_follow_up_icon_mouse_state, app);
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(ai_icon)
            .with_child(reply_icon)
            .finish()
    } else {
        ai_icon
    };
    Some(
        Container::new(all_icons)
            .with_margin_right(em_width)
            .finish(),
    )
}

#[cfg(feature = "integration_tests")]
impl Input {}

#[cfg(test)]
impl Input {
    pub fn agent_footer_chip_kinds(
        &self,
        app: &AppContext,
    ) -> (
        Vec<crate::context_chips::ContextChipKind>,
        Vec<crate::context_chips::ContextChipKind>,
    ) {
        self.agent_input_footer
            .as_ref(app)
            .displayed_chip_kinds(app)
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
