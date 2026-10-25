use std::fmt::Display;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use serde_with::SerializeDisplay;
use strum_macros::{EnumDiscriminants, EnumIter};
use warp_core::telemetry::{EnablementState, TelemetryEvent, TelemetryEventDesc};

use crate::code_review::diff_state::{BackendOrigin, DiffMode, DiffOperation};
use crate::features::FeatureFlag;
use crate::view_components::find::FindDirection;

/// Identifies which git button the user clicked in the code review header.
/// Each variant maps to one of the primary action button / dropdown items.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum GitButtonKind {
    #[serde(rename = "commit")]
    Commit,
    #[serde(rename = "push")]
    Push,
    #[serde(rename = "publish")]
    Publish,
    #[serde(rename = "create_pr")]
    CreatePr,
    #[serde(rename = "view_pr")]
    ViewPr,
}

/// Identifies which git operation actually ran when a `GitDialog` completed.
/// Distinguishes commit-dialog chained intents (e.g. commit-and-push) from
/// standalone push/publish/create-PR dialogs so analytics can tell the user
/// flows apart.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum GitOperationKind {
    /// Commit dialog with the commit-only intent.
    #[serde(rename = "commit_only")]
    CommitOnly,
    /// Commit dialog with the commit-and-push intent.
    #[serde(rename = "commit_and_push")]
    CommitAndPush,
    /// Commit dialog with the commit-and-create-PR intent.
    #[serde(rename = "commit_and_create_pr")]
    CommitAndCreatePr,
    /// Standalone push dialog.
    #[serde(rename = "push")]
    Push,
    /// Standalone publish dialog (push that also sets upstream).
    #[serde(rename = "publish")]
    Publish,
    /// Standalone create-PR dialog.
    #[serde(rename = "create_pr")]
    CreatePr,
}

/// Terminal status of a `GitDialog`. Captures both async-op outcomes and
/// pre-confirmation user cancels in a single enum.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum GitDialogStatus {
    /// User confirmed the dialog and the underlying git operation succeeded.
    #[serde(rename = "succeeded")]
    Succeeded,
    /// User confirmed the dialog and the underlying git operation failed.
    #[serde(rename = "failed")]
    Failed,
    /// User cancelled the dialog (ESC / close button / cancel button) before
    /// the async op ran.
    #[serde(rename = "cancelled")]
    Cancelled,
}

/// Entry points for opening the code review pane.
#[derive(Clone, Copy, Debug, SerializeDisplay, Default)]
pub enum CodeReviewPaneEntrypoint {
    /// Opened via the git diff chip.
    GitDiffChip,
    // Opened via the pane header
    PaneHeader,
    // Opened via the code mode v2 right panel button
    RightPanel,
    /// Opened via other means (unknown entry point).
    #[default]
    Other,
}

impl Display for CodeReviewPaneEntrypoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GitDiffChip => write!(f, "git_diff_chip"),
            Self::PaneHeader => write!(f, "pane_header"),
            Self::RightPanel => write!(f, "right_panel"),
            Self::Other => write!(f, "other"),
        }
    }
}

/// Pane state change for minimize/maximize events.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum PaneStateChange {
    /// Pane was minimized.
    #[serde(rename = "minimized")]
    Minimized,
    /// Pane was maximized.
    #[serde(rename = "maximized")]
    Maximized,
}

/// Telemetry events associated with the code review pane.
#[derive(Serialize, Debug, EnumDiscriminants)]
#[strum_discriminants(derive(EnumIter))]
#[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
pub enum CodeReviewTelemetryEvent {
    /// Emitted when the code review pane is opened.
    PaneOpened {
        is_local: Option<bool>,
        entrypoint: CodeReviewPaneEntrypoint,
        is_code_mode_v2: bool,
    },
    /// Emitted when a user clicks the revert hunk button.
    RevertHunkClicked { is_local: Option<bool> },
    /// Emitted when a file is saved in the code review pane.
    FileSaved { is_local: Option<bool> },
    /// Emitted when the code review pane is minimized or maximized.
    PaneStateChanged {
        is_local: Option<bool>,
        state_change: PaneStateChange,
    },
    /// Emitted when the diff base is changed (e.g., from uncommitted to main branch).
    BaseChanged {
        is_local: Option<bool>,
        /// The new diff mode.
        mode: DiffMode,
    },
    /// Failure when we are calculating the diff metadata.
    LoadMetadataFailed {
        backend_origin: BackendOrigin,
        mode: DiffMode,
        error: String,
    },
    /// Failure when we are loading the actual diff content. Shared across
    /// file-invalidation, full diff load, and remote diff paths; the
    /// `operation` field distinguishes which one produced the failure.
    LoadDiffFailed {
        backend_origin: BackendOrigin,
        operation: DiffOperation,
        mode: DiffMode,
        error: String,
        /// Time elapsed between when the tracked diff load was requested and
        /// when this failure was observed. `None` if no tracked load was in
        /// flight (e.g. a background invalidation error).
        load_duration: Option<Duration>,
    },
    /// Emitted when a full diff load completes successfully.
    DiffLoadCompleted {
        is_local: Option<bool>,
        mode: DiffMode,
        file_count: usize,
        files_changed: usize,
        total_additions: usize,
        total_deletions: usize,
        /// Time elapsed between when the diff load was requested and when the
        /// diffs were ready. `None` if the load was not initiated through a
        /// tracked entry point (e.g. background refresh).
        load_duration: Option<Duration>,
    },
    /// Emitted when the code review find bar is opened or closed.
    FindBarToggled {
        is_local: Option<bool>,
        /// Whether the find bar is now open.
        is_open: bool,
    },
    /// Emitted when search mode settings are changed.
    FindBarModeChanged {
        is_local: Option<bool>,
        /// Whether case-sensitive search is enabled.
        case_sensitive: bool,
        /// Whether regex search is enabled.
        regex: bool,
    },
    /// Emitted when the user navigates to the next or previous match.
    FindNavigated {
        is_local: Option<bool>,
        /// Direction of navigation.
        direction: FindDirection,
    },
    /// Emitted when the inline comment editor is opened in the code review pane.
    CommentEditorOpened { is_local: Option<bool> },
    /// Emitted when a new comment is added to the inline review.
    CommentAdded { is_local: Option<bool> },
    /// Emitted when an existing comment is edited.
    CommentEdited { is_local: Option<bool> },
    /// Emitted when a comment is deleted from the inline review.
    CommentDeleted { is_local: Option<bool> },
    /// Emitted when the bottom comment list panel is expanded.
    CommentListExpanded {
        is_local: Option<bool>,
        /// Number of comments currently in the list.
        comment_count: usize,
    },
    /// Emitted when a comment in the list view is clicked to jump to its location.
    CommentListItemClicked { is_local: Option<bool> },
    /// Emitted when one or more comments fail to be precisely relocated after code changes.
    CommentRelocationFailed {
        is_local: Option<bool>,
        /// Number of comments that could not be matched to an exact line and had to fall back.
        fallback_count: usize,
    },
    /// Emitted when a user clicks a git operation button in the code review
    /// header (primary button or dropdown item).
    GitButtonTriggered {
        is_local: Option<bool>,
        button: GitButtonKind,
    },
    /// Emitted when a git dialog reaches a terminal state — either the async
    /// op succeeded / failed, or the user cancelled before confirming.
    GitDialogCompleted {
        is_local: Option<bool>,
        /// The git operation that ran or would have run (e.g. `commit_and_push`
        /// for the commit dialog with that chained intent).
        operation: GitOperationKind,
        /// Whether the dialog succeeded, failed, or was cancelled.
        status: GitDialogStatus,
        /// Raw error string when `status == Failed`, `None` otherwise.
        error: Option<String>,
    },
}

impl TelemetryEvent for CodeReviewTelemetryEvent {
    fn name(&self) -> &'static str {
        CodeReviewTelemetryEventDiscriminants::from(self).name()
    }

    fn payload(&self) -> Option<serde_json::Value> {
        match self {
            CodeReviewTelemetryEvent::PaneOpened {
                is_local,
                entrypoint,
                is_code_mode_v2,
            } => Some(json!({
                "is_local": is_local,
                "entrypoint": entrypoint,
                "is_code_mode_v2": is_code_mode_v2,
            })),
            CodeReviewTelemetryEvent::RevertHunkClicked { is_local } => {
                Some(json!({ "is_local": is_local }))
            }
            CodeReviewTelemetryEvent::FileSaved { is_local } => {
                Some(json!({ "is_local": is_local }))
            }
            CodeReviewTelemetryEvent::PaneStateChanged {
                is_local,
                state_change,
            } => Some(json!({ "is_local": is_local, "state_change": state_change })),
            CodeReviewTelemetryEvent::BaseChanged { is_local, mode } => {
                Some(json!({ "is_local": is_local, "mode": mode }))
            }
            CodeReviewTelemetryEvent::LoadMetadataFailed {
                backend_origin,
                mode,
                error,
            } => Some(json!({
                "backend_origin": backend_origin,
                "mode": mode,
                "error": error,
            })),
            CodeReviewTelemetryEvent::LoadDiffFailed {
                backend_origin,
                operation,
                mode,
                error,
                load_duration,
            } => Some(json!({
                "backend_origin": backend_origin,
                "operation": operation,
                "mode": mode,
                "error": error,
                "load_duration": load_duration,
            })),
            CodeReviewTelemetryEvent::DiffLoadCompleted {
                is_local,
                mode,
                file_count,
                files_changed,
                total_additions,
                total_deletions,
                load_duration,
            } => Some(json!({
                "is_local": is_local,
                "mode": mode,
                "file_count": file_count,
                "files_changed": files_changed,
                "total_additions": total_additions,
                "total_deletions": total_deletions,
                "load_duration": load_duration,
            })),
            CodeReviewTelemetryEvent::FindBarToggled { is_local, is_open } => {
                Some(json!({ "is_local": is_local, "is_open": is_open }))
            }
            CodeReviewTelemetryEvent::FindBarModeChanged {
                is_local,
                case_sensitive,
                regex,
            } => Some(json!({
                "is_local": is_local,
                "case_sensitive": case_sensitive,
                "regex": regex,
            })),
            CodeReviewTelemetryEvent::FindNavigated {
                is_local,
                direction,
            } => Some(json!({ "is_local": is_local, "direction": direction })),
            CodeReviewTelemetryEvent::CommentEditorOpened { is_local } => {
                Some(json!({ "is_local": is_local }))
            }
            CodeReviewTelemetryEvent::CommentAdded { is_local } => {
                Some(json!({ "is_local": is_local }))
            }
            CodeReviewTelemetryEvent::CommentEdited { is_local } => {
                Some(json!({ "is_local": is_local }))
            }
            CodeReviewTelemetryEvent::CommentDeleted { is_local } => {
                Some(json!({ "is_local": is_local }))
            }
            CodeReviewTelemetryEvent::CommentListExpanded {
                is_local,
                comment_count,
            } => Some(json!({ "is_local": is_local, "comment_count": comment_count })),
            CodeReviewTelemetryEvent::CommentListItemClicked { is_local } => {
                Some(json!({ "is_local": is_local }))
            }
            CodeReviewTelemetryEvent::CommentRelocationFailed {
                is_local,
                fallback_count,
            } => Some(json!({ "is_local": is_local, "fallback_count": fallback_count })),
            CodeReviewTelemetryEvent::GitButtonTriggered { is_local, button } => {
                Some(json!({ "is_local": is_local, "button": button }))
            }
            CodeReviewTelemetryEvent::GitDialogCompleted {
                is_local,
                operation,
                status,
                error,
            } => Some(json!({
                "is_local": is_local,
                "operation": operation,
                "status": status,
                "error": error,
            })),
        }
    }

    fn description(&self) -> &'static str {
        CodeReviewTelemetryEventDiscriminants::from(self).description()
    }

    fn enablement_state(&self) -> EnablementState {
        CodeReviewTelemetryEventDiscriminants::from(self).enablement_state()
    }

    fn contains_ugc(&self) -> bool {
        CodeReviewTelemetryEventDiscriminants::from(self).contains_ugc()
    }

    fn event_descs() -> impl Iterator<Item = Box<dyn TelemetryEventDesc>> {
        warp_core::telemetry::enum_events::<Self>()
    }
}

impl CodeReviewTelemetryEventDiscriminants {
    pub fn contains_ugc(&self) -> bool {
        false
    }
}

impl TelemetryEventDesc for CodeReviewTelemetryEventDiscriminants {
    fn name(&self) -> &'static str {
        match self {
            Self::PaneOpened => "CodeReview.PaneOpened",
            Self::RevertHunkClicked => "CodeReview.RevertHunkClicked",
            Self::FileSaved => "CodeReview.FileSaved",
            Self::PaneStateChanged => "CodeReview.PaneStateChanged",
            Self::BaseChanged => "CodeReview.BaseChanged",
            Self::LoadMetadataFailed => "CodeReview.LoadMetadataFailed",
            Self::LoadDiffFailed => "CodeReview.LoadDiffFailed",
            Self::DiffLoadCompleted => "CodeReview.DiffLoadCompleted",
            Self::FindBarToggled => "CodeReview.FindBarToggled",
            Self::FindBarModeChanged => "CodeReview.FindBarModeChanged",
            Self::FindNavigated => "CodeReview.FindNavigated",
            Self::CommentEditorOpened => "CodeReview.CommentEditorOpened",
            Self::CommentAdded => "CodeReview.CommentAdded",
            Self::CommentEdited => "CodeReview.CommentEdited",
            Self::CommentDeleted => "CodeReview.CommentDeleted",
            Self::CommentListExpanded => "CodeReview.CommentListExpanded",
            Self::CommentListItemClicked => "CodeReview.CommentListItemClicked",
            Self::CommentRelocationFailed => "CodeReview.CommentRelocationFailed",
            Self::GitButtonTriggered => "CodeReview.GitButtonTriggered",
            Self::GitDialogCompleted => "CodeReview.GitDialogCompleted",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::PaneOpened => "Code review pane opened",
            Self::RevertHunkClicked => "Revert hunk button clicked",
            Self::FileSaved => "File saved in code review pane",
            Self::PaneStateChanged => "Code review pane minimized or maximized",
            Self::BaseChanged => "Diff base changed in code review",
            Self::LoadMetadataFailed => "Failure when calculating diff metadata",
            Self::LoadDiffFailed => "Failure when loading diff content",
            Self::DiffLoadCompleted => "Diff content loaded successfully",
            Self::FindBarToggled => "Code review find bar opened or closed",
            Self::FindBarModeChanged => "Search mode changed in code review find bar",
            Self::FindNavigated => "Navigated to next or previous match in code review find bar",
            Self::CommentEditorOpened => "Inline code review comment editor opened",
            Self::CommentAdded => "Inline code review comment added",
            Self::CommentEdited => "Inline code review comment edited",
            Self::CommentDeleted => "Inline code review comment deleted",
            Self::CommentListExpanded => "Inline code review comment list expanded",
            Self::CommentListItemClicked => "Inline code review comment list item clicked",
            Self::CommentRelocationFailed => {
                "Inline code review comment relocation fell back to approximate line"
            }
            Self::GitButtonTriggered => {
                "User clicked a git operation button in the code review header"
            }
            Self::GitDialogCompleted => {
                "Git operation dialog reached a terminal state (succeeded, failed, or cancelled)"
            }
        }
    }

    fn enablement_state(&self) -> EnablementState {
        match self {
            Self::GitButtonTriggered | Self::GitDialogCompleted => {
                EnablementState::Flag(FeatureFlag::GitOperationsInCodeReview)
            }
            _ => EnablementState::Always,
        }
    }
}

warp_core::register_telemetry_event!(CodeReviewTelemetryEvent);
