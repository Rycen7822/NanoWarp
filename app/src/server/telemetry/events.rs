use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use session_sharing_protocol::common::{ParticipantId, Role, SessionId as SharedSessionId};
use session_sharing_protocol::sharer::{SessionEndedReason, SessionSourceType};
use strum_macros::{EnumDiscriminants, EnumIter};
use warp_completer::completer::MatchType;
use warp_core::command::ExitCode;
use warp_core::interval_timer::TimingDataPoint;
use warp_core::telemetry::{
    EnablementState, TelemetryEvent as TelemetryEventTrait, TelemetryEventDesc,
};
pub use warp_terminal::ImageProtocol;
use warpui::keymap::Keystroke;
use warpui::notification::{NotificationSendError, RequestPermissionsOutcome};
use warpui::rendering::ThinStrokes;

use crate::auth::auth_manager::LoginGatedFeature;
use crate::channel::Channel;
use crate::cloud_object::model::generic_string_model::GenericStringObjectId;
use crate::cloud_object::{GenericStringObjectFormat, Space};
#[cfg(feature = "local_fs")]
use crate::code::editor_management::CodeSource;
use crate::drive::{CloudObjectTypeAndId, DriveSortOrder};
use crate::features::FeatureFlag;
use crate::launch_configs::save_modal::SaveState;
use crate::notebooks::telemetry::NotebookTelemetryAction;
use crate::notebooks::{NotebookId, NotebookLocation};
use crate::pane_group::PaneDragDropLocation;
use crate::prompt::editor_modal::OpenSource as PromptEditorOpenSource;
use crate::server::ids::ServerId;
use crate::settings::import::config::ParsedTerminalSetting;
use crate::settings::import::model::TerminalType;
use crate::tab::TabTelemetryAction;
use crate::terminal::block_list_viewport::InputMode;
use crate::terminal::input::TelemetryInputSuggestionsMode;
use crate::terminal::model::session::SessionId;
use crate::terminal::model::terminal_model::BlockSelectionCardinality;
use crate::terminal::settings::AltScreenPaddingMode;
use crate::terminal::shared_session::SharedSessionActionSource;
use crate::terminal::shell::ShellType;
use crate::terminal::view::{
    BlockEntity, BlockSelectionDetails, NotificationsDiscoveryBannerAction,
    NotificationsErrorBannerAction, NotificationsTrigger, PromptPart,
};
#[cfg(feature = "local_fs")]
use crate::util::file::external_editor::settings::EditorLayout;
#[cfg(feature = "local_fs")]
use crate::util::openable_file_type::FileTarget;
use crate::workspace::TabMovement;
use crate::workspace::tab_settings::{TabCloseButtonPosition, WorkspaceDecorationVisibility};

#[derive(Clone, Serialize, Deserialize)]
pub struct BootstrappingInfo {
    pub shell: &'static str,
    pub is_ssh: bool,
    pub is_subshell: bool,
    pub is_wsl: bool,
    pub is_msys2: bool,
    /// `true` if the bootstrapping process was triggered by an RC file snippet.
    ///
    /// This should only be true if `is_subshell` is true.
    pub was_triggered_by_rc_file: bool,
    /// The total time it took to bootstrap the shell, in seconds.
    pub bootstrap_duration_seconds: Option<f64>,
    /// The time it took to source the user's rcfiles, in seconds.  May be None
    /// if we weren't able to get that information from the shell.
    pub rcfiles_duration_seconds: Option<f64>,
    /// The difference between the total bootstrap time and the rcfile sourcing
    /// time, which roughly equals the time cost of running our bootstrap
    /// script.  Will be None if `bootstrap_duration_seconds` or
    /// `rcfiles_duration_seconds` is None.
    pub warp_attributed_bootstrap_duration_seconds: Option<f64>,
    pub shell_version: Option<String>,
    pub terminal_session_id: Option<SessionId>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SlowBootstrapInfo {
    pub shell: &'static str,
    pub is_ssh: bool,
    pub is_subshell: bool,
    pub is_wsl: bool,
    pub is_msys2: bool,
    /// Contents of the bootstrap block when the slow bootstrap was detected.
    /// This includes both command and output content from the block.
    pub bootstrap_block_contents: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AppStartupInfo {
    pub is_session_restoration_on: bool,
    /// Whether or not a screen reader is enabled at the time the app is
    /// launched.  Should be set to None if we do not know for sure.
    pub is_screen_reader_enabled: Option<bool>,
    pub timing_data: Vec<TimingDataPoint>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum DownloadSource {
    Website,
    Homebrew,
}

// For use when recording what type of cloud object a particular telemetry is for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TelemetryCloudObjectType {
    Notebook,
    Folder,
    GenericStringObject(GenericStringObjectFormat),
}

impl From<&CloudObjectTypeAndId> for TelemetryCloudObjectType {
    fn from(cloud_object_type_and_id: &CloudObjectTypeAndId) -> Self {
        match cloud_object_type_and_id {
            CloudObjectTypeAndId::Notebook(_) => Self::Notebook,
            CloudObjectTypeAndId::Folder(_) => Self::Folder,
            CloudObjectTypeAndId::GenericStringObject { object_type, .. } => {
                Self::GenericStringObject(*object_type)
            }
        }
    }
}

/// For use when recording how a user has access to a cloud object.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum TelemetrySpace {
    /// The object is owned by the current user.
    Personal,
    /// The object is owned by a team the user is on.
    Team,
    /// The object was shared with the user.
    Shared,
}

impl From<Space> for TelemetrySpace {
    fn from(space: Space) -> Self {
        match space {
            Space::Personal => Self::Personal,
            Space::Team { .. } => Self::Team,
            Space::Shared => Self::Shared,
        }
    }
}

/// Common metadata to include in all Warp Drive telemetry events that act on a specific object.
/// Events that only apply to a single object type may use [`NotebookTelemetryMetadata`]
/// or [`EnvVarTelemetryMetadata`] instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudObjectTelemetryMetadata {
    pub object_type: TelemetryCloudObjectType,
    /// The server UID of the object. This only exists for objects that have been synced to the
    /// server.
    pub object_uid: Option<ServerId>,
    /// The space through which the user has access to the object.
    pub space: Option<TelemetrySpace>,
    /// If the object is owned by a team, this is the owning team's UID. For shared objects, the
    /// user might not be on the team.
    pub team_uid: Option<ServerId>,
}

/// Metadata to include in all notebook telemetry events.
///
/// There are 4 expected configurations:
/// * Personal cloud notebooks: `notebook_id` is `Some`, `team_uid` is `None`, and location is `PersonalCloud`
/// * Team cloud notebooks: `notebook_id` is `Some`, `team_uid` is `Some`, and location is `Team`
/// * Local file-based notebooks: `notebook_id` and `team_uid` are `None`, and location is `LocalFile`
/// * Remote file-based notebooks: `notebook_id` and `team_uid` are `None`, and location is `RemoteFile`
///
/// This representation allows for invalid combinations, but makes querying the data easier (for
/// example, to find all notebook events for a given team).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct NotebookTelemetryMetadata {
    /// The notebook ID, only available for cloud notebooks that have been synced to the server.
    pub notebook_id: Option<NotebookId>,
    /// The team UID, only available for cloud notebooks in a shared team.
    pub team_uid: Option<ServerId>,
    pub space: Option<TelemetrySpace>,
    /// Where the notebook is canonically located.
    pub location: NotebookLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_table_count: Option<usize>,
}

impl NotebookTelemetryMetadata {
    pub fn new(
        notebook_id: impl Into<Option<NotebookId>>,
        team_uid: impl Into<Option<ServerId>>,
        location: impl Into<NotebookLocation>,
        space: Option<TelemetrySpace>,
    ) -> Self {
        Self {
            notebook_id: notebook_id.into(),
            team_uid: team_uid.into(),
            location: location.into(),
            space,
            markdown_table_count: None,
        }
    }

    pub fn with_markdown_table_count(mut self, markdown_table_count: usize) -> Self {
        self.markdown_table_count = Some(markdown_table_count);
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotebookActionEvent {
    #[serde(flatten)]
    pub action: NotebookTelemetryAction,
    #[serde(flatten)]
    pub metadata: NotebookTelemetryMetadata,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct EnvVarTelemetryMetadata {
    /// The object ID, only available for cloud env vars that have been synced to the server.
    pub object_id: Option<GenericStringObjectId>,
    /// The team UID, only available for cloud env vars in a shared team.
    pub team_uid: Option<ServerId>,
    pub space: TelemetrySpace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenedSharingDialogEvent {
    pub source: SharingDialogSource,

    /// Metadata for the object being shared, if it's a Warp Drive object.
    #[serde(flatten)]
    pub object_metadata: Option<CloudObjectTelemetryMetadata>,

    /// Metadata for the session being shared, if there is one.
    pub session_id: Option<SharedSessionId>,
}

/// How the user opened the Warp Drive sharing dialog.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SharingDialogSource {
    /// The sharing button in the pane header.
    PaneHeader,
    /// The per-pane command palette entry (includes keybindings).
    CommandPalette,
    /// The Warp Drive index context menu.
    DriveIndex,
    /// The sharing dialog was auto-opened from shared session creation.
    StartedSessionShare,
    /// The user intented into Warp with an email address to invite.
    /// The user jumped from an inherited ACL to its definition on a parent object.
    InheritedPermission,
    /// The onboarding block shown after users create new personal objects.
    OnboardingBlock,
    /// The conversation list overflow menu.
    ConversationList,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum TabRenameEvent {
    OpenedEditor,
    CustomNameSet,
    CustomNameCleared,
}

/// The possible sources notifications can turned on from.
#[derive(Clone, Serialize, Deserialize)]
pub enum NotificationsTurnedOnSource {
    Settings,
    Banner,
}

/// The possible types of toggles in the find bar
#[derive(Clone, Serialize, Deserialize)]
pub enum FindOption {
    CaseSensitive,
    FindInBlock,
    Regex,
}

/// The possible ways to trigger command x-ray
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandXRayTrigger {
    Hover,
    Keystroke,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
pub enum PaletteSource {
    PrefixChange,
    Keybinding,
    CtrlTab { shift_pressed_initially: bool },
    QuitModal,
    LogOutModal,
    IntegrationTest,
    ContextChip,
    PaneHeader,
    TitleBarSearchBar,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum FileTreeSource {
    /// Opened from the pane header toolbelt button.
    PaneHeader,
    Keybinding,
    LeftPanelToolbelt,
    ForceOpened,
}

#[cfg(feature = "local_fs")]
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodePanelsFileOpenEntrypoint {
    CodeReview,
    ProjectExplorer,
    GlobalSearch,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum WarpDriveSource {
    Legacy,
    LeftPanelToolbelt,
    ForceOpened,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum CloseTarget {
    App,
    Window,
    Tab,
    Pane,
    EditorTab,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum PtySpawnMode {
    /// The pty was spawned using the terminal server.
    TerminalServer,
    /// We tried to spawn the pty using the terminal server, but something went
    /// wrong so we fell back to spawning it directly.
    FallbackToDirect,
    /// The terminal server is not in use, and we spawned the pty directly
    /// (in tests, for example).
    Direct,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum LaunchConfigUiLocation {
    CommandPalette,
    AppMenu,
    TabMenu,
    Uri,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum SecretInteraction {
    RevealSecret,
    HideSecret,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum AnonymousUserSignupEntrypoint {
    HitDriveObjectLimit,
    LoginGatedFeature,
    SignUpButton,
    RenotificationBlock,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum UndoCloseItemType {
    Window,
    Tab,
    Pane,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PromptChoice {
    PS1,
    Default,
    Custom { builtin_chips: Vec<String> },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ToggleBlockFilterSource {
    /// This includes the keybinding and the command palette items.
    Binding,
    ContextMenu,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TierLimitHitEvent {
    pub team_uid: ServerId,
    pub feature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CpuUsageStats {
    /// The number of logical CPUs on the system.
    pub num_cpus: usize,

    /// The maximum CPU usage over the measurement interval.
    ///
    /// This number is in the range [0, num_cpus].  The CPU utilization, as a
    /// percentage, can be determined via `max_usage / num_cpus * 100`.
    pub max_usage: f32,

    /// The average CPU usage over the measurement interval.
    ///
    /// This number is in the range [0, num_cpus].  The CPU utilization, as a
    /// percentage, can be determined via `avg_usage / num_cpus * 100`.
    pub avg_usage: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryUsageStats {
    pub total_application_usage_bytes: usize,
    pub total_blocks: usize,
    pub total_lines: usize,

    /// Statistics about blocks that have been seen in the past 5 minutes.
    pub active_block_stats: BlockMemoryUsageStats,
    /// Statistics about blocks that haven't been seen since [5m, 1h).
    pub inactive_5m_stats: BlockMemoryUsageStats,
    /// Statistics about blocks that haven't been seen since [1h, 24h).
    pub inactive_1h_stats: BlockMemoryUsageStats,
    /// Statistics about blocks that haven't been seen since [24h, ..).
    pub inactive_24h_stats: BlockMemoryUsageStats,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockMemoryUsageStats {
    pub num_blocks: usize,
    pub num_lines: usize,
    pub estimated_memory_usage_bytes: usize,
}

/// How the user triggered the [`AddTabWithShell`] event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AddTabWithShellSource {
    CommandPalette,
    ShellSelectorMenu,
}

#[derive(Clone, Copy, Debug, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputUXChangeOrigin {
    #[default]
    Settings,
    ADELaunchModal,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginEventSource {
    OnboardingSlide,
    AuthModal,
}

#[derive(Clone, EnumDiscriminants)]
#[strum_discriminants(derive(EnumIter))]
pub enum TelemetryEvent {
    BlockCompleted {
        block_finished_to_precmd_delay_ms: u64,
        honor_ps1_enabled: bool,
        num_secrets_redacted: usize,
        /// The number of lines in the block's output grid when it was
        /// finished.
        num_output_lines: u64,
        /// The number of lines of output that were truncated while the block
        /// was active and receiving output.
        num_output_lines_truncated: u64,
        terminal_session_id: Option<SessionId>,
        is_udi_enabled: bool,
    },
    /// This is identical to the `BlockCompleted` event, but includes extra fields for
    /// the command run / time it took the block to complete / exit code.
    /// That sort of telemetry should *NEVER* be sent in production, so
    /// DO NOT SEND THIS IN NON-DOGFOOD ENVIRONMENTS!
    BlockCompletedOnDogfoodOnly {
        block_finished_to_precmd_delay_ms: u64,
        honor_ps1_enabled: bool,
        num_secrets_redacted: usize,
        /// The number of lines in the block's output grid when it was
        /// finished.
        num_output_lines: u64,
        /// The number of lines of output that were truncated while the block
        /// was active and receiving output.
        num_output_lines_truncated: u64,
        command: String,
        duration: Duration,
        exit_code: ExitCode,
        terminal_session_id: Option<SessionId>,
    },
    /// A new block of background output was started and added to the block list.
    BackgroundBlockStarted,
    SessionCreation,
    Login,
    ConfirmSuggestion {
        mode: TelemetryInputSuggestionsMode,
        match_type: MatchType,
    },
    /// Copy command, output or both for some number of blocks.
    ContextMenuCopy(BlockEntity, BlockSelectionCardinality),
    ContextMenuFindWithinBlocks(BlockSelectionCardinality),
    ContextMenuCopyPrompt {
        part: PromptPart,
    },
    ContextMenuToggleGitPromptDirtyIndicator {
        enabled: bool,
    },
    ContextMenuInsertSelectedText,
    /// The user opened the prompt editor modal.
    OpenPromptEditor {
        entrypoint: PromptEditorOpenSource,
    },
    /// The user's prompt was edited via the prompt editor modal.
    PromptEdited {
        prompt: PromptChoice,
        entrypoint: String,
    },
    ReinputCommands(BlockSelectionCardinality),
    JumpToPreviousCommand,
    BlockSelection(BlockSelectionDetails),
    BootstrappingSlow(BootstrappingInfo),
    BootstrappingSlowContents(SlowBootstrapInfo),
    /// Logged when a pending session is abandoned before it hits Bootstrapped.
    SessionAbandonedBeforeBootstrap {
        pending_shell: Option<ShellType>,
        has_pending_ssh_session: bool,
        was_ever_visible: bool,
        duration_since_start: Duration,
    },
    BootstrappingSucceeded(BootstrappingInfo),
    OpenThemeChooser,
    ThemeSelection {
        theme: String,
        entrypoint: String,
    },
    AppIconSelection {
        icon: String,
    },
    CursorDisplayType {
        cursor: String,
    },
    OpenThemeCreatorModal,
    CreateCustomTheme,
    DeleteCustomTheme,
    SplitPane,
    ToggleRestoreSession(bool),
    DatabaseStartUpError(String),
    DatabaseReadError(String),
    DatabaseWriteError(String),
    AppStartup(AppStartupInfo),
    /// The native app was opened while logged out. Since Warp requires login,
    /// this usually means a new user.
    LoggedOutStartup,
    /// The download source, if it can be determined. Will only be sent when
    /// the app is launched while logged out.
    DownloadSource(DownloadSource),
    /// We attempted to bootstrap an SSH session via the SSH wrapper.  The
    /// argument is the name of the remote shell.
    SSHBootstrapAttempt(String),
    SSHControlMasterError {
        has_remote_server: bool,
    },
    KeybindingChanged {
        action: String,
        keystroke: Keystroke,
    },
    KeybindingResetToDefault {
        action: String,
    },
    KeybindingRemoved {
        action: String,
    },
    FeaturesPageAction {
        action: String,
        value: String,
    },
    OpenQuakeModeWindow,
    OpenWelcomeTips,
    DismissWelcomeTips,
    ShowNotificationsDiscoveryBanner,
    NotificationsDiscoveryBannerAction(NotificationsDiscoveryBannerAction),
    ShowNotificationsErrorBanner,
    NotificationsErrorBannerAction(NotificationsErrorBannerAction),
    NotificationPermissionsRequested {
        source: NotificationsTurnedOnSource,
        trigger: Option<NotificationsTrigger>,
    },
    NotificationsRequestPermissionsOutcome {
        outcome: RequestPermissionsOutcome,
    },
    // NotificationSent events are emitted at the app level. Thus, they encompass
    // notifications that are successfully sent _and_ those that fail at the platform level.
    NotificationSent {
        trigger: NotificationsTrigger,
    },
    NotificationFailedToSend {
        error: NotificationSendError,
    },
    NotificationClicked,
    ToggleFindOption {
        option: FindOption,
        enabled: bool,
    },
    SignUpButtonClicked,
    LoginButtonClicked {
        source: LoginEventSource,
    },
    LoginLaterButtonClicked {
        source: LoginEventSource,
    },
    LoginLaterConfirmationButtonClicked {
        source: LoginEventSource,
    },
    OpenNewSessionFromFilePath,
    AuthCommonQuestionClicked {
        question: &'static str,
    },
    AuthToggleFAQ {
        open: bool,
    },
    OpenAuthPrivacySettings {
        source: LoginEventSource,
    },
    TabRenamed(TabRenameEvent),
    MoveActiveTab {
        direction: TabMovement,
    },
    MoveTab {
        direction: TabMovement,
    },
    DragAndDropTab,
    DragAndDropTabGroup,
    TabOperations {
        action: TabTelemetryAction,
    },
    TriedToExecuteBeforePrecmd,
    ThinStrokesSettingChanged {
        new_value: ThinStrokes,
    },
    BookmarkBlockToggled {
        enable_bookmark: bool,
    },
    JumpToBookmark,
    JumpToBottomofBlockButtonClicked,
    ToggleJumpToBottomofBlockButton {
        enabled: bool,
    },
    ToggleShowBlockDividers {
        enabled: bool,
    },
    ShowInFileExplorer,
    OpenLaunchConfigSaveModal,
    SaveLaunchConfig {
        state: SaveState,
    },
    OpenLaunchConfigFile,
    OpenLaunchConfig {
        ui_location: LaunchConfigUiLocation,
        open_in_active_window: bool,
    },
    TeamCreated,
    TeamJoined,
    TeamLeft,
    ToggleSettingsSync {
        is_settings_sync_enabled: bool,
    },
    RemovedUserFromTeam,
    DeletedNotebook,
    ToggleApprovalsModal,
    SetLineHeight {
        new_value: f32,
    },
    ResourceCenterOpened,
    ResourceCenterTipsCompleted,
    ResourceCenterTipsSkipped,
    KeybindingsPageOpened,
    GlobalSearchOpened,
    GlobalSearchQueryStarted,
    GlobalSearchQueryCompleted {
        duration_ms: u64,
        /// Number of distinct remote hosts searched via the remote server
        /// daemon (0 for purely local searches).
        remote_host_count: usize,
        total_match_count: usize,
        /// Whether the result set was capped (locally or by a remote
        /// server-side cap).
        capped: bool,
        /// Whether the local search source failed while another source
        /// completed.
        local_source_failed: bool,
        /// Number of remote host search sources that failed while another
        /// source completed.
        remote_source_failures: usize,
    },
    OpenNotebook(NotebookTelemetryMetadata),
    EditNotebook {
        metadata: NotebookTelemetryMetadata,
        meaningful_change: bool,
    },
    NotebookAction(NotebookActionEvent),
    OpenedAltScreenFind,
    UserInitiatedClose {
        initiated_on: CloseTarget,
    },
    QuitModalShown {
        running_processes: u32,
        shared_sessions: u32,
        modal_for: CloseTarget,
    },
    QuitModalCancel {
        nav_palette: bool,
        modal_for: CloseTarget,
    },
    QuitModalDisabled,
    UserInitiatedLogOut,
    LogOutModalShown,
    LogOutModalCancel {
        nav_palette: bool,
    },
    SetOpacity {
        // Represented in percentages from 1-100.
        opacity: u8,
    },
    SetBlurRadius {
        // The radius value from 1-18.
        blur_radius: u8,
    },
    ToggleDimInactivePanes {
        enabled: bool,
    },
    InputModeChanged {
        old_mode: InputMode,
        new_mode: InputMode,
    },
    PtySpawned {
        mode: PtySpawnMode,
    },
    InitialWorkingDirectoryConfigurationChanged {
        advanced_mode_enabled: bool,
    },
    ToggleFocusPaneOnHover {
        enabled: bool,
    },
    OpenInputContextMenu,
    InputCutSelectedText,
    InputCopySelectedText,
    InputSelectAll,
    InputPaste,
    InputCommandSearch,
    ExperimentTriggered {
        experiment: &'static str,
        layer: &'static str,
        group_assignment: &'static str,
    },
    ToggleSyncAllPanesInAllTabs {
        enabled: bool,
    },
    ToggleSyncAllPanesInTab {
        enabled: bool,
    },
    ToggleSameLinePrompt {
        enabled: bool,
    },
    ToggleNewWindowsAtCustomSize {
        enabled: bool,
    },
    SetNewWindowsAtCustomSize,
    DisableInputSync,
    ToggleTabIndicators {
        enabled: bool,
    },
    TogglePreserveActiveTabColor {
        enabled: bool,
    },
    ShowSubshellBanner,
    DeclineSubshellBootstrap {
        remember: bool,
    },
    TriggerSubshellBootstrap {
        triggered_by_rc_file_snippet: bool,
    },
    AddDenylistedSubshellCommand,
    RemoveDenylistedSubshellCommand,
    AddAddedSubshellCommand,
    RemoveAddedSubshellCommand,
    ReceivedSubshellRcFileDcs,
    ToggleSshWarpification {
        enabled: bool,
    },
    /// User changed the SSH extension install mode.
    SetSshExtensionInstallMode {
        mode: &'static str,
    },
    /// User toggled the "Don't ask me this again" checkbox on the SSH
    /// remote-server choice block.
    SshRemoteServerChoiceDoNotAskAgainToggled {
        checked: bool,
    },
    WarpifyFooterShown {
        is_ssh: bool,
    },
    WarpifyFooterAcceptedWarpify {
        is_ssh: bool,
    },
    ShowAliasExpansionBanner,
    EnableAliasExpansionFromBanner,
    DismissAliasExpansionBanner,
    ShowVimKeybindingsBanner,
    EnableVimKeybindingsFromBanner,
    DismissVimKeybindingsBanner,
    InitiateReauth,
    InitiateAnonymousUserSignup {
        entrypoint: AnonymousUserSignupEntrypoint,
    },
    AnonymousUserExpirationLockout,
    AnonymousUserLinkedFromBrowser,
    AnonymousUserAttemptLoginGatedFeature {
        feature: LoginGatedFeature,
    },
    AnonymousUserHitCloudObjectLimit,
    NeedsReauth,
    WarpDriveOpened {
        source: WarpDriveSource,
        is_code_mode_v2: bool,
    },
    ToggleSecretRedaction {
        enabled: bool,
    },
    CustomSecretRegexAdded,
    ToggleObfuscateSecret {
        interaction: SecretInteraction,
    },
    CopySecret,
    UpdateSortingChoice {
        sorting_choice: DriveSortOrder,
    },
    UndoClose {
        item_type: UndoCloseItemType,
    },
    /// This event is used to measure PTY throughput.
    /// NOTE: this event is only meant to be used for WarpDev.
    PtyThroughput {
        /// The maximum PTY throughput in bytes/sec, aggregated over a 10 minute period.
        max_bytes_per_second: usize,
    },
    DuplicateObject(TelemetryCloudObjectType),
    DriveSharingOnboardingBlockShown,
    CommandFileRun,
    PageUpDownInEditorPressed {
        // Key pressed when nothing is in the editor (no-op)
        is_empty_editor: bool,
        // Is PageDown. Otherwise is PageUp
        is_down: bool,
    },
    /// Emitted on start share attempt, not on success.
    StartedSharingCurrentSession {
        includes_scrollback: bool,
        source: SharedSessionActionSource,
    },
    StoppedSharingCurrentSession {
        source: SharedSessionActionSource,
        reason: SessionEndedReason,
    },
    JoinedSharedSession {
        session_id: SharedSessionId,
        source_type: SessionSourceType,
    },
    /// Emitted when a shared session sharer cancels granting a role
    /// (currently only applies when granting executor mode).
    SharerCancelledGrantRole {
        role: Role,
    },
    /// Emitted when a shared session sharer checks "dont show again"
    /// in confirmation modal when granting a role.
    SharerGrantModalDontShowAgain,
    JumpToSharedSessionParticipant {
        jumped_to: ParticipantId,
    },
    CopiedSharedSessionLink {
        source: SharedSessionActionSource,
    },
    WebSessionOpenedOnDesktop {
        source: SharedSessionActionSource,
    },
    WebCloudObjectOpenedOnDesktop {
        object_metadata: CloudObjectTelemetryMetadata,
    },
    UnsupportedShell {
        shell: String,
    },
    LogOut,
    SettingsImportInitiated,
    InviteTeammates {
        num_teammates: usize,
        team_uid: ServerId,
    },
    CopyObjectToClipboard(TelemetryCloudObjectType),
    OpenAndWarpifyDockerSubshell {
        /// Some variant if we support this shell type, and None otherwise.
        shell_type: Option<ShellType>,
    },
    /// Represents an update to a block filter query that goes from empty to non-empty.
    UpdateBlockFilterQuery,
    UpdateBlockFilterQueryContextLines {
        num_context_lines: u16,
    },
    ToggleBlockFilterQuery {
        enabled: bool,
        source: ToggleBlockFilterSource,
    },
    ToggleBlockFilterCaseSensitivity {
        enabled: bool,
    },
    ToggleBlockFilterRegex {
        enabled: bool,
    },
    ToggleBlockFilterInvert {
        enabled: bool,
    },
    BlockFilterToolbeltButtonClicked,
    ToggleSnackbarInActivePane {
        show_snackbar: bool,
    },
    PaneDragInitiated,
    PaneDropped {
        drop_location: PaneDragDropLocation,
    },
    ObjectLinkCopied {
        link: String,
    },
    FileTreeToggled {
        source: FileTreeSource,
        is_code_mode_v2: bool,
    },
    /// User created a new file from the file tree
    FileTreeItemCreated,

    TierLimitHit(TierLimitHitEvent),
    ResourceUsageStats {
        cpu: CpuUsageStats,
        mem: MemoryUsageStats,
    },
    MemoryUsageStats {
        total_application_usage_bytes: usize,
        total_blocks: usize,
        total_lines: usize,

        /// Statistics about blocks that have been seen in the past 5 minutes.
        active_block_stats: BlockMemoryUsageStats,
        /// Statistics about blocks that haven't been seen since [5m, 1h).
        inactive_5m_stats: BlockMemoryUsageStats,
        /// Statistics about blocks that haven't been seen since [1h, 24h).
        inactive_1h_stats: BlockMemoryUsageStats,
        /// Statistics about blocks that haven't been seen since [24h, ..).
        inactive_24h_stats: BlockMemoryUsageStats,
    },
    MemoryUsageHigh {
        total_application_usage_bytes: u64,
        /// Platform-specific memory breakdown (JSON object with keys that
        /// vary by OS).  See `memory_footprint::memory_breakdown()`.
        memory_breakdown: serde_json::Value,
    },
    /// Emitted when the OS memory footprint crossed `MEMORY_USAGE_WARNING_THRESHOLD_BYTES` but had
    /// already dropped back under it by the time we re-checked on the next poll tick, so the spike
    /// looks transient rather than sustained.
    TransientMemorySpike {
        /// The OS footprint, not RSS.
        triggering_footprint_bytes: u64,
        /// The OS footprint, not RSS, at confirmation time.
        confirmation_footprint_bytes: u64,
    },
    EnvVarCollectionInvoked(EnvVarTelemetryMetadata),
    EnvVarWorkflowParameterization(EnvVarTelemetryMetadata),

    /// The user imported settings from another terminal.
    CompletedSettingsImport {
        terminal_type: TerminalType,
        imported_settings: Vec<ParsedTerminalSetting>,
    },
    /// The user focused a terminal option to import settings from.
    SettingsImportConfigFocused(TerminalType),
    /// The user clicked the "Reset to defaults" button in the settings import onboarding block.
    SettingsImportResetButtonClicked,
    /// When parsing iTerm for settings it contained multiple hotkey bindings.
    ITermMultipleHotkeys,
    UserMenuUpgradeClicked,
    ToggleWorkspaceDecorationVisibility {
        previous_value: WorkspaceDecorationVisibility,
        new_value: WorkspaceDecorationVisibility,
    },
    UpdateAltScreenPaddingMode {
        new_mode: AltScreenPaddingMode,
    },
    AddTabWithShell {
        source: AddTabWithShellSource,
        shell: String,
    },
    OpenedSharingDialog(OpenedSharingDialogEvent),
    ToggleLigatureRendering {
        enabled: bool,
    },
    #[cfg(feature = "local_fs")]
    CodePaneOpened {
        source: CodeSource,
        layout: EditorLayout,
        preview: bool,
    },
    #[cfg(feature = "local_fs")]
    CodePanelsFileOpened {
        entrypoint: CodePanelsFileOpenEntrypoint,
        target: FileTarget,
    },
    #[cfg(feature = "local_fs")]
    PreviewPanePromoted,
    /// An error was encountered fetching available WSL distributions from the Registry.
    /// This typically means the user hasn't installed or enabled WSL.
    #[cfg(windows)]
    WSLRegistryError,
    ShellTerminatedPrematurely {
        shell_type: Option<ShellType>,
        shell_path: Option<String>,
        reason: String,
        reason_details: Option<String>,
        antivirus_name: Option<String>,
        long_os_version: Option<String>,
        exit_reason: Option<String>,
    },
    /// User changed the input UX mode (e.g. Universal Developer Input, UDI, mode or Classic)
    InputUXModeChanged {
        is_udi_enabled: bool,
        origin: InputUXChangeOrigin,
    },
    TabCloseButtonPositionUpdated {
        position: TabCloseButtonPosition,
    },
    /// User selected an item from the "Recent" list on the new tab zero state
    RecentMenuItemSelected {
        // The kind of recent menu item selected
        kind: &'static str,
    },

    /// Emitted when the Codex modal is opened.
    /// Emitted when the user clicks "Use Codex" in the Codex modal.
    /// Emitted when a warp://linear deeplink is opened.
    /// Emitted when the remote server binary check completes.
    RemoteServerBinaryCheck {
        found: bool,
        error: Option<String>,
        remote_os: Option<String>,
        remote_arch: Option<String>,
    },
    /// Emitted when the remote server binary installation completes.
    /// `error` is `None` on success, `Some(reason)` on failure.
    RemoteServerInstallation {
        error: Option<String>,
        install_source: Option<remote_server::transport::InstallSource>,
        remote_os: Option<String>,
        remote_arch: Option<String>,
    },
    /// Emitted when the remote server connection + initialization completes.
    /// `error` is `None` on success, `Some(reason)` on failure.
    RemoteServerInitialization {
        phase: remote_server::manager::RemoteServerInitPhase,
        error: Option<String>,
        remote_os: Option<String>,
        remote_arch: Option<String>,
        /// Exit code of the SSH subprocess, if available.
        /// Helps distinguish proxy crashes from transport failures.
        exit_code: Option<i32>,
        /// Whether the SSH subprocess was killed by a signal.
        signal_killed: Option<bool>,
        /// Last lines from the proxy's stderr, if available.
        /// Provides server-side context for why the proxy exited.
        proxy_stderr: Option<String>,
    },
    /// Emitted when an established remote server connection drops.
    RemoteServerDisconnection {
        remote_os: Option<String>,
        remote_arch: Option<String>,
    },
    /// Emitted when a client request to the remote server fails.
    RemoteServerClientRequestError {
        operation: remote_server::manager::RemoteServerOperation,
        error_type: remote_server::manager::RemoteServerErrorKind,
        remote_os: Option<String>,
        remote_arch: Option<String>,
    },
    /// Emitted when a server message cannot be decoded (no parseable request_id).
    RemoteServerMessageDecodingError {
        remote_os: Option<String>,
        remote_arch: Option<String>,
    },
    /// Emitted when the full remote server setup flow completes successfully.
    RemoteServerSetupDuration {
        duration_ms: u64,
        installed_binary: bool,
        remote_os: Option<String>,
        remote_arch: Option<String>,
        /// Short description of the remote libc (e.g. "glibc 2.35",
        /// "musl", "unknown"). `None` when the preinstall check did
        /// not run (e.g. macOS hosts).
        remote_libc: Option<String>,
    },
    /// Emitted when the preinstall check classifies the remote host as
    /// unsupported by the prebuilt remote-server binary, so the controller
    /// silently falls back to the wrapper-only SSH/`RemoteCommandExecutor`
    /// flow without surfacing an install prompt.
    RemoteServerHostUnsupported {
        remote_os: Option<String>,
        remote_arch: Option<String>,
        /// Typed unsupported reason. Converted into stable telemetry
        /// fields in `payload()`.
        unsupported_reason: remote_server::setup::UnsupportedReason,
        /// Detected libc on the remote host, e.g. `"glibc 2.28"`,
        /// `"musl"`, `"unknown"`.
        detected_libc: String,
    },
    /// Emitted when a reconnection attempt succeeds after a spontaneous disconnect.
    RemoteServerReconnection {
        attempt: u32,
        remote_os: Option<String>,
        remote_arch: Option<String>,
    },
    /// Emitted when the remote server daemon process finishes startup and
    /// binds its Unix domain socket.  Reports the same `IntervalTimer`
    /// data that `AppStartup` reports for the GUI, but from the headless
    /// daemon process on the remote host.  Only emitted on success — if
    /// the daemon crashes before binding, no event is sent.
    RemoteServerDaemonStartup {
        timing_data: Vec<warp_core::interval_timer::TimingDataPoint>,
    },
    /// Emitted when all reconnection attempts are exhausted.
    RemoteServerReconnectExhausted {
        attempts: u32,
        remote_os: Option<String>,
        remote_arch: Option<String>,
        exit_code: Option<i32>,
        signal_killed: Option<bool>,
    },
}

impl TelemetryEventTrait for TelemetryEvent {
    fn name(&self) -> &'static str {
        self.name()
    }

    fn payload(&self) -> Option<Value> {
        self.payload()
    }

    fn description(&self) -> &'static str {
        let discriminant: TelemetryEventDiscriminants = self.into();
        discriminant.description()
    }

    fn contains_ugc(&self) -> bool {
        self.contains_ugc()
    }

    fn enablement_state(&self) -> EnablementState {
        self.enablement_state()
    }

    fn event_descs() -> impl Iterator<Item = Box<dyn TelemetryEventDesc>> {
        warp_core::telemetry::enum_events::<Self>()
    }
}

impl TelemetryEvent {
    pub fn name(&self) -> &'static str {
        let discriminant: TelemetryEventDiscriminants = self.into();
        discriminant.name()
    }

    pub fn enablement_state(&self) -> EnablementState {
        let discriminant: TelemetryEventDiscriminants = self.into();
        discriminant.enablement_state()
    }

    pub fn payload(&self) -> Option<Value> {
        match self {
            TelemetryEvent::BootstrappingSlow(info) => Some(json!(info)),
            TelemetryEvent::BootstrappingSlowContents(info) => Some(json!(info)),
            TelemetryEvent::ToggleSettingsSync {
                is_settings_sync_enabled,
            } => Some(json!({ "is_settings_sync_enabled": is_settings_sync_enabled })),
            TelemetryEvent::SessionAbandonedBeforeBootstrap {
                pending_shell,
                has_pending_ssh_session,
                was_ever_visible,
                duration_since_start,
            } => Some(json!({
                "pending_shell": pending_shell.map(|shell| shell.name()),
                "has_pending_ssh_session": has_pending_ssh_session,
                "was_ever_visible": was_ever_visible,
                "duration_since_start_secs": duration_since_start.as_secs_f32(),
            })),
            TelemetryEvent::BlockCompleted {
                block_finished_to_precmd_delay_ms,
                honor_ps1_enabled,
                num_secrets_redacted,
                num_output_lines,
                num_output_lines_truncated,
                terminal_session_id,
                is_udi_enabled,
            } => Some(json!({
                "block_finished_to_precmd_delay_ms": block_finished_to_precmd_delay_ms,
                "honor_ps1_enabled": honor_ps1_enabled,
                "num_secrets_redacted": num_secrets_redacted,
                "num_output_lines": num_output_lines,
                "num_output_lines_truncated": num_output_lines_truncated,
                "terminal_session_id": terminal_session_id,
                "is_udi_enabled": is_udi_enabled,
            })),
            TelemetryEvent::ToggleFocusPaneOnHover { enabled } => Some(json!({
                "enabled": enabled,
            })),
            TelemetryEvent::BlockCompletedOnDogfoodOnly {
                block_finished_to_precmd_delay_ms,
                honor_ps1_enabled,
                num_secrets_redacted,
                num_output_lines,
                num_output_lines_truncated,
                command,
                duration,
                exit_code,
                terminal_session_id,
            } => Some(json!({
                "block_finished_to_precmd_delay_ms": block_finished_to_precmd_delay_ms,
                "honor_ps1_enabled": honor_ps1_enabled,
                "num_secrets_redacted": num_secrets_redacted,
                "num_output_lines": num_output_lines,
                "num_output_lines_truncated": num_output_lines_truncated,
                "command": command,
                "duration": duration,
                "exit_code": exit_code,
                "terminal_session_id": terminal_session_id,
            })),
            TelemetryEvent::BootstrappingSucceeded(info) => Some(json!(info)),
            TelemetryEvent::SSHBootstrapAttempt(remote_shell) => {
                Some(json!({ "shell": remote_shell.as_str() }))
            }
            TelemetryEvent::ContextMenuCopy(entity, cardinality) => {
                Some(json!({ "entity": entity.as_str(), "cardinality": cardinality }))
            }
            TelemetryEvent::ContextMenuFindWithinBlocks(cardinality) => {
                Some(json!({ "cardinality": cardinality }))
            }
            TelemetryEvent::ContextMenuCopyPrompt { part } => Some(json!({ "part": part })),
            TelemetryEvent::ReinputCommands(cardinality) => {
                Some(json!({ "cardinality": cardinality }))
            }
            TelemetryEvent::ContextMenuToggleGitPromptDirtyIndicator { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::BlockSelection(details) => Some(json!(details)),
            TelemetryEvent::ConfirmSuggestion { mode, match_type } => {
                Some(json!({ "mode": mode, "match_type": match_type }))
            }
            TelemetryEvent::ThemeSelection { theme, entrypoint } => {
                Some(json!({ "theme": theme, "entrypoint": entrypoint }))
            }
            TelemetryEvent::AppIconSelection { icon } => Some(json!({"icon": icon})),
            TelemetryEvent::CursorDisplayType {
                cursor: cursor_display_type,
            } => Some(json!({"cursor": cursor_display_type})),
            TelemetryEvent::ObjectLinkCopied { link } => Some(json!({"link": link})),
            TelemetryEvent::FileTreeToggled {
                source,
                is_code_mode_v2,
            } => Some(json!({
                "source": source,
                "is_code_mode_v2": is_code_mode_v2,
            })),
            TelemetryEvent::ToggleRestoreSession(enabled) => Some(json!({ "enabled": enabled })),
            TelemetryEvent::DatabaseStartUpError(error) => Some(json!(error)),
            TelemetryEvent::DatabaseReadError(error) => Some(json!(error)),
            TelemetryEvent::DatabaseWriteError(error) => Some(json!(error)),
            TelemetryEvent::AppStartup(info) => Some(json!(info)),
            TelemetryEvent::DownloadSource(source) => Some(json!(source)),
            TelemetryEvent::KeybindingChanged { action, keystroke } => {
                Some(json!({ "action": action, "keystroke": keystroke.normalized() }))
            }
            TelemetryEvent::KeybindingResetToDefault { action } => {
                Some(json!({ "action": action }))
            }
            TelemetryEvent::KeybindingRemoved { action } => Some(json!({ "action": action })),
            TelemetryEvent::FeaturesPageAction { action, value } => {
                Some(json!({"action": action, "value": value}))
            }
            TelemetryEvent::NotificationsDiscoveryBannerAction(action) => {
                Some(json!({ "action": action }))
            }
            TelemetryEvent::InputModeChanged { old_mode, new_mode } => {
                Some(json!({ "old_mode": old_mode, "new_mode": new_mode }))
            }
            TelemetryEvent::NotificationsErrorBannerAction(action) => {
                Some(json!({ "action": action }))
            }
            TelemetryEvent::NotificationPermissionsRequested { source, trigger } => {
                Some(json!({ "source": source, "trigger": trigger }))
            }
            TelemetryEvent::NotificationFailedToSend { error } => Some(json!({ "error": error })),
            TelemetryEvent::NotificationSent { trigger } => Some(json!({
                "trigger": trigger,
            })),
            TelemetryEvent::NotificationsRequestPermissionsOutcome { outcome } => {
                Some(json!({ "outcome": outcome }))
            }
            TelemetryEvent::ToggleFindOption { option, enabled } => {
                Some(json!({ "option": option, "enabled": enabled }))
            }
            TelemetryEvent::AuthCommonQuestionClicked { question } => Some(json!(question)),
            TelemetryEvent::AuthToggleFAQ { open } => {
                let payload = if *open { "open" } else { "close" };
                Some(json!(payload))
            }
            TelemetryEvent::TabRenamed(rename_event) => Some(json!(rename_event)),
            TelemetryEvent::MoveActiveTab { direction } => Some(json!({ "direction": direction })),
            TelemetryEvent::MoveTab { direction } => Some(json!({ "direction": direction })),
            TelemetryEvent::TabOperations { action } => Some(json!({ "action": action })),
            TelemetryEvent::ThinStrokesSettingChanged { new_value } => {
                Some(json!({ "new_value": new_value }))
            }
            TelemetryEvent::BookmarkBlockToggled { enable_bookmark } => {
                Some(json!({ "enable_bookmark": enable_bookmark }))
            }
            TelemetryEvent::SaveLaunchConfig { state } => Some(json!({ "state": state })),
            TelemetryEvent::SetLineHeight { new_value } => Some(json!({ "new_value": new_value })),
            TelemetryEvent::OpenNotebook(metadata) => Some(json!(metadata)),
            TelemetryEvent::EditNotebook {
                metadata,
                meaningful_change,
            } => Some(json!({
                "notebook_id": metadata.notebook_id,
                "team_uid": metadata.team_uid,
                "meaningful_change": meaningful_change,
            })),
            TelemetryEvent::NotebookAction(event) => Some(json!(event)),
            TelemetryEvent::UserInitiatedClose { initiated_on } => {
                Some(json!({ "initiated_on": initiated_on }))
            }
            TelemetryEvent::QuitModalShown {
                running_processes,
                shared_sessions,
                modal_for,
            } => Some(
                json!({ "running_processes": running_processes, "shared_sessions": shared_sessions, "modal_for": modal_for }),
            ),
            TelemetryEvent::QuitModalCancel {
                nav_palette,
                modal_for,
            } => Some(json!({ "nav_palette": nav_palette, "modal_for": modal_for })),
            TelemetryEvent::LogOutModalCancel { nav_palette } => {
                Some(json!({ "nav_palette": nav_palette }))
            }
            TelemetryEvent::SetBlurRadius { blur_radius } => {
                Some(json!({ "blur_radius": blur_radius }))
            }
            TelemetryEvent::SetOpacity { opacity } => Some(json!({ "opacity": opacity })),
            TelemetryEvent::ToggleDimInactivePanes { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::ToggleJumpToBottomofBlockButton { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::PtySpawned { mode } => Some(json!({ "mode": mode })),
            TelemetryEvent::InitialWorkingDirectoryConfigurationChanged {
                advanced_mode_enabled,
            } => Some(json!({ "advanced_mode_enabled": advanced_mode_enabled })),
            #[cfg(feature = "local_fs")]
            TelemetryEvent::CodePaneOpened {
                source,
                layout,
                preview,
            } => Some(
                json!({ "source": source.telemetry_source_name(), "layout": layout, "preview": preview }),
            ),
            #[cfg(feature = "local_fs")]
            TelemetryEvent::CodePanelsFileOpened { entrypoint, target } => {
                let (target, layout, editor) = match target {
                    FileTarget::MarkdownViewer(layout) => {
                        ("warp_markdown_viewer", Some(*layout), None)
                    }
                    FileTarget::CodeEditor(layout) => ("warp_code_editor", Some(*layout), None),
                    FileTarget::EnvEditor => ("env_editor", None, None),
                    FileTarget::SystemDefault => ("system_default", None, None),
                    FileTarget::SystemGeneric => ("system_generic", None, None),
                    FileTarget::ExternalEditor(editor) => ("external_editor", None, Some(*editor)),
                };

                Some(json!({
                    "entrypoint": entrypoint,
                    "target": target,
                    "layout": layout,
                    "editor": editor,
                }))
            }
            #[cfg(feature = "local_fs")]
            TelemetryEvent::PreviewPanePromoted => None,
            TelemetryEvent::ExperimentTriggered {
                experiment,
                layer,
                group_assignment,
            } => Some(
                json!({ "experiment": experiment, "layer": layer, "group_assignment": group_assignment }),
            ),
            TelemetryEvent::ToggleSyncAllPanesInAllTabs { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::ToggleSyncAllPanesInTab { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::ToggleTabIndicators { enabled } => Some(json!({ "enabled": enabled })),
            TelemetryEvent::TogglePreserveActiveTabColor { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::DeclineSubshellBootstrap { remember } => {
                Some(json!({ "remember": remember }))
            }
            TelemetryEvent::WarpifyFooterShown { is_ssh }
            | TelemetryEvent::WarpifyFooterAcceptedWarpify { is_ssh } => {
                Some(json!({ "is_ssh": is_ssh }))
            }
            TelemetryEvent::ToggleSameLinePrompt { enabled } => Some(json!({ "enabled": enabled })),
            TelemetryEvent::TriggerSubshellBootstrap {
                triggered_by_rc_file_snippet,
            } => Some(json!({
                "triggered_by_rc_file_snippet": triggered_by_rc_file_snippet
            })),
            TelemetryEvent::OpenLaunchConfig {
                ui_location,
                open_in_active_window,
            } => Some(
                json!({ "ui_location": ui_location, "open_in_active_window": open_in_active_window }),
            ),
            TelemetryEvent::ToggleSecretRedaction { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::ToggleObfuscateSecret { interaction } => {
                Some(json!({ "interaction": interaction }))
            }
            TelemetryEvent::UpdateSortingChoice { sorting_choice } => {
                Some(json!({ "sorting_choice": sorting_choice }))
            }
            TelemetryEvent::UndoClose { item_type } => Some(json!({ "item_type": item_type })),
            TelemetryEvent::PromptEdited { prompt, entrypoint } => Some(json!({
                "prompt": prompt,
                "entrypoint": entrypoint
            })),
            TelemetryEvent::OpenPromptEditor { entrypoint } => {
                Some(json!({ "entrypoint": entrypoint }))
            }
            TelemetryEvent::PtyThroughput {
                max_bytes_per_second,
            } => Some(json!({
                "max_bytes_per_second": max_bytes_per_second,
            })),
            TelemetryEvent::DuplicateObject(object_type) => {
                Some(json!({ "object_type": object_type }))
            }
            TelemetryEvent::PageUpDownInEditorPressed {
                is_empty_editor,
                is_down,
            } => Some(json!({"is_empty_editor": is_empty_editor, "is_down": is_down})),
            TelemetryEvent::StartedSharingCurrentSession {
                includes_scrollback,
                source,
            } => Some(json!({ "includes_scrollback": includes_scrollback, "source": source })),
            TelemetryEvent::StoppedSharingCurrentSession { source, reason } => {
                Some(json!({ "source": source, "reason": reason }))
            }
            TelemetryEvent::UnsupportedShell { shell } => Some(json!({ "shell": shell })),
            TelemetryEvent::CopyObjectToClipboard(object_type) => {
                Some(json!({ "object_type": object_type }))
            }
            TelemetryEvent::OpenAndWarpifyDockerSubshell { shell_type } => {
                Some(json!({ "shell_type": shell_type }))
            }
            TelemetryEvent::ToggleBlockFilterQuery { enabled, source } => {
                Some(json!({"enabled": enabled, "source": source}))
            }
            TelemetryEvent::ToggleBlockFilterRegex { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::ToggleShowBlockDividers { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::ToggleBlockFilterCaseSensitivity { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::ToggleBlockFilterInvert { enabled } => {
                Some(json!({ "enabled": enabled }))
            }
            TelemetryEvent::UpdateBlockFilterQueryContextLines { num_context_lines } => {
                Some(json!({ "num_context_lines": num_context_lines }))
            }
            TelemetryEvent::ToggleNewWindowsAtCustomSize { enabled } => {
                Some(json!({"enabled": enabled}))
            }
            TelemetryEvent::ToggleSshWarpification { enabled } => Some(json!({"enabled": enabled})),
            TelemetryEvent::SetSshExtensionInstallMode { mode } => Some(json!({"mode": mode})),
            TelemetryEvent::SshRemoteServerChoiceDoNotAskAgainToggled { checked } => {
                Some(json!({"checked": checked}))
            }
            TelemetryEvent::JoinedSharedSession {
                session_id,
                source_type,
            } => Some(json!({
                "session_id": session_id,
                "source_type": source_type,
            })),
            TelemetryEvent::SharerCancelledGrantRole { role } => Some(json!({ "role": role })),
            TelemetryEvent::JumpToSharedSessionParticipant { jumped_to } => {
                Some(json!({ "jumped_to": jumped_to }))
            }
            TelemetryEvent::CopiedSharedSessionLink { source } => Some(json!({ "source": source })),
            TelemetryEvent::WebSessionOpenedOnDesktop { source } => {
                Some(json!({ "source": source}))
            }
            TelemetryEvent::WebCloudObjectOpenedOnDesktop { object_metadata } => Some(json!({
                "object": object_metadata,
            })),
            TelemetryEvent::ToggleSnackbarInActivePane { show_snackbar } => {
                Some(json!({ "show_snackbar": show_snackbar }))
            }
            TelemetryEvent::PaneDropped { drop_location } => {
                Some(json!({ "location": drop_location }))
            }
            TelemetryEvent::InviteTeammates {
                num_teammates,
                team_uid,
            } => Some(json!({"num_teammates": num_teammates, "team_uid": team_uid})),
            TelemetryEvent::TierLimitHit(event) => Some(json!(event)),
            TelemetryEvent::ResourceUsageStats { cpu, mem } => Some(json!({
                "cpu": cpu,
                "mem": {
                    // Only report the total application usage; skip sending
                    // the additional, more detailed usage information.
                    "total_application_usage_bytes": mem.total_application_usage_bytes,
                },
            })),
            TelemetryEvent::MemoryUsageStats {
                total_application_usage_bytes,
                total_blocks,
                total_lines,
                active_block_stats,
                inactive_5m_stats,
                inactive_1h_stats,
                inactive_24h_stats,
            } => Some(json!({
                "total_application_usage_bytes": total_application_usage_bytes,
                "total_blocks": total_blocks,
                "total_lines": total_lines,
                "active_block_stats": active_block_stats,
                "inactive_5m_stats": inactive_5m_stats,
                "inactive_1h_stats": inactive_1h_stats,
                "inactive_24h_stats": inactive_24h_stats
            })),
            TelemetryEvent::MemoryUsageHigh {
                total_application_usage_bytes,
                memory_breakdown,
            } => Some(json!({
                "total_application_usage_bytes": total_application_usage_bytes,
                "memory_breakdown": memory_breakdown,
            })),
            TelemetryEvent::TransientMemorySpike {
                triggering_footprint_bytes,
                confirmation_footprint_bytes,
            } => Some(json!({
                "triggering_footprint_bytes": triggering_footprint_bytes,
                "confirmation_footprint_bytes": confirmation_footprint_bytes,
            })),
            TelemetryEvent::EnvVarCollectionInvoked(metadata) => Some(json!(metadata)),
            TelemetryEvent::EnvVarWorkflowParameterization(metadata) => Some(json!(metadata)),
            TelemetryEvent::CompletedSettingsImport {
                terminal_type,
                imported_settings,
            } => Some(
                json!({ "terminal_type": terminal_type, "imported_settings": imported_settings}),
            ),
            TelemetryEvent::SettingsImportConfigFocused(terminal_type_and_profile) => {
                Some(json!({"terminal_and_type_profile": terminal_type_and_profile}))
            }
            TelemetryEvent::InitiateAnonymousUserSignup { entrypoint } => {
                Some(json!({"entrypoint": entrypoint}))
            }
            TelemetryEvent::AnonymousUserAttemptLoginGatedFeature { feature } => {
                Some(json!({"feature": feature}))
            }
            TelemetryEvent::ToggleWorkspaceDecorationVisibility {
                previous_value,
                new_value,
            } => Some(json!({
                "previous_value": previous_value,
                "new_value": new_value,
            })),
            TelemetryEvent::UpdateAltScreenPaddingMode { new_mode } => Some(json!({
                "new_mode": new_mode,
            })),
            TelemetryEvent::AddTabWithShell { source, shell } => {
                Some(json!({ "source": source, "shell": shell }))
            }
            TelemetryEvent::OpenedSharingDialog(event) => Some(json!(event)),
            TelemetryEvent::ToggleLigatureRendering { enabled } => {
                Some(json!({"enabled": enabled}))
            }
            TelemetryEvent::ShellTerminatedPrematurely {
                shell_type,
                shell_path,
                reason,
                reason_details,
                antivirus_name,
                long_os_version,
                exit_reason,
            } => Some(json!({
                "shell_type": shell_type,
                "shell_path": shell_path,
                "reason": reason,
                "reason_details": reason_details,
                "antivirus_name": antivirus_name,
                "long_os_version": long_os_version,
                "exit_reason": exit_reason,
            })),
            TelemetryEvent::InputUXModeChanged {
                is_udi_enabled,
                origin,
            } => Some(json!({
                "is_udi_enabled": is_udi_enabled,
                "origin": origin,
            })),
            TelemetryEvent::TabCloseButtonPositionUpdated { position } => Some(json!({
                "position": position,
            })),
            TelemetryEvent::BackgroundBlockStarted
            | TelemetryEvent::SessionCreation
            | TelemetryEvent::Login
            | TelemetryEvent::ContextMenuInsertSelectedText
            | TelemetryEvent::JumpToPreviousCommand
            | TelemetryEvent::OpenThemeChooser
            | TelemetryEvent::OpenThemeCreatorModal
            | TelemetryEvent::CreateCustomTheme
            | TelemetryEvent::DeleteCustomTheme
            | TelemetryEvent::SplitPane
            | TelemetryEvent::LoggedOutStartup
            | TelemetryEvent::OpenQuakeModeWindow
            | TelemetryEvent::OpenWelcomeTips
            | TelemetryEvent::DismissWelcomeTips
            | TelemetryEvent::ShowNotificationsDiscoveryBanner
            | TelemetryEvent::ShowNotificationsErrorBanner
            | TelemetryEvent::NotificationClicked
            | TelemetryEvent::SignUpButtonClicked
            | TelemetryEvent::OpenNewSessionFromFilePath
            | TelemetryEvent::DragAndDropTab
            | TelemetryEvent::DragAndDropTabGroup
            | TelemetryEvent::TriedToExecuteBeforePrecmd
            | TelemetryEvent::JumpToBookmark
            | TelemetryEvent::JumpToBottomofBlockButtonClicked
            | TelemetryEvent::ShowInFileExplorer
            | TelemetryEvent::OpenLaunchConfigSaveModal
            | TelemetryEvent::OpenLaunchConfigFile
            | TelemetryEvent::TeamCreated
            | TelemetryEvent::TeamJoined
            | TelemetryEvent::TeamLeft
            | TelemetryEvent::RemovedUserFromTeam
            | TelemetryEvent::DeletedNotebook
            | TelemetryEvent::ToggleApprovalsModal
            | TelemetryEvent::ResourceCenterOpened
            | TelemetryEvent::ResourceCenterTipsCompleted
            | TelemetryEvent::ResourceCenterTipsSkipped
            | TelemetryEvent::KeybindingsPageOpened
            | TelemetryEvent::OpenedAltScreenFind
            | TelemetryEvent::QuitModalDisabled
            | TelemetryEvent::UserInitiatedLogOut
            | TelemetryEvent::LogOutModalShown
            | TelemetryEvent::OpenInputContextMenu
            | TelemetryEvent::InputCutSelectedText
            | TelemetryEvent::InputCopySelectedText
            | TelemetryEvent::InputSelectAll
            | TelemetryEvent::InputPaste
            | TelemetryEvent::InputCommandSearch
            | TelemetryEvent::SetNewWindowsAtCustomSize
            | TelemetryEvent::DisableInputSync
            | TelemetryEvent::ShowSubshellBanner
            | TelemetryEvent::AddDenylistedSubshellCommand
            | TelemetryEvent::RemoveDenylistedSubshellCommand
            | TelemetryEvent::AddAddedSubshellCommand
            | TelemetryEvent::RemoveAddedSubshellCommand
            | TelemetryEvent::ReceivedSubshellRcFileDcs
            | TelemetryEvent::ShowAliasExpansionBanner
            | TelemetryEvent::EnableAliasExpansionFromBanner
            | TelemetryEvent::DismissAliasExpansionBanner
            | TelemetryEvent::ShowVimKeybindingsBanner
            | TelemetryEvent::EnableVimKeybindingsFromBanner
            | TelemetryEvent::DismissVimKeybindingsBanner
            | TelemetryEvent::InitiateReauth
            | TelemetryEvent::NeedsReauth
            | TelemetryEvent::AnonymousUserExpirationLockout
            | TelemetryEvent::AnonymousUserLinkedFromBrowser
            | TelemetryEvent::AnonymousUserHitCloudObjectLimit
            | TelemetryEvent::CustomSecretRegexAdded
            | TelemetryEvent::CopySecret
            | TelemetryEvent::CommandFileRun
            | TelemetryEvent::SharerGrantModalDontShowAgain
            | TelemetryEvent::LogOut
            | TelemetryEvent::UpdateBlockFilterQuery
            | TelemetryEvent::BlockFilterToolbeltButtonClicked
            | TelemetryEvent::PaneDragInitiated
            | TelemetryEvent::SettingsImportResetButtonClicked
            | TelemetryEvent::ITermMultipleHotkeys
            | TelemetryEvent::DriveSharingOnboardingBlockShown
            | TelemetryEvent::SettingsImportInitiated
            | TelemetryEvent::UserMenuUpgradeClicked
            | TelemetryEvent::FileTreeItemCreated
            | TelemetryEvent::GlobalSearchOpened
            | TelemetryEvent::GlobalSearchQueryStarted => None,
            TelemetryEvent::GlobalSearchQueryCompleted {
                duration_ms,
                remote_host_count,
                total_match_count,
                capped,
                local_source_failed,
                remote_source_failures,
            } => Some(json!({
                "duration_ms": duration_ms,
                "remote_host_count": remote_host_count,
                "total_match_count": total_match_count,
                "capped": capped,
                "local_source_failed": local_source_failed,
                "remote_source_failures": remote_source_failures,
            })),
            TelemetryEvent::SSHControlMasterError { has_remote_server } => Some(json!({
                "has_remote_server": has_remote_server,
            })),
            TelemetryEvent::RemoteServerBinaryCheck {
                found,
                error,
                remote_os,
                remote_arch,
            } => Some(json!({
                "found": found,
                "error": error,
                "remote_os": remote_os,
                "remote_arch": remote_arch,
            })),
            TelemetryEvent::RemoteServerInstallation {
                error,
                install_source,
                remote_os,
                remote_arch,
            } => Some(json!({
                "error": error,
                "install_source": install_source,
                "remote_os": remote_os,
                "remote_arch": remote_arch,
            })),
            TelemetryEvent::RemoteServerInitialization {
                phase,
                error,
                remote_os,
                remote_arch,
                exit_code,
                signal_killed,
                proxy_stderr,
            } => Some(json!({
                "phase": phase,
                "error": error,
                "remote_os": remote_os,
                "remote_arch": remote_arch,
                "exit_code": exit_code,
                "signal_killed": signal_killed,
                "proxy_stderr": proxy_stderr,
            })),
            TelemetryEvent::RemoteServerDisconnection {
                remote_os,
                remote_arch,
            } => Some(json!({
                "remote_os": remote_os,
                "remote_arch": remote_arch,
            })),
            TelemetryEvent::RemoteServerReconnection {
                attempt,
                remote_os,
                remote_arch,
            } => Some(json!({
                "attempt": attempt,
                "remote_os": remote_os,
                "remote_arch": remote_arch,
            })),
            TelemetryEvent::RemoteServerReconnectExhausted {
                attempts,
                remote_os,
                remote_arch,
                exit_code,
                signal_killed,
            } => Some(json!({
                "attempts": attempts,
                "remote_os": remote_os,
                "remote_arch": remote_arch,
                "exit_code": exit_code,
                "signal_killed": signal_killed,
            })),
            TelemetryEvent::RemoteServerClientRequestError {
                operation,
                error_type,
                remote_os,
                remote_arch,
            } => Some(json!({
                "operation": operation,
                "error_type": error_type,
                "remote_os": remote_os,
                "remote_arch": remote_arch,
            })),
            TelemetryEvent::RemoteServerMessageDecodingError {
                remote_os,
                remote_arch,
            } => Some(json!({
                "remote_os": remote_os,
                "remote_arch": remote_arch,
            })),
            TelemetryEvent::RemoteServerDaemonStartup { timing_data } => {
                Some(json!({ "timing_data": timing_data }))
            }
            TelemetryEvent::RemoteServerSetupDuration {
                duration_ms,
                installed_binary,
                remote_os,
                remote_arch,
                remote_libc,
            } => Some(json!({
                "duration_ms": duration_ms,
                "installed_binary": installed_binary,
                "remote_os": remote_os,
                "remote_arch": remote_arch,
                "remote_libc": remote_libc,
            })),
            TelemetryEvent::RemoteServerHostUnsupported {
                remote_os,
                remote_arch,
                unsupported_reason,
                detected_libc,
            } => {
                let unsupported_os = match unsupported_reason {
                    remote_server::setup::UnsupportedReason::UnsupportedOs { os } => {
                        Some(os.clone())
                    }
                    remote_server::setup::UnsupportedReason::GlibcTooOld { .. }
                    | remote_server::setup::UnsupportedReason::NonGlibc { .. }
                    | remote_server::setup::UnsupportedReason::UnsupportedArch { .. } => None,
                };
                let unsupported_arch = match unsupported_reason {
                    remote_server::setup::UnsupportedReason::UnsupportedArch { arch } => {
                        Some(arch.clone())
                    }
                    remote_server::setup::UnsupportedReason::GlibcTooOld { .. }
                    | remote_server::setup::UnsupportedReason::NonGlibc { .. }
                    | remote_server::setup::UnsupportedReason::UnsupportedOs { .. } => None,
                };
                Some(json!({
                    "remote_os": remote_os,
                    "remote_arch": remote_arch,
                    "reason": unsupported_reason.as_telemetry_reason(),
                    "detected_libc": detected_libc,
                    "unsupported_os": unsupported_os,
                    "unsupported_arch": unsupported_arch,
                }))
            }
            #[cfg(windows)]
            TelemetryEvent::WSLRegistryError => None,
            TelemetryEvent::RecentMenuItemSelected { kind } => Some(json!({
                "kind": kind,
            })),
            TelemetryEvent::WarpDriveOpened {
                source,
                is_code_mode_v2,
            } => Some(json!({
                "source": source,
                "is_code_mode_v2": is_code_mode_v2,
            })),
            TelemetryEvent::LoginButtonClicked { source }
            | TelemetryEvent::LoginLaterButtonClicked { source }
            | TelemetryEvent::LoginLaterConfirmationButtonClicked { source }
            | TelemetryEvent::OpenAuthPrivacySettings { source } => Some(json!({
                "source": source,
            })),
        }
    }

    /// Returns whether the event contains user generated content, indicating it should
    /// be sent to a dedicated rudderstack source.
    pub fn contains_ugc(&self) -> bool {
        match self {
            TelemetryEvent::BootstrappingSlowContents { .. } => true,
            TelemetryEvent::BlockCompleted { .. }
            | TelemetryEvent::BlockCompletedOnDogfoodOnly { .. }
            | TelemetryEvent::BackgroundBlockStarted
            | TelemetryEvent::SessionCreation
            | TelemetryEvent::Login
            | TelemetryEvent::ConfirmSuggestion { .. }
            | TelemetryEvent::ContextMenuCopy(_, _)
            | TelemetryEvent::ContextMenuFindWithinBlocks(_)
            | TelemetryEvent::ContextMenuCopyPrompt { .. }
            | TelemetryEvent::ContextMenuToggleGitPromptDirtyIndicator { .. }
            | TelemetryEvent::ContextMenuInsertSelectedText
            | TelemetryEvent::OpenPromptEditor { .. }
            | TelemetryEvent::PromptEdited { .. }
            | TelemetryEvent::ReinputCommands(_)
            | TelemetryEvent::JumpToPreviousCommand
            | TelemetryEvent::BlockSelection(_)
            | TelemetryEvent::BootstrappingSlow(_)
            | TelemetryEvent::SessionAbandonedBeforeBootstrap { .. }
            | TelemetryEvent::BootstrappingSucceeded(_)
            | TelemetryEvent::OpenThemeChooser
            | TelemetryEvent::ThemeSelection { .. }
            | TelemetryEvent::AppIconSelection { .. }
            | TelemetryEvent::CursorDisplayType { .. }
            | TelemetryEvent::OpenThemeCreatorModal
            | TelemetryEvent::CreateCustomTheme
            | TelemetryEvent::DeleteCustomTheme
            | TelemetryEvent::SplitPane
            | TelemetryEvent::ToggleRestoreSession(_)
            | TelemetryEvent::DatabaseStartUpError(_)
            | TelemetryEvent::DatabaseReadError(_)
            | TelemetryEvent::DatabaseWriteError(_)
            | TelemetryEvent::AppStartup(_)
            | TelemetryEvent::LoggedOutStartup
            | TelemetryEvent::DownloadSource(_)
            | TelemetryEvent::SSHBootstrapAttempt(_)
            | TelemetryEvent::SSHControlMasterError { .. }
            | TelemetryEvent::KeybindingChanged { .. }
            | TelemetryEvent::KeybindingResetToDefault { .. }
            | TelemetryEvent::KeybindingRemoved { .. }
            | TelemetryEvent::FeaturesPageAction { .. }
            | TelemetryEvent::OpenQuakeModeWindow
            | TelemetryEvent::OpenWelcomeTips
            | TelemetryEvent::DismissWelcomeTips
            | TelemetryEvent::ShowNotificationsDiscoveryBanner
            | TelemetryEvent::NotificationsDiscoveryBannerAction(_)
            | TelemetryEvent::ShowNotificationsErrorBanner
            | TelemetryEvent::NotificationsErrorBannerAction(_)
            | TelemetryEvent::NotificationPermissionsRequested { .. }
            | TelemetryEvent::NotificationsRequestPermissionsOutcome { .. }
            | TelemetryEvent::NotificationSent { .. }
            | TelemetryEvent::NotificationFailedToSend { .. }
            | TelemetryEvent::NotificationClicked
            | TelemetryEvent::ToggleFindOption { .. }
            | TelemetryEvent::SignUpButtonClicked
            | TelemetryEvent::LoginButtonClicked { .. }
            | TelemetryEvent::LoginLaterButtonClicked { .. }
            | TelemetryEvent::LoginLaterConfirmationButtonClicked { .. }
            | TelemetryEvent::OpenNewSessionFromFilePath
            | TelemetryEvent::AuthCommonQuestionClicked { .. }
            | TelemetryEvent::AuthToggleFAQ { .. }
            | TelemetryEvent::OpenAuthPrivacySettings { .. }
            | TelemetryEvent::TabRenamed(_)
            | TelemetryEvent::MoveActiveTab { .. }
            | TelemetryEvent::MoveTab { .. }
            | TelemetryEvent::DragAndDropTab
            | TelemetryEvent::DragAndDropTabGroup
            | TelemetryEvent::TabOperations { .. }
            | TelemetryEvent::TriedToExecuteBeforePrecmd
            | TelemetryEvent::ThinStrokesSettingChanged { .. }
            | TelemetryEvent::BookmarkBlockToggled { .. }
            | TelemetryEvent::JumpToBookmark
            | TelemetryEvent::JumpToBottomofBlockButtonClicked
            | TelemetryEvent::ToggleJumpToBottomofBlockButton { .. }
            | TelemetryEvent::ToggleShowBlockDividers { .. }
            | TelemetryEvent::ShowInFileExplorer
            | TelemetryEvent::OpenLaunchConfigSaveModal
            | TelemetryEvent::SaveLaunchConfig { .. }
            | TelemetryEvent::OpenLaunchConfigFile
            | TelemetryEvent::OpenLaunchConfig { .. }
            | TelemetryEvent::TeamCreated
            | TelemetryEvent::TeamJoined
            | TelemetryEvent::TeamLeft
            | TelemetryEvent::ToggleSettingsSync { .. }
            | TelemetryEvent::RemovedUserFromTeam
            | TelemetryEvent::DeletedNotebook
            | TelemetryEvent::ToggleApprovalsModal
            | TelemetryEvent::SetLineHeight { .. }
            | TelemetryEvent::ResourceCenterOpened
            | TelemetryEvent::ResourceCenterTipsCompleted
            | TelemetryEvent::ResourceCenterTipsSkipped
            | TelemetryEvent::KeybindingsPageOpened
            | TelemetryEvent::GlobalSearchOpened
            | TelemetryEvent::GlobalSearchQueryStarted
            | TelemetryEvent::GlobalSearchQueryCompleted { .. }
            | TelemetryEvent::OpenNotebook(_)
            | TelemetryEvent::EditNotebook { .. }
            | TelemetryEvent::NotebookAction(_)
            | TelemetryEvent::OpenedAltScreenFind
            | TelemetryEvent::UserInitiatedClose { .. }
            | TelemetryEvent::QuitModalShown { .. }
            | TelemetryEvent::QuitModalCancel { .. }
            | TelemetryEvent::QuitModalDisabled
            | TelemetryEvent::UserInitiatedLogOut
            | TelemetryEvent::LogOutModalShown
            | TelemetryEvent::LogOutModalCancel { .. }
            | TelemetryEvent::SetOpacity { .. }
            | TelemetryEvent::SetBlurRadius { .. }
            | TelemetryEvent::ToggleDimInactivePanes { .. }
            | TelemetryEvent::InputModeChanged { .. }
            | TelemetryEvent::PtySpawned { .. }
            | TelemetryEvent::InitialWorkingDirectoryConfigurationChanged { .. }
            | TelemetryEvent::ToggleFocusPaneOnHover { .. }
            | TelemetryEvent::OpenInputContextMenu
            | TelemetryEvent::InputCutSelectedText
            | TelemetryEvent::InputCopySelectedText
            | TelemetryEvent::InputSelectAll
            | TelemetryEvent::InputPaste
            | TelemetryEvent::InputCommandSearch
            | TelemetryEvent::ExperimentTriggered { .. }
            | TelemetryEvent::ToggleSyncAllPanesInAllTabs { .. }
            | TelemetryEvent::ToggleSyncAllPanesInTab { .. }
            | TelemetryEvent::ToggleSameLinePrompt { .. }
            | TelemetryEvent::ToggleNewWindowsAtCustomSize { .. }
            | TelemetryEvent::SetNewWindowsAtCustomSize
            | TelemetryEvent::DisableInputSync
            | TelemetryEvent::ToggleTabIndicators { .. }
            | TelemetryEvent::TogglePreserveActiveTabColor { .. }
            | TelemetryEvent::ShowSubshellBanner
            | TelemetryEvent::DeclineSubshellBootstrap { .. }
            | TelemetryEvent::TriggerSubshellBootstrap { .. }
            | TelemetryEvent::AddDenylistedSubshellCommand
            | TelemetryEvent::RemoveDenylistedSubshellCommand
            | TelemetryEvent::AddAddedSubshellCommand
            | TelemetryEvent::RemoveAddedSubshellCommand
            | TelemetryEvent::ReceivedSubshellRcFileDcs
            | TelemetryEvent::WarpifyFooterShown { .. }
            | TelemetryEvent::WarpifyFooterAcceptedWarpify { .. }
            | TelemetryEvent::ShowAliasExpansionBanner
            | TelemetryEvent::EnableAliasExpansionFromBanner
            | TelemetryEvent::DismissAliasExpansionBanner
            | TelemetryEvent::ShowVimKeybindingsBanner
            | TelemetryEvent::EnableVimKeybindingsFromBanner
            | TelemetryEvent::DismissVimKeybindingsBanner
            | TelemetryEvent::InitiateReauth
            | TelemetryEvent::InitiateAnonymousUserSignup { .. }
            | TelemetryEvent::AnonymousUserExpirationLockout
            | TelemetryEvent::AnonymousUserLinkedFromBrowser
            | TelemetryEvent::AnonymousUserAttemptLoginGatedFeature { .. }
            | TelemetryEvent::AnonymousUserHitCloudObjectLimit
            | TelemetryEvent::NeedsReauth
            | TelemetryEvent::WarpDriveOpened { .. }
            | TelemetryEvent::ToggleSecretRedaction { .. }
            | TelemetryEvent::CustomSecretRegexAdded
            | TelemetryEvent::ToggleObfuscateSecret { .. }
            | TelemetryEvent::CopySecret
            | TelemetryEvent::UpdateSortingChoice { .. }
            | TelemetryEvent::UndoClose { .. }
            | TelemetryEvent::PtyThroughput { .. }
            | TelemetryEvent::DuplicateObject(_)
            | TelemetryEvent::DriveSharingOnboardingBlockShown
            | TelemetryEvent::CommandFileRun
            | TelemetryEvent::PageUpDownInEditorPressed { .. }
            | TelemetryEvent::StartedSharingCurrentSession { .. }
            | TelemetryEvent::StoppedSharingCurrentSession { .. }
            | TelemetryEvent::JoinedSharedSession { .. }
            | TelemetryEvent::SharerCancelledGrantRole { .. }
            | TelemetryEvent::SharerGrantModalDontShowAgain
            | TelemetryEvent::JumpToSharedSessionParticipant { .. }
            | TelemetryEvent::CopiedSharedSessionLink { .. }
            | TelemetryEvent::WebSessionOpenedOnDesktop { .. }
            | TelemetryEvent::WebCloudObjectOpenedOnDesktop { .. }
            | TelemetryEvent::UnsupportedShell { .. }
            | TelemetryEvent::LogOut
            | TelemetryEvent::InviteTeammates { .. }
            | TelemetryEvent::CopyObjectToClipboard(_)
            | TelemetryEvent::OpenAndWarpifyDockerSubshell { .. }
            | TelemetryEvent::UpdateBlockFilterQuery
            | TelemetryEvent::UpdateBlockFilterQueryContextLines { .. }
            | TelemetryEvent::ToggleBlockFilterQuery { .. }
            | TelemetryEvent::ToggleBlockFilterCaseSensitivity { .. }
            | TelemetryEvent::ToggleBlockFilterRegex { .. }
            | TelemetryEvent::ToggleBlockFilterInvert { .. }
            | TelemetryEvent::BlockFilterToolbeltButtonClicked
            | TelemetryEvent::ToggleSnackbarInActivePane { .. }
            | TelemetryEvent::PaneDragInitiated
            | TelemetryEvent::PaneDropped { .. }
            | TelemetryEvent::ObjectLinkCopied { .. }
            | TelemetryEvent::FileTreeToggled { .. }
            | TelemetryEvent::TierLimitHit(_)
            | TelemetryEvent::ResourceUsageStats { .. }
            | TelemetryEvent::MemoryUsageStats { .. }
            | TelemetryEvent::MemoryUsageHigh { .. }
            | TelemetryEvent::TransientMemorySpike { .. }
            | TelemetryEvent::EnvVarCollectionInvoked(_)
            | TelemetryEvent::EnvVarWorkflowParameterization(_)
            | TelemetryEvent::CompletedSettingsImport { .. }
            | TelemetryEvent::SettingsImportConfigFocused(_)
            | TelemetryEvent::SettingsImportResetButtonClicked
            | TelemetryEvent::ITermMultipleHotkeys
            | TelemetryEvent::ToggleWorkspaceDecorationVisibility { .. }
            | TelemetryEvent::UpdateAltScreenPaddingMode { .. }
            | TelemetryEvent::AddTabWithShell { .. }
            | TelemetryEvent::OpenedSharingDialog(_)
            | TelemetryEvent::ToggleLigatureRendering { .. }
            | TelemetryEvent::ToggleSshWarpification { .. }
            | TelemetryEvent::SetSshExtensionInstallMode { .. }
            | TelemetryEvent::SshRemoteServerChoiceDoNotAskAgainToggled { .. }
            | TelemetryEvent::SettingsImportInitiated
            | TelemetryEvent::ShellTerminatedPrematurely { .. }
            | TelemetryEvent::InputUXModeChanged { .. }
            | TelemetryEvent::UserMenuUpgradeClicked
            | TelemetryEvent::TabCloseButtonPositionUpdated { .. }
            | TelemetryEvent::FileTreeItemCreated
            | TelemetryEvent::RecentMenuItemSelected { .. }
            | TelemetryEvent::RemoteServerBinaryCheck { .. }
            | TelemetryEvent::RemoteServerInstallation { .. }
            | TelemetryEvent::RemoteServerInitialization { .. }
            | TelemetryEvent::RemoteServerDaemonStartup { .. }
            | TelemetryEvent::RemoteServerDisconnection { .. }
            | TelemetryEvent::RemoteServerClientRequestError { .. }
            | TelemetryEvent::RemoteServerMessageDecodingError { .. }
            | TelemetryEvent::RemoteServerSetupDuration { .. }
            | TelemetryEvent::RemoteServerHostUnsupported { .. }
            | TelemetryEvent::RemoteServerReconnection { .. }
            | TelemetryEvent::RemoteServerReconnectExhausted { .. } => false,
            #[cfg(feature = "local_fs")]
            TelemetryEvent::CodePaneOpened { .. }
            | TelemetryEvent::CodePanelsFileOpened { .. }
            | TelemetryEvent::PreviewPanePromoted => false,
            #[cfg(windows)]
            TelemetryEvent::WSLRegistryError => false,
        }
    }

    /// Prints a JSON containing all telemetry events enabled for the current build.
    /// The keys are the event name and the values are the event description.
    #[cfg(not(target_family = "wasm"))]
    pub fn print_telemetry_events_json() -> anyhow::Result<()> {
        // We initialize the feature flags so that we can determine which telemetry events to print.
        crate::features::init_feature_flags();

        let events: serde_json::Map<String, Value> = warp_core::telemetry::all_events()
            .filter_map(|event| {
                if !event.enablement_state().is_enabled() {
                    return None;
                }

                Some((
                    event.name().to_string(),
                    Value::String(event.description().to_string()),
                ))
            })
            .collect();

        let json_pretty_print_string = serde_json::to_string_pretty(&events)?;
        println!("{json_pretty_print_string}");
        Ok(())
    }
}

impl TelemetryEventDesc for TelemetryEventDiscriminants {
    fn enablement_state(&self) -> EnablementState {
        // We disallow the wildcard statement to prevent us from accidentally ignoring any
        // variants added in the future. Going forward, we should associate all new telemetry events
        // with a feature flag when appropriate.
        #[deny(clippy::wildcard_enum_match_arm)]
        match self {
            Self::ObjectLinkCopied => EnablementState::Always,
            Self::FileTreeToggled => EnablementState::Flag(FeatureFlag::FileTree),
            Self::FileTreeItemCreated => EnablementState::Flag(FeatureFlag::FileTree),
            Self::PtyThroughput => EnablementState::Flag(FeatureFlag::RecordPtyThroughput),
            #[cfg(feature = "local_fs")]
            Self::CodePaneOpened { .. } => EnablementState::Always,
            #[cfg(feature = "local_fs")]
            Self::CodePanelsFileOpened { .. } => EnablementState::Always,
            #[cfg(feature = "local_fs")]
            Self::PreviewPanePromoted => EnablementState::Always,
            Self::ToggleFocusPaneOnHover { .. } => EnablementState::Always,
            Self::InitiateAnonymousUserSignup { .. }
            | Self::LoginLaterButtonClicked
            | Self::LoginLaterConfirmationButtonClicked
            | Self::AnonymousUserExpirationLockout
            | Self::AnonymousUserLinkedFromBrowser
            | Self::AnonymousUserAttemptLoginGatedFeature
            | Self::AnonymousUserHitCloudObjectLimit => EnablementState::Always,

            Self::StartedSharingCurrentSession | Self::StoppedSharingCurrentSession => {
                EnablementState::Flag(FeatureFlag::CreatingSharedSessions)
            }
            Self::JoinedSharedSession => EnablementState::Flag(FeatureFlag::ViewingSharedSessions),
            Self::OpenNotebook | Self::EditNotebook | Self::NotebookAction => {
                EnablementState::Always
            }
            Self::ToggleSettingsSync { .. } => EnablementState::Always,
            Self::BlockCompleted => EnablementState::Always,
            Self::BackgroundBlockStarted => EnablementState::Always,
            Self::SessionCreation => EnablementState::Always,
            Self::Login => EnablementState::Always,
            Self::ConfirmSuggestion => EnablementState::Always,
            Self::ContextMenuCopy => EnablementState::Always,
            Self::ContextMenuFindWithinBlocks => EnablementState::Always,
            Self::ContextMenuCopyPrompt => EnablementState::Always,
            Self::ContextMenuToggleGitPromptDirtyIndicator => EnablementState::Always,
            Self::ContextMenuInsertSelectedText => EnablementState::Always,
            Self::OpenPromptEditor => EnablementState::Always,
            Self::PromptEdited => EnablementState::Always,
            Self::ReinputCommands => EnablementState::Always,
            Self::JumpToPreviousCommand => EnablementState::Always,
            Self::BlockSelection => EnablementState::Always,
            Self::BootstrappingSlow => EnablementState::Always,
            Self::BootstrappingSlowContents => EnablementState::Always,
            Self::SessionAbandonedBeforeBootstrap => EnablementState::Always,
            Self::BootstrappingSucceeded => EnablementState::Always,
            Self::OpenThemeChooser => EnablementState::Always,
            Self::ThemeSelection => EnablementState::Always,
            Self::AppIconSelection => EnablementState::Always,
            Self::CursorDisplayType => EnablementState::Always,
            Self::OpenThemeCreatorModal => EnablementState::Always,
            Self::CreateCustomTheme => EnablementState::Always,
            Self::DeleteCustomTheme => EnablementState::Always,
            Self::SplitPane => EnablementState::Always,
            Self::ToggleRestoreSession => EnablementState::Always,
            Self::DatabaseStartUpError => EnablementState::Always,
            Self::DatabaseReadError => EnablementState::Always,
            Self::DatabaseWriteError => EnablementState::Always,
            Self::AppStartup => EnablementState::Always,
            Self::LoggedOutStartup => EnablementState::Always,
            Self::DownloadSource => EnablementState::Always,
            Self::SSHBootstrapAttempt => EnablementState::Always,
            Self::SSHControlMasterError => EnablementState::Always,
            Self::KeybindingChanged => EnablementState::Always,
            Self::KeybindingResetToDefault => EnablementState::Always,
            Self::KeybindingRemoved => EnablementState::Always,
            Self::FeaturesPageAction => EnablementState::Always,
            Self::OpenQuakeModeWindow => EnablementState::Always,
            Self::OpenWelcomeTips => EnablementState::Always,
            Self::DismissWelcomeTips => EnablementState::Always,
            Self::ShowNotificationsDiscoveryBanner => EnablementState::Always,
            Self::NotificationsDiscoveryBannerAction => EnablementState::Always,
            Self::ShowNotificationsErrorBanner => EnablementState::Always,
            Self::NotificationsErrorBannerAction => EnablementState::Always,
            Self::NotificationPermissionsRequested => EnablementState::Always,
            Self::NotificationsRequestPermissionsOutcome => EnablementState::Always,
            Self::NotificationSent => EnablementState::Always,
            Self::NotificationFailedToSend => EnablementState::Always,
            Self::NotificationClicked => EnablementState::Always,
            Self::ToggleFindOption => EnablementState::Always,
            Self::SignUpButtonClicked => EnablementState::Always,
            Self::LoginButtonClicked => EnablementState::Always,
            Self::OpenNewSessionFromFilePath => EnablementState::Always,
            Self::AuthCommonQuestionClicked => EnablementState::Always,
            Self::AuthToggleFAQ => EnablementState::Always,
            Self::OpenAuthPrivacySettings => EnablementState::Always,
            Self::TabRenamed => EnablementState::Always,
            Self::MoveActiveTab => EnablementState::Always,
            Self::MoveTab => EnablementState::Always,
            Self::DragAndDropTab => EnablementState::Always,
            Self::DragAndDropTabGroup => EnablementState::Always,
            Self::TabOperations => EnablementState::Always,
            Self::TriedToExecuteBeforePrecmd => EnablementState::Always,
            Self::ThinStrokesSettingChanged => EnablementState::Always,
            Self::BookmarkBlockToggled => EnablementState::Always,
            Self::JumpToBookmark => EnablementState::Always,
            Self::JumpToBottomofBlockButtonClicked => EnablementState::Always,
            Self::ToggleJumpToBottomofBlockButton => EnablementState::Always,
            Self::ShowInFileExplorer => EnablementState::Always,
            Self::OpenLaunchConfigSaveModal => EnablementState::Always,
            Self::SaveLaunchConfig => EnablementState::Always,
            Self::OpenLaunchConfigFile => EnablementState::Always,
            Self::OpenLaunchConfig => EnablementState::Always,
            Self::TeamCreated => EnablementState::Always,
            Self::TeamJoined => EnablementState::Always,
            Self::TeamLeft => EnablementState::Always,
            Self::RemovedUserFromTeam => EnablementState::Always,
            Self::DeletedNotebook => EnablementState::Always,
            Self::ToggleApprovalsModal => EnablementState::Always,
            Self::SetLineHeight => EnablementState::Always,
            Self::ResourceCenterOpened => EnablementState::Always,
            Self::ResourceCenterTipsCompleted => EnablementState::Always,
            Self::ResourceCenterTipsSkipped => EnablementState::Always,
            Self::KeybindingsPageOpened => EnablementState::Always,
            Self::GlobalSearchOpened => EnablementState::Always,
            Self::GlobalSearchQueryStarted => EnablementState::Always,
            Self::GlobalSearchQueryCompleted => EnablementState::Always,
            Self::OpenedAltScreenFind => EnablementState::Always,
            Self::UserInitiatedClose => EnablementState::Always,
            Self::QuitModalShown => EnablementState::Always,
            Self::QuitModalCancel => EnablementState::Always,
            Self::QuitModalDisabled => EnablementState::Always,
            Self::UserInitiatedLogOut => EnablementState::Always,
            Self::LogOutModalShown => EnablementState::Always,
            Self::LogOutModalCancel => EnablementState::Always,
            Self::SetOpacity => EnablementState::Always,
            Self::SetBlurRadius => EnablementState::Always,
            Self::ToggleDimInactivePanes => EnablementState::Always,
            Self::InputModeChanged => EnablementState::Always,
            Self::PtySpawned => EnablementState::Always,
            Self::InitialWorkingDirectoryConfigurationChanged => EnablementState::Always,
            Self::OpenInputContextMenu => EnablementState::Always,
            Self::InputCutSelectedText => EnablementState::Always,
            Self::InputCopySelectedText => EnablementState::Always,
            Self::InputSelectAll => EnablementState::Always,
            Self::InputPaste => EnablementState::Always,
            Self::InputCommandSearch => EnablementState::Always,
            Self::ExperimentTriggered => EnablementState::Always,
            Self::ToggleSyncAllPanesInAllTabs => EnablementState::Always,
            Self::ToggleSyncAllPanesInTab => EnablementState::Always,
            Self::ToggleSameLinePrompt => EnablementState::Always,
            Self::ToggleNewWindowsAtCustomSize => EnablementState::Always,
            Self::SetNewWindowsAtCustomSize => EnablementState::Always,
            Self::DisableInputSync => EnablementState::Always,
            Self::ToggleTabIndicators => EnablementState::Always,
            Self::TogglePreserveActiveTabColor => EnablementState::Always,
            Self::ShowSubshellBanner => EnablementState::Always,
            Self::DeclineSubshellBootstrap => EnablementState::Always,
            Self::TriggerSubshellBootstrap => EnablementState::Always,
            Self::AddDenylistedSubshellCommand => EnablementState::Always,
            Self::RemoveDenylistedSubshellCommand => EnablementState::Always,
            Self::ToggleSshWarpification => EnablementState::Always,
            Self::SetSshExtensionInstallMode => EnablementState::Always,
            Self::SshRemoteServerChoiceDoNotAskAgainToggled => EnablementState::Always,
            Self::WarpifyFooterShown | Self::WarpifyFooterAcceptedWarpify => {
                EnablementState::Always
            }
            Self::AddAddedSubshellCommand => EnablementState::Always,
            Self::RemoveAddedSubshellCommand => EnablementState::Always,
            Self::ReceivedSubshellRcFileDcs => EnablementState::Always,
            Self::ShowAliasExpansionBanner => EnablementState::Always,
            Self::EnableAliasExpansionFromBanner => EnablementState::Always,
            Self::DismissAliasExpansionBanner => EnablementState::Always,
            Self::ShowVimKeybindingsBanner => EnablementState::Always,
            Self::EnableVimKeybindingsFromBanner => EnablementState::Always,
            Self::DismissVimKeybindingsBanner => EnablementState::Always,
            Self::InitiateReauth => EnablementState::Always,
            Self::NeedsReauth => EnablementState::Always,
            Self::WarpDriveOpened => EnablementState::Always,
            Self::ToggleSecretRedaction => EnablementState::Always,
            Self::CustomSecretRegexAdded => EnablementState::Always,
            Self::ToggleObfuscateSecret => EnablementState::Always,
            Self::CopySecret => EnablementState::Always,
            Self::UpdateSortingChoice => EnablementState::Always,
            Self::UndoClose => EnablementState::Always,
            Self::DuplicateObject => EnablementState::Always,
            Self::CommandFileRun => EnablementState::Always,
            Self::PageUpDownInEditorPressed => EnablementState::Always,
            Self::UnsupportedShell => EnablementState::Always,
            Self::LogOut => EnablementState::Always,
            Self::SettingsImportInitiated => EnablementState::Always,
            Self::InviteTeammates => EnablementState::Always,
            Self::CopyObjectToClipboard => EnablementState::Always,
            Self::OpenAndWarpifyDockerSubshell => EnablementState::Always,
            Self::UpdateBlockFilterQuery => EnablementState::Always,
            Self::UpdateBlockFilterQueryContextLines => EnablementState::Always,
            Self::ToggleBlockFilterQuery => EnablementState::Always,
            Self::ToggleBlockFilterCaseSensitivity => EnablementState::Always,
            Self::ToggleBlockFilterRegex => EnablementState::Always,
            Self::ToggleBlockFilterInvert => EnablementState::Always,
            Self::BlockFilterToolbeltButtonClicked => EnablementState::Always,
            Self::ToggleSnackbarInActivePane => EnablementState::Always,
            Self::PaneDragInitiated => EnablementState::Always,
            Self::PaneDropped => EnablementState::Always,
            Self::TierLimitHit => EnablementState::Always,
            Self::SharerCancelledGrantRole => EnablementState::Always,
            Self::SharerGrantModalDontShowAgain => EnablementState::Always,
            Self::JumpToSharedSessionParticipant => EnablementState::Always,
            Self::CopiedSharedSessionLink => EnablementState::Always,
            Self::WebSessionOpenedOnDesktop => EnablementState::Always,
            Self::WebCloudObjectOpenedOnDesktop => EnablementState::Always,
            Self::ToggleShowBlockDividers => EnablementState::Flag(FeatureFlag::MinimalistUI),
            Self::DriveSharingOnboardingBlockShown => EnablementState::Always,
            Self::ResourceUsageStats => EnablementState::Always,
            Self::MemoryUsageStats => EnablementState::ChannelSpecific {
                channels: vec![Channel::Local, Channel::Dev],
            },
            Self::MemoryUsageHigh => EnablementState::Always,
            Self::TransientMemorySpike => EnablementState::Always,
            Self::EnvVarCollectionInvoked | Self::EnvVarWorkflowParameterization => {
                EnablementState::Always
            }
            Self::BlockCompletedOnDogfoodOnly => EnablementState::ChannelSpecific {
                channels: vec![Channel::Local, Channel::Dev],
            },
            Self::CompletedSettingsImport
            | Self::SettingsImportConfigFocused
            | Self::SettingsImportResetButtonClicked
            | Self::ITermMultipleHotkeys => EnablementState::Always,
            Self::ToggleWorkspaceDecorationVisibility => {
                EnablementState::Flag(FeatureFlag::FullScreenZenMode)
            }
            Self::UpdateAltScreenPaddingMode => EnablementState::Always,
            Self::AddTabWithShell => EnablementState::Flag(FeatureFlag::ShellSelector),
            Self::OpenedSharingDialog => EnablementState::Always,
            Self::ToggleLigatureRendering => EnablementState::Flag(FeatureFlag::Ligatures),
            #[cfg(windows)]
            Self::WSLRegistryError => EnablementState::Always,
            Self::ShellTerminatedPrematurely { .. } => EnablementState::Always,
            Self::InputUXModeChanged { .. } => EnablementState::Always,
            Self::UserMenuUpgradeClicked => EnablementState::Always,
            Self::TabCloseButtonPositionUpdated { .. } => EnablementState::Always,
            Self::RecentMenuItemSelected => EnablementState::Always,
            Self::RemoteServerBinaryCheck
            | Self::RemoteServerInstallation
            | Self::RemoteServerInitialization
            | Self::RemoteServerDaemonStartup
            | Self::RemoteServerDisconnection
            | Self::RemoteServerClientRequestError
            | Self::RemoteServerMessageDecodingError
            | Self::RemoteServerSetupDuration
            | Self::RemoteServerHostUnsupported
            | Self::RemoteServerReconnection
            | Self::RemoteServerReconnectExhausted => {
                EnablementState::Flag(FeatureFlag::SshRemoteServer)
            }
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::BlockCompleted => "Block Creation",
            Self::BlockCompletedOnDogfoodOnly => "Block Completed (dogfood only)",
            Self::BackgroundBlockStarted => "Background Block Started",
            Self::SessionCreation => "Tab Creation",
            Self::Login => "Logged in to native app",
            Self::ReinputCommands => "Context Menu: Reinput Commands",
            Self::ToggleSettingsSync => "Toggle Settings Sync",
            Self::ToggleFocusPaneOnHover => "Toggle Focus Pane On Hover",
            Self::LoginLaterButtonClicked => "Login Later Button Clicked",
            Self::LoginLaterConfirmationButtonClicked => "Login Later Confirmation Button Clicked",
            Self::JumpToPreviousCommand => "Jumped to Previous Command",
            Self::ContextMenuFindWithinBlocks => "Context Menu: Find Within Blocks",
            Self::ContextMenuCopy => "Context Menu Copy",
            Self::BlockSelection => "Block Selection",
            Self::BootstrappingSlow => "Bootstrapping Slow",
            Self::BootstrappingSlowContents => "Bootstrap Slow Contents",
            Self::ObjectLinkCopied => "Object Link Copied",
            Self::FileTreeToggled => "File Tree Toggled",
            Self::FileTreeItemCreated => "FileTree.ItemCreated",
            Self::InitiateAnonymousUserSignup => "Anonymous User Initiated Signup",
            Self::AnonymousUserExpirationLockout => "Anonymous User Expiration Lockout",
            Self::AnonymousUserLinkedFromBrowser => "Anonymous User Linked from Browser",
            Self::AnonymousUserAttemptLoginGatedFeature => {
                "Anonymous User Attempted Login-Gated Feature"
            }
            #[cfg(feature = "local_fs")]
            Self::CodePaneOpened { .. } => "Code Pane Opened",
            #[cfg(feature = "local_fs")]
            Self::CodePanelsFileOpened { .. } => "CodePanels.FileOpened",
            #[cfg(feature = "local_fs")]
            Self::PreviewPanePromoted => "Preview Pane Promoted",
            Self::AnonymousUserHitCloudObjectLimit => "Anonymous User Hit Cloud Object Limit",
            Self::BootstrappingSucceeded => "Bootstrapping Succeeded",
            Self::SessionAbandonedBeforeBootstrap => "Session Abandoned Before Bootstrap",
            Self::ConfirmSuggestion => "Confirm Suggestion",
            Self::ContextMenuInsertSelectedText => "Context Menu Insert Selected Text into Input",
            Self::ContextMenuCopyPrompt => "Context Menu Copy Prompt",
            Self::ContextMenuToggleGitPromptDirtyIndicator => {
                "Context Menu Toggle Git Prompt Dirty Indicator"
            }
            Self::OpenThemeChooser => "Open Theme Chooser",
            Self::ThemeSelection => "Select Theme",
            Self::AppIconSelection => "Select App Icon",
            Self::CursorDisplayType => "Select Cursor Type",
            Self::OpenThemeCreatorModal => "Open Theme Creator Modal",
            Self::CreateCustomTheme => "Create Custom Theme",
            Self::DeleteCustomTheme => "Delete Custom Theme",
            Self::SplitPane => "Split Pane",
            Self::ToggleRestoreSession => "Toggle Restore Session",
            Self::DatabaseStartUpError => "Database Startup Error",
            Self::DatabaseWriteError => "Database Write Error",
            Self::DatabaseReadError => "Database Read Error",
            Self::AppStartup => "App Startup",
            Self::LoggedOutStartup => "Logged-out App Startup",
            Self::DownloadSource => "App Download Source",
            Self::SSHBootstrapAttempt => "SSH Bootstrap Attempt",
            Self::SSHControlMasterError => "SSH ControlMaster Error",
            Self::SetNewWindowsAtCustomSize => "Set New Windows at Custom Size",
            Self::ToggleNewWindowsAtCustomSize => "Toggle New Windows at Custom Size",
            Self::KeybindingChanged => "Keybinding Changed",
            Self::KeybindingResetToDefault => "Keybinding Reset to Default",
            Self::KeybindingRemoved => "Keybinding Removed",
            Self::FeaturesPageAction => "Features Page Action",
            Self::OpenQuakeModeWindow => "Open Quake Mode Window",
            Self::OpenWelcomeTips => "Open Welcome Tips",
            Self::DismissWelcomeTips => "Dismiss Welcome Tips",
            Self::ShowNotificationsDiscoveryBanner => "ShowNotificationsDiscoveryBanner",
            Self::NotificationsDiscoveryBannerAction => "Notifications Discovery Banner Action",
            Self::ShowNotificationsErrorBanner => "ShowNotificationsErrorBanner",
            Self::NotificationsErrorBannerAction => "Notifications Error Banner Action",
            Self::NotificationPermissionsRequested => "Notification Permissions Requested",
            Self::NotificationSent => "Notification Sent",
            Self::NotificationFailedToSend => "Notification Failed to Send",
            Self::NotificationClicked => "Notification Clicked",
            Self::NotificationsRequestPermissionsOutcome => {
                "Notification Request Permissions Outcome"
            }
            Self::ToggleFindOption => "Find Option Toggled",
            Self::SignUpButtonClicked => "Sign Up Button Clicked in App",
            Self::LoginButtonClicked => "Log In Button Clicked in App",
            Self::OpenNewSessionFromFilePath => "New Session From Directory",
            Self::AuthCommonQuestionClicked => "Auth Common Question Clicked in App",
            Self::AuthToggleFAQ => "Auth: Toggle Common Questions",
            Self::OpenAuthPrivacySettings => "Auth: Open Privacy Settings Overlay",
            Self::TabRenamed => "Tab Renamed",
            Self::MoveActiveTab => "Move Active Tab",
            Self::MoveTab => "Move Tab",
            Self::DragAndDropTab => "Drag and Drop Tab",
            Self::DragAndDropTabGroup => "Drag and Drop Tab Group",
            Self::TabOperations => "Tab Operations",
            Self::TriedToExecuteBeforePrecmd => "Tried to Execute Before Precmd",
            Self::ThinStrokesSettingChanged => "Thin Strokes Setting Changed",
            Self::BookmarkBlockToggled => "Toggled Bookmark Block",
            Self::JumpToBookmark => "Jumped to Bookmark Block",
            Self::JumpToBottomofBlockButtonClicked => "Jumped to Bottom of Block Button Clicked",
            Self::ShowInFileExplorer => "Showed File in File Explorer",
            Self::OpenLaunchConfigSaveModal => "Open Save Config Modal",
            Self::SaveLaunchConfig => "Save Launch Config",
            Self::OpenLaunchConfigFile => "Open Launch Config File",
            Self::OpenLaunchConfig => "Open Launch Config",
            Self::LogOut => "Log Out",
            Self::SetLineHeight => "Set Line Height",
            Self::ResourceCenterOpened => "Resource Center Opened",
            Self::ResourceCenterTipsCompleted => "Resource Center Tips Completed",
            Self::ResourceCenterTipsSkipped => "Resource Center Tips Skipped",
            Self::KeybindingsPageOpened => "Resource Center Keybindings Page Opened",
            Self::GlobalSearchOpened => "Global Search Opened",
            Self::GlobalSearchQueryStarted => "Global Search Query Started",
            Self::GlobalSearchQueryCompleted => "Global Search Query Completed",
            Self::OpenNotebook => "Notebook Opened",
            Self::EditNotebook => "Notebook Edited",
            Self::NotebookAction => "Notebook Action",
            Self::OpenedAltScreenFind => "Opened alt screen find bar",
            Self::UserInitiatedClose => "User Initiated Closing Something",
            Self::QuitModalShown => "Quit Modal Shown",
            Self::QuitModalCancel => "Quit Modal Cancel Pressed",
            Self::QuitModalDisabled => "Quit Modal Disabled",
            Self::UserInitiatedLogOut => "User Initiated Log Out",
            Self::LogOutModalShown => "Log Out Modal Shown",
            Self::LogOutModalCancel => "Log Out Modal Cancel Pressed",
            Self::SetBlurRadius => "Set Window Blur Radius",
            Self::SetOpacity => "Set Window Opacity",
            Self::ToggleDimInactivePanes => "Toggle Dim Inactive Panes",
            Self::ToggleJumpToBottomofBlockButton => "Toggle Jump to Bottom of Block Button",
            Self::ToggleShowBlockDividers => "Toggle Show Block Dividers",
            Self::PtySpawned => "Pty Spawned",
            Self::InitialWorkingDirectoryConfigurationChanged => {
                "InitialWorkingDirectoryConfigurationChanged"
            }
            Self::InputModeChanged => "Input Mode Changed",
            Self::OpenInputContextMenu => "OpenInputBoxContextMenu",
            Self::InputCutSelectedText => "InputBoxCutSelectedText",
            Self::InputCopySelectedText => "InputBoxCutSelectedText",
            Self::InputSelectAll => "InputBoxSelectAll",
            Self::InputPaste => "InputBoxPaste",
            Self::InputCommandSearch => "InputBoxCommandSearch",
            Self::ExperimentTriggered => "experiments.client.enroll_client",
            Self::ToggleSyncAllPanesInAllTabs => "Toggle Sync Inputs Across All Panes in All Tabs",
            Self::ToggleSyncAllPanesInTab => "Toggle Sync Inputs Across All Panes in Current Tab",
            Self::ToggleSameLinePrompt => "Toggle Same Line Prompt",
            Self::DisableInputSync => "Disable Input Sync Inputs",
            Self::ToggleTabIndicators => "Toggle Tab Indicators",
            Self::TogglePreserveActiveTabColor => "Toggle Preserve Active Tab Color",
            Self::ShowSubshellBanner => "Show Subshell Banner",
            Self::DeclineSubshellBootstrap => "Decline Subshell Bootstrap",
            Self::TriggerSubshellBootstrap => "Trigger Subshell Bootstrap",
            Self::AddDenylistedSubshellCommand => "Add Denylisted Subshell Command",
            Self::RemoveDenylistedSubshellCommand => "Remove Denylisted Subshell Command",
            Self::AddAddedSubshellCommand => "Add Added Subshell Command",
            Self::RemoveAddedSubshellCommand => "Remove Added Subshell Command",
            Self::ReceivedSubshellRcFileDcs => "Received Subshell RC File DCS",
            Self::ToggleSshWarpification => "Toggle SSH Warpification",
            Self::SetSshExtensionInstallMode => "Set SSH Extension Install Mode",
            Self::SshRemoteServerChoiceDoNotAskAgainToggled => {
                "SSH Remote Server Choice Do Not Ask Again Toggled"
            }
            Self::WarpifyFooterShown => "Warpify Footer Shown",
            Self::WarpifyFooterAcceptedWarpify => "Warpify Footer Accepted Warpify",
            Self::ShowAliasExpansionBanner => "Show Alias Expansion Banner",
            Self::DismissAliasExpansionBanner => "Dismiss Alias Expansion Banner",
            Self::EnableAliasExpansionFromBanner => "Enable Alias Expansion From Banner",
            Self::InitiateReauth => "Initiate Reauth",
            Self::NeedsReauth => "Needs Reauth",
            Self::WarpDriveOpened => "Warp Drive Opened",
            Self::ToggleSecretRedaction => "Toggle Secret Redaction",
            Self::CustomSecretRegexAdded => "Custom Secret Regex Added",
            Self::ToggleObfuscateSecret => "Toggle Obfuscate Secret",
            Self::CopySecret => "Copy Obfuscated Secret",
            Self::UpdateSortingChoice => "Updated Sorting Choice",
            Self::UndoClose => "Undo Close",
            Self::OpenPromptEditor => "Prompt Editor Opened",
            Self::PromptEdited => "Prompt Edited",
            Self::PtyThroughput => "PTY Throughput",
            Self::DuplicateObject => "Duplicate Object",
            Self::CommandFileRun => "Command File Run",
            Self::PageUpDownInEditorPressed => "Page Up/Down In Editor Pressed",
            Self::StartedSharingCurrentSession => "Started Sharing Current Session",
            Self::StoppedSharingCurrentSession => "Stopped Sharing Current Session",
            Self::JoinedSharedSession => "Joined Shared Session",
            Self::SharerCancelledGrantRole => "Sharer Cancelled Grant Role",
            Self::SharerGrantModalDontShowAgain => "Don't Show Sharer Grant Modal Again",
            Self::JumpToSharedSessionParticipant { .. } => "Jumped to Shared Session Participant",
            Self::CopiedSharedSessionLink { .. } => "Copied Shared Session Link",
            Self::WebSessionOpenedOnDesktop { .. } => "Web session opened on desktop",
            Self::WebCloudObjectOpenedOnDesktop { .. } => "Warp Drive object opened on desktop",
            Self::DriveSharingOnboardingBlockShown => "Warp Drive Sharing onboarding block shown",
            Self::UnsupportedShell => "Unsupported Shell",
            Self::SettingsImportInitiated => "Settings Import Initiated",
            Self::InviteTeammates => "Invited Teammates",
            Self::CopyObjectToClipboard => "Copy Object To Clipboard",
            Self::OpenAndWarpifyDockerSubshell => "OpenAndWarpifyDockerSubshell",
            Self::UpdateBlockFilterQuery => "Update Block Filter Query",
            Self::ToggleBlockFilterQuery => "Toggle Block Filter Query",
            Self::ToggleBlockFilterCaseSensitivity => "Toggle Block Filter Case Sensitivity",
            Self::ToggleBlockFilterRegex => "Toggle Block Filter Regex",
            Self::ToggleBlockFilterInvert => "Toggle Block Filter Invert",
            Self::BlockFilterToolbeltButtonClicked => "Block Filter Toolbelt Button Clicked",
            Self::ShowVimKeybindingsBanner => "Vim Keybindings Banner Displayed",
            Self::EnableVimKeybindingsFromBanner => "Vim Keybindings Enabled from Banner",
            Self::DismissVimKeybindingsBanner => "Vim Keybindings Banner Dismissed",
            Self::UpdateBlockFilterQueryContextLines => {
                "Update Block Filter Query With Context Lines"
            }
            Self::ToggleSnackbarInActivePane => "Toggle Sticky Command Header in Active Pane",
            Self::PaneDragInitiated => "Pane Drag Inititiated",
            Self::PaneDropped => "Pane Drag Ended",
            Self::TeamCreated => "Team Created",
            Self::TeamJoined => "Team Joined",
            Self::TeamLeft => "Team Left",
            Self::RemovedUserFromTeam => "Removed user from team",
            Self::DeletedNotebook => "Deleted Notebook",
            Self::ToggleApprovalsModal => "Toggle Approvals Modal",
            Self::TierLimitHit => "Tier Limit Hit",
            Self::ResourceUsageStats => "perf_metrics.resource_usage",
            Self::MemoryUsageStats => "perf_metrics.memory_usage",
            Self::MemoryUsageHigh => "perf_metrics.memory_usage_high",
            Self::TransientMemorySpike => "perf_metrics.transient_memory_spike",
            Self::EnvVarCollectionInvoked => "Invoked Environment Variables",
            Self::EnvVarWorkflowParameterization => {
                "Parameterized Workflow With Environment Variables"
            }
            Self::CompletedSettingsImport => "Completed Settings Import",
            Self::SettingsImportConfigFocused => "Focused Config in Settings Import",
            Self::SettingsImportResetButtonClicked => {
                "Clicked Reset to Defaults Button in Settings Import"
            }
            Self::ITermMultipleHotkeys => "ITerm Profile has Multiple Hotkeys",
            Self::ToggleWorkspaceDecorationVisibility => "Toggled Tab Bar Visibility",
            Self::UpdateAltScreenPaddingMode => "Updated Alt Screen Padding Mode",
            Self::AddTabWithShell => "Add Tab With Shell",
            Self::OpenedSharingDialog => "Opened Sharing Dialog",
            Self::ToggleLigatureRendering => "Toggle Ligature Rendering",

            Self::RemoteServerBinaryCheck => "RemoteServer.BinaryCheck",
            Self::RemoteServerInstallation => "RemoteServer.Installation",
            Self::RemoteServerInitialization => "RemoteServer.Initialization",
            Self::RemoteServerDaemonStartup => "RemoteServer.DaemonStartup",
            Self::RemoteServerDisconnection => "RemoteServer.Disconnection",
            Self::RemoteServerClientRequestError => "RemoteServer.ClientRequestError",
            Self::RemoteServerMessageDecodingError => "RemoteServer.MessageDecodingError",
            Self::RemoteServerSetupDuration => "RemoteServer.SetupDuration",
            Self::RemoteServerHostUnsupported => "RemoteServer.HostUnsupported",
            Self::RemoteServerReconnection => "RemoteServer.Reconnection",
            Self::RemoteServerReconnectExhausted => "RemoteServer.ReconnectExhausted",
            #[cfg(windows)]
            Self::WSLRegistryError => "WSL Distribution Registry Error",
            Self::ShellTerminatedPrematurely { .. } => "Shell Terminated Prematurely",
            Self::InputUXModeChanged { .. } => "Input.InputUXModeChanged",
            Self::UserMenuUpgradeClicked => "User Menu Upgrade Clicked",
            Self::TabCloseButtonPositionUpdated { .. } => "Update Tab Close Button Position",
            Self::RecentMenuItemSelected { .. } => "Recent Menu Item Selected",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::BlockCompleted => "Created Block",
            Self::BlockCompletedOnDogfoodOnly => {
                "Completed a block, with extra information for dogfood only"
            }
            Self::InitiateAnonymousUserSignup => "An anonymous user initiated the sign up flow",
            Self::AnonymousUserExpirationLockout => {
                "An anonymous user opened Warp after their conversion deadline and was locked out"
            }
            Self::AnonymousUserLinkedFromBrowser => {
                "Received an auth payload from anonymous user after linking in browser"
            }
            Self::AnonymousUserAttemptLoginGatedFeature => {
                "Anonymous user attempted to access a login-gated feature"
            }
            Self::AnonymousUserHitCloudObjectLimit => {
                "Anonymous user attempted to create a cloud object past their personal object limit"
            }
            Self::BackgroundBlockStarted => {
                "Warp created a background-output Block (whenever a processes has been backgrounded and yields some output)"
            }
            Self::SessionCreation => "Created a tab",
            #[cfg(feature = "local_fs")]
            Self::CodePaneOpened { .. } => "Opened the code editor pane from various sources",
            #[cfg(feature = "local_fs")]
            Self::CodePanelsFileOpened { .. } => {
                "Opened a file from code review, project explorer, or global search"
            }
            #[cfg(feature = "local_fs")]
            Self::PreviewPanePromoted => "Promoted a preview code tab to a normal tab",
            Self::ToggleSettingsSync => "Toggle Settings Sync",
            Self::Login => "Login is successful",
            Self::LoginLaterButtonClicked => "Clicked \"Login later\" button",
            Self::LoginLaterConfirmationButtonClicked => {
                "Clicked \"Yes, skip login\" confirmation button"
            }
            Self::ConfirmSuggestion => "Accepted tab completion suggestion",
            Self::ContextMenuCopy => "Clicked \"Copy\" in context menu",
            Self::ContextMenuFindWithinBlocks => "Clicked \"find within blocks\" in context menu",
            Self::ContextMenuCopyPrompt => "Clicked  \"Copy Prompt\" in context menu",
            Self::ContextMenuToggleGitPromptDirtyIndicator => {
                "Toggled indicator of dirty git prompt"
            }
            Self::ContextMenuInsertSelectedText => "Clicked \"insert into input\" in context menu",
            Self::OpenPromptEditor => "Opened the prompt editor",
            Self::PromptEdited => "Edited the prompt using the built-in prompt editor",
            Self::ReinputCommands => "Clicked \"reinput commands\" in context menu",
            Self::JumpToPreviousCommand => "Jumped to a previous command",
            Self::BlockSelection => "Selected Block",
            Self::BootstrappingSlow => "Slow bootstrap on session startup",
            Self::BootstrappingSlowContents => {
                "Contents of the bootstrap block if bootstrapping is slow"
            }
            Self::SessionAbandonedBeforeBootstrap => {
                "Abandoned session before the bootstrapping completes"
            }
            Self::BootstrappingSucceeded => "Successful bootstrap for session",
            Self::OpenThemeChooser => {
                "Opened theme chooser (list of different themes and visualizations of those themes)"
            }
            Self::ThemeSelection => "Selected theme",
            Self::AppIconSelection => "Selected app icon",
            Self::CursorDisplayType => "Selected cursor type",
            Self::OpenThemeCreatorModal => {
                "Opened theme creator modal (modal to create a new theme)"
            }
            Self::CreateCustomTheme => "Created a custom theme using the built-in theme creator",
            Self::DeleteCustomTheme => "Deleted a custom theme using the built-in theme creator",
            Self::SplitPane => "Split tab into multiple panes",
            Self::ToggleRestoreSession => {
                "Toggled session restoration (\"Restore windows, tabs, panes, on startup\")"
            }
            Self::DatabaseStartUpError => "Failed to initialize sqlite upon startup",
            Self::DatabaseReadError => {
                "Database read error when trying to get app state for session restoration"
            }
            Self::DatabaseWriteError => {
                "Database write error when trying to write app state for session restoration"
            }
            Self::AppStartup => "App is launched",
            Self::LoggedOutStartup => "Started Warp in the logged-out / signed-out state",
            Self::DownloadSource => {
                "Whether the Warp was installed from the home page or through homebrew"
            }
            Self::SSHBootstrapAttempt => "Attempted bootstrapping for an SSH session",
            Self::SSHControlMasterError => {
                "Encountered a ControlMaster error during an SSH session"
            }
            Self::KeybindingChanged => "Edited a custom keybinding",
            Self::KeybindingResetToDefault => "Reset a custom keybinding to its default",
            Self::KeybindingRemoved => "Removed / cleared a keybinding",
            Self::FeaturesPageAction => "Changed settings in Features Page",
            Self::OpenQuakeModeWindow => {
                "Toggled quake mode window when previously hidden or closed"
            }
            Self::OpenWelcomeTips => "Opened welcome tips in app",
            Self::DismissWelcomeTips => "Dismissed Welcome tips",
            Self::ShowNotificationsDiscoveryBanner => {
                "Showed notifications discovery banner in the block list"
            }
            Self::NotificationsDiscoveryBannerAction => {
                "Showed banner introducing the notifications feature"
            }
            Self::ShowNotificationsErrorBanner => "Showed error banner for notifications feature",
            Self::NotificationsErrorBannerAction => "Showed error banner for notifications feature",
            Self::NotificationPermissionsRequested => {
                "Requested permission for desktop notification permissions"
            }
            Self::NotificationsRequestPermissionsOutcome => {
                "Recorded outcome of attempting to request desktop notification permissions"
            }
            Self::NotificationSent => "Sent desktop notification",
            Self::NotificationFailedToSend => "Failed to send desktop notification",
            Self::NotificationClicked => "Clicked desktop notification sent from Warp",
            Self::ToggleFindOption => "Changed settings in Find Toggle",
            Self::SignUpButtonClicked => "Clicked \"Sign Up\" button",
            Self::LoginButtonClicked => "Clicked on \"Log in\" button",
            Self::OpenNewSessionFromFilePath => {
                "Dragged a file, folder, etc. into Warp to start a session"
            }
            Self::AuthCommonQuestionClicked => "Clicked on \"Common Question\" when logging in",
            Self::AuthToggleFAQ => "Toggled FAQ Page when logging in",
            Self::OpenAuthPrivacySettings => "Privacy settings are open during sign-in",
            Self::TabRenamed => "Changed tab title",
            Self::MoveActiveTab => "Move active tab left or right",
            Self::MoveTab => "Move tab left or right",
            Self::DragAndDropTab => "Tab dragged and dropped",
            Self::DragAndDropTabGroup => "Tab group dragged and dropped",
            Self::TabOperations => {
                "Took operation on a tab: change color, close tab, close adjacent tabs, etc."
            }
            Self::TriedToExecuteBeforePrecmd => {
                "Attempted to execute command before precmd, a shell stage that has metadata on a command such as ssh, prompt info, etc."
            }
            Self::ThinStrokesSettingChanged => {
                "Changed thin strokes setting in settings -> Appearance"
            }
            Self::BookmarkBlockToggled => "Bookmarked or unbookmarked Block",
            Self::JumpToBookmark => "Jumped to bookmarked Block",
            Self::JumpToBottomofBlockButtonClicked => {
                "Used the button to jump to the bottom of a Block"
            }
            Self::ToggleJumpToBottomofBlockButton => {
                "Enabled or disabled the Jump to Bottom of Block Button"
            }
            Self::ToggleShowBlockDividers => "Enabled or disabled the Show Block Dividers Button",
            Self::ShowInFileExplorer => "Opened a file in Finder by using \"Show in Finder\"",
            Self::OpenLaunchConfigSaveModal => "Opened save launch configuration modal",
            Self::SaveLaunchConfig => {
                "Saved current launch configuration of windows, tabs, and panes"
            }
            Self::OpenLaunchConfigFile => {
                "Opened the launch config YAML file from modal once saved successfully"
            }
            Self::OpenLaunchConfig => "Opened launch config for a session",
            Self::TeamCreated => "Created a Warp Drive team",
            Self::TeamJoined => "Joined a Warp Drive team",
            Self::TeamLeft => "Left a Warp Drive team",
            Self::RemovedUserFromTeam => "Remove user from Warp Drive team",
            Self::DeletedNotebook => "Deleted notebook from Warp Drive team",
            Self::ToggleApprovalsModal => "Opened or closed teams modal",
            Self::SetLineHeight => "Set line height through Settings -> Appearance",
            Self::ResourceCenterOpened => "Opened Resource Center pane",
            Self::ResourceCenterTipsCompleted => "Completed resource center tips",
            Self::ResourceCenterTipsSkipped => "Skipped welcome tips for new users",
            Self::KeybindingsPageOpened => "Opened the keybinding page within the resource center",
            Self::OpenNotebook => "Opened a notebook",
            Self::EditNotebook => "Edited a notebook",
            Self::NotebookAction => {
                "Took an action on a notebook: edit, delete, modified font size, etc."
            }
            Self::OpenedAltScreenFind => "Opened the Find bar in the Alt Screen",
            Self::UserInitiatedClose => "Attempted to either quit the app or close a window",
            Self::QuitModalShown => {
                "Showed an alert modal to warn the user about closing the app/window with a running process"
            }
            Self::QuitModalCancel => "`Cancel` button on the alert modal was pressed",
            Self::QuitModalDisabled => {
                "The quit modal dialog has been disabled and will not popup when a user closes Warp while a session is running"
            }
            Self::UserInitiatedLogOut => {
                "Confirms a user has explicitly logged out of the application"
            }
            Self::LogOutModalShown => "When the log out modal is displayed",
            Self::LogOutModalCancel => "Escaped the log out flow by canceling the log out modal",
            Self::SetOpacity => {
                "Changed the opacity (window transparency) from the `Settings -> Appearance` dialog"
            }
            Self::SetBlurRadius => {
                "Changed the blur radius from the `Settings -> Appearance` dialog"
            }
            Self::ToggleDimInactivePanes => {
                "Whether the dim inactive panes feature has been toggled"
            }
            Self::InputModeChanged => {
                "Changed the Input Editor Mode (Pinned to Bottom, Pinned to Top, Classic / Waterfall Mode)"
            }
            Self::PtySpawned => {
                "Tracks the manner by which we create a new shell process (new codepath vs. old codepath).  Used to ensure nothing breaks as we change parts of our infrastructure."
            }
            Self::InitialWorkingDirectoryConfigurationChanged => {
                "Replaced the default working directory with a different path"
            }
            Self::OpenInputContextMenu => "Opened the Input Editor's context menu",
            Self::InputCutSelectedText => {
                "Cut the highlighted text via the Input Editor's context menu (right clicking the buffer)"
            }
            Self::InputCopySelectedText => "Copied selected text from Input Editor",
            Self::InputSelectAll => {
                "Selected all the text in the Input Editor via its context menu (right clicking the buffer)"
            }
            Self::InputPaste => {
                "Pasted text into the Input Editor's via its context menu (right clicking the buffer)"
            }
            Self::InputCommandSearch => {
                "Opened Command Search via the Input Editor's context menu (right clicking the buffer)"
            }
            Self::ExperimentTriggered => "Client assigned to A/B test",
            Self::ToggleSyncAllPanesInAllTabs => {
                "Enable the synchronization of the Input Editor's buffer to all the panes in all the tabs"
            }
            Self::ToggleSyncAllPanesInTab => {
                "Enable the synchronization of the Input Editor's buffer to all the panes in the current tab"
            }
            Self::ToggleSameLinePrompt => "Toggled on/off same line prompt",
            Self::ToggleNewWindowsAtCustomSize => {
                "Whether the new windows at custom size feature has been toggled"
            }
            Self::ToggleFocusPaneOnHover => {
                "Toggled on/off focus pane on hover feature, which causes panes to automatically focus when hovering over them"
            }
            Self::SetNewWindowsAtCustomSize => {
                "Set new windows at custom size through Settings -> Appearance"
            }
            Self::DisableInputSync => {
                "Disabled / turn off the Input Synchronization (across editors)"
            }
            Self::ToggleTabIndicators => {
                "Enabled or disabled the tab indicators (failed command, etc.)"
            }
            Self::TogglePreserveActiveTabColor => {
                "Enabled or disabled preserving the active tab color"
            }
            Self::ShowSubshellBanner => {
                "Displayed the banner asking whether Warp should Warpify the current session via Warp's subshell wrapper"
            }
            Self::DeclineSubshellBootstrap => {
                "Developer declined the Warp banner to Warpify the current session"
            }
            Self::TriggerSubshellBootstrap => {
                "Attempted to Warpify the current session via Warp's subshell wrapper"
            }
            Self::AddDenylistedSubshellCommand => {
                "Explicitly prevent a command from being Warpified via Warp's subshell wrapper"
            }
            Self::RemoveDenylistedSubshellCommand => {
                "Removed a command from the list of commands to IGNORE when trying to Warpify via Warp's subshell wrapper"
            }
            Self::AddAddedSubshellCommand => {
                "Added a command to be automatically Warpified via Warp's subshell wrapper"
            }
            Self::RemoveAddedSubshellCommand => {
                "Removed a command from the list of commands to automatically Warpify via Warp's subshell wrapper"
            }
            Self::ReceivedSubshellRcFileDcs => "Spawned a subshell to be automatically Warpified",
            Self::ToggleSshWarpification => "Changed the setting for SSH sessions to be warified",
            Self::SetSshExtensionInstallMode => {
                "Changed the SSH extension install mode (always ask / always allow / always skip)"
            }
            Self::SshRemoteServerChoiceDoNotAskAgainToggled => {
                "Toggled the 'Don't ask me this again' checkbox on the SSH remote-server choice block"
            }
            Self::WarpifyFooterShown => {
                "Displayed the warpify footer for a detected subshell or SSH session"
            }
            Self::WarpifyFooterAcceptedWarpify => "User clicked Warpify in the warpify footer",
            Self::ShowAliasExpansionBanner => {
                "Displayed the banner asking whether Warp should automatically expand aliases within the Input Editor"
            }
            Self::EnableAliasExpansionFromBanner => {
                "Enabled automatic alias expansion within the Input Editor from the banner"
            }
            Self::DismissAliasExpansionBanner => {
                "Dismissed the banner to enable automatic alias expansion within the Input Editor"
            }
            Self::ShowVimKeybindingsBanner => {
                "Displayed the banner asking whether Warp should enable Vim keybindings in the Input Editor"
            }
            Self::EnableVimKeybindingsFromBanner => {
                "Enabled Vim keybindings in the Input Editor from the banner"
            }
            Self::DismissVimKeybindingsBanner => {
                "Dismissed the banner to enable Vim keybindings in the Input Editor"
            }
            Self::InitiateReauth => "Started the flow to re-authenticate the client",
            Self::NeedsReauth => "User needs to re-authenticate",
            Self::WarpDriveOpened => "Opened Warp Drive panel",
            Self::ToggleSecretRedaction => {
                "Toggled on/off the setting for Secret Redaction - attempts to redact secrets and sensitive information"
            }
            Self::CustomSecretRegexAdded => "Custom Secret Regex Added",
            Self::ToggleObfuscateSecret => "Revealed or hid a secret",
            Self::CopySecret => "Copied a secret's obfuscated contents to clipboard",
            Self::UpdateSortingChoice => "Modified the sorting scheme for Warp Drive objects",
            Self::UndoClose => "Re-opened a closed tab or window (undo closing a tab or window)",
            Self::PtyThroughput => "A sample of the max PTY throughput in bytes/sec",
            Self::DuplicateObject => "Cloned a Warp Drive object",
            Self::CommandFileRun => {
                "Opened a .cmd or unix executable file and ran it directly in Warp"
            }
            Self::PageUpDownInEditorPressed => {
                "Pressed `PAGE-UP` or `PAGE-DOWN` within the Input Editor"
            }
            Self::StartedSharingCurrentSession => "Started sharing the current session",
            Self::StoppedSharingCurrentSession => "Halted sharing the current session",
            Self::JoinedSharedSession => {
                "When you join another instance of Warp using shared sessions"
            }
            Self::SharerCancelledGrantRole => {
                "When you cancel granting a role to a shared session participant"
            }
            Self::SharerGrantModalDontShowAgain => {
                "When you check don't show again on the confirmation modal for granting a role"
            }
            Self::JumpToSharedSessionParticipant => {
                "Clicked on a shared session participant avatar to jump to their location in the session"
            }
            Self::CopiedSharedSessionLink => "Copied a shared session link",
            Self::WebSessionOpenedOnDesktop => {
                "Shared session viewed on the web was opened on the desktop"
            }
            Self::WebCloudObjectOpenedOnDesktop => {
                "Warp Drive object on the web was opened on the desktop"
            }
            Self::DriveSharingOnboardingBlockShown => {
                "Showed onboarding block for Warp Drive sharing"
            }
            Self::UnsupportedShell => "Booted Warp with a shell that isn't supported",
            Self::LogOut => "Logged out of the Warp client",
            Self::SettingsImportInitiated => "Started the import settings flow for new users",
            Self::InviteTeammates => "Sent emails to invite teammates to join Warp Drive team",
            Self::CopyObjectToClipboard => "Copied an object to the user's keyboard",
            Self::OpenAndWarpifyDockerSubshell => {
                "Warpifying a docker subshell from using the docker extension"
            }
            Self::UpdateBlockFilterQuery => "When a new filter is applied to a block",
            Self::UpdateBlockFilterQueryContextLines => {
                "When the number of context lines for a block filter query is updated"
            }
            Self::ToggleBlockFilterQuery => "Toggled on/off a block filter query",
            Self::ToggleBlockFilterCaseSensitivity => {
                "Toggled on/off case sensitivity within the block filter editor"
            }
            Self::ToggleBlockFilterRegex => "Toggled on/off regex within the block filter editor",
            Self::ToggleBlockFilterInvert => "Toggled on/off invert within the block filter editor",
            Self::BlockFilterToolbeltButtonClicked => {
                "Clicked the block filter icon in the top-right of a block"
            }
            Self::ToggleSnackbarInActivePane => {
                "Expanded or collapsed the sticky command header in the active pane"
            }
            Self::PaneDragInitiated => "Initiated dragging a pane via the header",
            Self::PaneDropped => "Ended dragging a pane via the pane header",
            Self::TierLimitHit => "User hit the tier limit for a feature",
            Self::ResourceUsageStats => "Periodic report on application resource usage statistics",
            Self::MemoryUsageStats => "Periodic report on application memory usage statistics",
            Self::MemoryUsageHigh => {
                "Total application memory usage exceeded a significant threshold"
            }
            Self::TransientMemorySpike => {
                "Application memory usage briefly crossed the excessive-usage threshold but \
                 dropped back under it before being reported as high"
            }
            Self::EnvVarCollectionInvoked => "Invoked an environment variables object",
            Self::EnvVarWorkflowParameterization => {
                "Selected from environment variables dropdown to parameterize workflow"
            }
            Self::ObjectLinkCopied => "The web link to an object has been copied.",
            Self::FileTreeToggled => "Opened the file tree/project explorer",
            Self::GlobalSearchOpened => "Opened the global search view",
            Self::GlobalSearchQueryStarted => "Started a global search (warp_ripgrep) search",
            Self::GlobalSearchQueryCompleted => {
                "Completed a global search across local and remote sources"
            }
            Self::FileTreeItemCreated => "Created a new file from the file tree",
            Self::CompletedSettingsImport => {
                "Imported a terminal's settings via the settings import onboarding block"
            }
            Self::SettingsImportConfigFocused => {
                "Selected a terminal in the settings import onboarding block"
            }
            Self::SettingsImportResetButtonClicked => {
                "Reset the imported settings in the settings import onboarding block"
            }
            Self::ITermMultipleHotkeys => {
                "Attempted to import an iTerm profile that contained multiple hotkey window bindings"
            }
            Self::ToggleWorkspaceDecorationVisibility => "Toggled when to display the tab bar",
            Self::UpdateAltScreenPaddingMode => {
                "Updated the custom padding setting for the alt-screen"
            }
            Self::AddTabWithShell => "Added a tab with specific shell",
            Self::OpenedSharingDialog => {
                "Opened the sharing settings dialog for a session or Warp Drive object"
            }
            Self::ToggleLigatureRendering => "Toggled ligature rendering",
            #[cfg(windows)]
            Self::WSLRegistryError => {
                "Encountered an error while fetching WSL distributions from the registry"
            }
            Self::ShellTerminatedPrematurely { .. } => "The shell process terminated prematurely",
            Self::InputUXModeChanged { .. } => "Changed the input UX mode",
            Self::UserMenuUpgradeClicked => "Clicked the 'Upgrade' menu item in the user menu",
            Self::TabCloseButtonPositionUpdated { .. } => "Updated the tab close button position",
            Self::RecentMenuItemSelected { .. } => {
                "User selected an item from the recents list on the new tab zero state"
            }
            Self::RemoteServerBinaryCheck => {
                "Remote server binary check completed (found, not found, or error)"
            }
            Self::RemoteServerInstallation => {
                "Remote server binary installation completed (success or failure)"
            }
            Self::RemoteServerInitialization => {
                "Remote server connection and initialization completed (success or failure)"
            }
            Self::RemoteServerDaemonStartup => {
                "Remote server daemon startup completed and socket bound"
            }
            Self::RemoteServerDisconnection => {
                "An established remote server connection was dropped"
            }
            Self::RemoteServerClientRequestError => "A client request to the remote server failed",
            Self::RemoteServerMessageDecodingError => {
                "A server message could not be decoded (no parseable request_id)"
            }
            Self::RemoteServerSetupDuration => {
                "End-to-end duration of the remote server setup flow"
            }
            Self::RemoteServerHostUnsupported => {
                "Preinstall check classified the remote host as unsupported, \
                 falling back to the wrapper-only SSH flow"
            }
            Self::RemoteServerReconnection => {
                "A reconnection attempt succeeded after a spontaneous disconnect"
            }
            Self::RemoteServerReconnectExhausted => {
                "All reconnection attempts were exhausted after a spontaneous disconnect"
            }
        }
    }
}

warp_core::register_telemetry_event!(TelemetryEvent);

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
