use std::path::PathBuf;

use repo_metadata::repositories::DetectedRepositories;
use warp_core::ui::appearance::Appearance;
use warpui::elements::Empty;
use warpui::platform::WindowStyle;
use warpui::{App, ViewHandle};

use super::*;
use crate::code::buffer_location::LocalOrRemotePath;
use crate::code::persisted_workspace::PersistedWorkspace;
use crate::code_review::GlobalCodeReviewModel;
use crate::code_review::diff_state::DiffStateModel;
use crate::code_review::git_repo_model::GitRepoModels;
use crate::settings_view::keybindings::KeybindingChangedNotifier;
use crate::terminal::local_shell::LocalShellState;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::vim_registers::VimRegisters;
use crate::workspace::ActiveSession;
use crate::workspace::sync_inputs::SyncedInputState;

#[derive(Default)]
struct TestView;

impl warpui::Entity for TestView {
    type Event = ();
}

impl warpui::View for TestView {
    fn render(&self, _: &warpui::AppContext) -> Box<dyn warpui::Element> {
        Empty::new().finish()
    }

    fn ui_name() -> &'static str {
        "TestView"
    }
}

impl warpui::TypedActionView for TestView {
    type Action = ();
}

/// Initialize required singletons for testing
fn initialize_test_app(app: &mut App) {
    initialize_settings_for_tests(app);
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(|_| SyncedInputState::mock());
    app.add_singleton_model(|_| VimRegisters::new());
    app.add_singleton_model(|_| KeybindingChangedNotifier::mock());
    app.add_singleton_model(|_| DetectedRepositories::default());
    app.add_singleton_model(|_| GitRepoModels::new());
    app.add_singleton_model(|_| LocalShellState::NotLoaded);
    app.add_singleton_model(PersistedWorkspace::new_for_test);
    app.add_singleton_model(|_| GlobalCodeReviewModel);

    app.add_singleton_model(|_| ActiveSession::default());
}

/// Test context that holds all common test state
struct TestContext {
    repo_path: PathBuf,
    code_review_view: ViewHandle<CodeReviewView>,
}

impl TestContext {
    /// Initialize the code review view for selector and lifecycle tests
    fn new(app: &mut App) -> Self {
        initialize_test_app(app);

        let diff_state_model = app.add_model(DiffStateModel::new_for_test);

        let repo_path = PathBuf::from("/repo");
        let (window_id, _) = app.add_window(WindowStyle::NotStealFocus, |_| TestView);
        let repo_key = LocalOrRemotePath::Local(repo_path.clone());
        let code_review_view = app.add_view(window_id, |ctx| {
            CodeReviewView::new(Some(repo_key.clone()), diff_state_model, None, ctx)
        });

        Self {
            repo_path,
            code_review_view,
        }
    }
}

#[test]
fn test_setup_dropdown_with_branches_includes_all_items() {
    App::test((), |mut app| async move {
        let ctx = TestContext::new(&mut app);

        // Populate branches and compute targets via the selector's build method.
        let target_count = ctx.code_review_view.update(&mut app, |view, view_ctx| {
            if let Some(repo) = view.active_repo.as_mut() {
                repo.available_branches = vec![
                    BranchEntry {
                        name: "main".to_string(),
                        is_main: true,
                    },
                    BranchEntry {
                        name: "feature-1".to_string(),
                        is_main: false,
                    },
                    BranchEntry {
                        name: "feature-2".to_string(),
                        is_main: false,
                    },
                ];
            }
            view.build_diff_targets(view_ctx).len()
        });

        // Verify the selector surfaces all expected items:
        // 1. "Uncommitted changes" (always first)
        // 2. "main" (main branch)
        // 3. "feature-1"
        // 4. "feature-2"
        assert_eq!(
            target_count, 4,
            "Diff selector should have 4 targets: Uncommitted changes + main + 2 feature branches"
        );
    });
}

#[test]
fn test_setup_dropdown_without_branches_only_has_uncommitted_changes() {
    App::test((), |mut app| async move {
        let ctx = TestContext::new(&mut app);

        // Ensure branches are empty (simulates the bug state) and count targets.
        let target_count = ctx.code_review_view.update(&mut app, |view, view_ctx| {
            if let Some(repo) = view.active_repo.as_mut() {
                repo.available_branches = vec![];
            }
            view.build_diff_targets(view_ctx).len()
        });

        assert_eq!(
            target_count, 1,
            "Diff selector should only have 'Uncommitted changes' when no branches are available"
        );
    });
}

#[test]
fn test_duplicate_single_file_discard_confirmation_is_ignored() {
    App::test((), |mut app| async move {
        let ctx = TestContext::new(&mut app);

        ctx.code_review_view.update(&mut app, |view, view_ctx| {
            view.discard_dialog_state.operation_type = DiscardOperationType::FileUncommittedChanges;
            view.discard_dialog_state.show_discard_confirm_dialog = false;
            view.discard_dialog_state.discard_file_paths.clear();

            view.handle_action(&CodeReviewAction::ConfirmDiscardFile, view_ctx);

            assert!(!view.discard_dialog_state.show_discard_confirm_dialog);
            assert!(view.discard_dialog_state.discard_file_paths.is_empty());
        });
    });
}

#[test]
fn test_on_close_then_on_open_reinitializes_repo_state() {
    App::test((), |mut app| async move {
        let ctx = TestContext::new(&mut app);
        let repo_path = ctx.repo_path.clone();

        // Populate branches to simulate a working state
        let target_count_before = ctx.code_review_view.update(&mut app, |view, view_ctx| {
            if let Some(repo) = view.active_repo.as_mut() {
                repo.available_branches = vec![
                    BranchEntry {
                        name: "main".to_string(),
                        is_main: true,
                    },
                    BranchEntry {
                        name: "feature-1".to_string(),
                        is_main: false,
                    },
                ];
            }
            view.build_diff_targets(view_ctx).len()
        });
        assert_eq!(target_count_before, 3, "Should have 3 targets before close");

        // Close the view
        ctx.code_review_view.update(&mut app, |view, view_ctx| {
            view.on_close(view_ctx);
            assert!(!view.is_open, "View should be closed after on_close");
        });

        // Re-open the view
        ctx.code_review_view.update(&mut app, |view, view_ctx| {
            view.on_open(view_ctx);

            assert!(view.is_open, "View should be open after on_open");
            assert_eq!(
                view.repo_path().and_then(LocalOrRemotePath::to_local_path),
                Some(repo_path.as_path()),
                "Repo path should be preserved after on_open (set at construction)"
            );
        });
    });
}
