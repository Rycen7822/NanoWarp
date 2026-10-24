use serde::{Deserialize, Serialize};
use warpui::SingletonEntity;

use crate::context_chips::{ContextChipKind, agent_footer_available_chips};
use crate::features::FeatureFlag;
use crate::settings::CodeSettings;
use crate::terminal::shared_session::SharedSessionStatus;
use crate::ui_components::icons::Icon;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::workspaces::workspace::ChargeUnit;

/// A configurable item
///
/// This unifies context-chip data displays with interactive control buttons so
/// they can all be arranged through the same drag-and-drop editor.
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "An item that can appear in the agent toolbar.",
    rename_all = "snake_case"
)]
pub enum AgentToolbarItemKind {
    #[schemars(description = "A prompt context chip.")]
    ContextChip(ContextChipKind),
    // Agent view only
    NLDToggle,
    ContextWindowUsage,
    /// Trigger for the "Conversation" usage popover. Only offered to viewers whose tier
    /// charges usage in cents, since the popover's content is entirely usage figures.
    UsageSummary,

    FileExplorer,
    // Renamed from ImageAttach; alias preserves existing user toolbar configs.
    #[serde(alias = "ImageAttach")]
    FileAttach,
    ShareSession,

    // Agent view only – shows fast-forward (auto-approve) toggle in the footer
    FastForwardToggle,
}

impl AgentToolbarItemKind {
    /// Whether this item should be visible to session viewers.
    /// Items that control host settings or initiate actions on the host's
    /// behalf are hidden from viewers.
    pub fn available_to_session_viewer(&self, status: &SharedSessionStatus) -> bool {
        match self {
            Self::ShareSession | Self::FileExplorer => !status.is_viewer(),
            Self::FileAttach => !status.is_viewer(),
            Self::FastForwardToggle => !status.is_viewer() || status.is_executor(),
            Self::ContextChip(_)
            | Self::NLDToggle
            | Self::ContextWindowUsage
            | Self::UsageSummary => true,
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            Self::ContextChip(_) => "Context Chip",
            Self::NLDToggle => "Autodetection",
            Self::FileAttach => "Attach File",
            Self::ContextWindowUsage => "Context Usage",
            Self::UsageSummary => "Conversation Usage",
            Self::FileExplorer => "File Explorer",
            Self::ShareSession => "/remote-control",
            Self::FastForwardToggle => "Fast Forward",
        }
    }

    pub fn icon(&self) -> Option<Icon> {
        match self {
            Self::ContextChip(kind) => kind.udi_icon(),
            Self::NLDToggle => Some(Icon::NLD),
            Self::FileAttach => Some(Icon::Plus),
            Self::ContextWindowUsage => Some(Icon::ContextRemaining100),
            Self::UsageSummary => Some(Icon::PieChart),
            Self::FileExplorer => Some(Icon::FileCopy),
            Self::ShareSession => Some(Icon::Phone01),
            Self::FastForwardToggle => Some(Icon::FastForward),
        }
    }

    /// Whether this item should be included in the toolbar given the current app state.
    /// Feature-flag checks live in `all_available()` / `default_*()`. This method
    /// handles runtime conditions that depend on user settings or workspace state.
    pub fn is_available(&self, app: &warpui::AppContext) -> bool {
        match self {
            // Drops the item from the toolbar editor for tiers charged in credits. The render
            // path does not consult this method, so it repeats the check itself.
            Self::UsageSummary => match UserWorkspaces::as_ref(app).charge_unit() {
                ChargeUnit::Cents => true,
                ChargeUnit::Credits => false,
            },
            // Matches the gating on every other project explorer entry point, so the chip
            // cannot open a tool view the rest of the app hides. See
            // `Workspace::compute_left_panel_views` and the `SHOW_PROJECT_EXPLORER`
            // keybinding predicate.
            Self::FileExplorer => {
                cfg!(feature = "local_fs") && *CodeSettings::as_ref(app).show_project_explorer
            }
            _ => true,
        }
    }

    pub fn is_context_chip(&self) -> bool {
        matches!(self, Self::ContextChip(_))
    }

    pub fn context_chip_kind(&self) -> Option<&ContextChipKind> {
        match self {
            Self::ContextChip(kind) => Some(kind),
            _ => None,
        }
    }

    /// Default left-side items for the agent view footer.
    pub fn default_left() -> Vec<Self> {
        let mut items = vec![
            Self::ContextChip(ContextChipKind::Ssh),
            Self::ContextChip(ContextChipKind::WorkingDirectory),
            Self::ContextChip(ContextChipKind::ShellGitBranch),
            Self::ContextChip(ContextChipKind::GitDiffStats),
        ];
        if FeatureFlag::GithubPrPromptChip.is_enabled() {
            items.push(Self::ContextChip(ContextChipKind::GithubPullRequest));
        }
        items.push(Self::NLDToggle);
        items
    }

    /// Default right-side items for the agent view footer.
    pub fn default_right() -> Vec<Self> {
        // `UsageSummary` is listed unconditionally: `is_available` and the render path hide
        // it for tiers charged in credits, and this layout has no app context to consult.
        let mut items = vec![
            Self::ContextChip(ContextChipKind::AgentPlanAndTodoList),
            Self::ContextWindowUsage,
            Self::UsageSummary,
        ];
        if FeatureFlag::CreatingSharedSessions.is_enabled()
            && FeatureFlag::HOARemoteControl.is_enabled()
        {
            items.push(Self::ShareSession);
        }
        items.push(Self::FileAttach);
        items
    }

    /// All items available for the agent view footer configurator.
    pub fn all_available() -> Vec<Self> {
        let mut items: Vec<Self> = agent_footer_available_chips()
            .into_iter()
            .map(Self::ContextChip)
            .collect();
        items.extend([
            Self::NLDToggle,
            Self::FileAttach,
            Self::ContextWindowUsage,
            Self::UsageSummary,
            // Opt-in only: deliberately absent from `default_left`/`default_right`.
            Self::FileExplorer,
        ]);
        if FeatureFlag::FastForwardAutoexecuteButton.is_enabled() {
            items.push(Self::FastForwardToggle);
        }
        if FeatureFlag::CreatingSharedSessions.is_enabled()
            && FeatureFlag::HOARemoteControl.is_enabled()
        {
            items.push(Self::ShareSession);
        }
        items
    }
}

impl From<ContextChipKind> for AgentToolbarItemKind {
    fn from(kind: ContextChipKind) -> Self {
        Self::ContextChip(kind)
    }
}

#[cfg(test)]
#[path = "toolbar_item_tests.rs"]
mod tests;
