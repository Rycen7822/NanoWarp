use session_sharing_protocol::sharer::SessionSourceType;
use std::cell::RefCell;
use std::rc::Rc;

use pathfinder_geometry::vector::vec2f;
use warpui::App;

use super::*;
use crate::context_chips::prompt_type::PromptType;
use crate::editor::InteractionState;
use crate::pane_group::{BackingView, PaneConfigurationEvent};
use crate::terminal::model::blocks::{INLINE_BANNER_HEIGHT, ToTotalIndex as _};
use crate::terminal::view::shared_session::test_utils::terminal_view_for_viewer;
use crate::test_util::add_window_with_terminal;
use crate::test_util::terminal::initialize_app_for_terminal_view;
#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
use crate::{FeatureFlag, assert_lines_approx_eq};

#[test]
fn test_prompt_context_menu_items_shared_session_viewer_no_edit_prompt() {
    App::test((), |mut app| async move {
        let terminal = terminal_view_for_viewer(&mut app);

        terminal.update(&mut app, |view, ctx| {
            let mut model = view.model.lock();
            view.current_prompt.update(ctx, |prompt, ctx| {
                model.set_shared_session_status(SharedSessionStatus::ActiveViewer {
                    role: Default::default(),
                });

                let PromptType::Dynamic { prompt } = prompt else {
                    return;
                };
                prompt.update(ctx, |prompt, ctx| {
                    prompt.update_context(model.block_list().active_block(), ctx)
                });
            })
        });

        let session_settings = SessionSettings::handle(&app);
        session_settings.update(&mut app, |settings, ctx| {
            let _ = settings.honor_ps1.set_value(false, ctx);
        });

        terminal.read(&app, |view, ctx| {
            let items: Vec<MenuItem<TerminalAction>> = view.prompt_context_menu_items(ctx);
            assert_eq!(items.len(), 3);

            // We expect the prompt menu items to be something like the following when no context chips exist:
            // Copy prompt
            // ------------
            // Edit prompt (disabled for shared-session viewers)
            assert_eq!(items[0].fields().unwrap().label(), "Copy prompt");
            assert!(items[1].is_separator());
            assert_eq!(items[2].fields().unwrap().label(), "Edit prompt");
            assert!(items[2].fields().unwrap().is_disabled());
        });
    })
}

#[test]
fn test_shared_session_banners() {
    let _flag = FeatureFlag::CreatingSharedSessions.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);

        let terminal = add_window_with_terminal(&mut app, None);
        let mut expected_block_heights_len = terminal.read(&app, |view, _| {
            assert!(matches!(
                view.inline_banners_state.shared_session_banner_state,
                SharedSessionBanners::None
            ));
            view.model.lock().block_list().block_heights().items().len()
        });

        // Make a block and then insert the shared session starter banner.
        terminal.update(&mut app, |view, ctx| {
            view.model.lock().simulate_block("ls", "foo");
            view.insert_shared_session_started_banner(
                SharedSessionScrollbackType::All,
                false,
                Local::now(),
                ctx,
            );
            expected_block_heights_len += 2;
        });

        terminal.read(&app, |view, _ctx| {
            let model = view.model.lock();

            // Make sure the state has changed.
            assert!(matches!(
                view.inline_banners_state.shared_session_banner_state,
                SharedSessionBanners::ActiveShare { .. }
            ));

            // We should have inserted a block and a banner.
            let block_height_items = model.block_list().block_heights().items();
            assert_eq!(block_height_items.len(), expected_block_heights_len);

            // The banner should have been inserted before the first visible block.
            let first_block_total_index = model
                .block_list()
                .first_non_hidden_block_by_index()
                .unwrap()
                .to_total_index(model.block_list());
            assert_lines_approx_eq!(
                block_height_items[first_block_total_index.0 - 1]
                    .height()
                    .into_lines(),
                INLINE_BANNER_HEIGHT
            );
        });

        // Insert another block and then the shared session ended banner.
        terminal.update(&mut app, |view, ctx| {
            view.model.lock().simulate_block("ls", "foo");
            view.insert_shared_session_ended_banner(ctx);
            expected_block_heights_len += 2;
        });

        terminal.read(&app, |view, _ctx| {
            let model = view.model.lock();

            // Make sure the state has changed.
            assert!(matches!(
                view.inline_banners_state.shared_session_banner_state,
                SharedSessionBanners::LastShared { .. }
            ));

            // by now, we've inserted two new blocks and two new banners since the initialization of the view.
            let block_height_items = model.block_list().block_heights().items();
            assert_eq!(block_height_items.len(), expected_block_heights_len);

            // The first banner should continue to be at the start of the blocklist.
            let first_block_total_index = model
                .block_list()
                .first_non_hidden_block_by_index()
                .unwrap()
                .to_total_index(model.block_list());
            assert_lines_approx_eq!(
                block_height_items[first_block_total_index.0 - 1]
                    .height()
                    .into_lines(),
                INLINE_BANNER_HEIGHT
            );

            // The second banner should be at the end of the blocklist, before the active block.
            let last_block_total_index = model
                .block_list()
                .last_non_hidden_block_by_index()
                .unwrap()
                .to_total_index(model.block_list());
            assert_lines_approx_eq!(
                block_height_items[last_block_total_index.0 + 1]
                    .height()
                    .into_lines(),
                INLINE_BANNER_HEIGHT
            );
        });

        // Mimic starting a shared session again in the same view.
        terminal.update(&mut app, |view, ctx| {
            view.insert_shared_session_started_banner(
                SharedSessionScrollbackType::None,
                false,
                Local::now(),
                ctx,
            );

            // We should have removed two banners and inserted one. So overall,
            // we lost one item in the blocklist since the last time.
            expected_block_heights_len -= 1;
        });

        terminal.read(&app, |view, _ctx| {
            let model = view.model.lock();

            // Make sure the state has changed.
            assert!(matches!(
                view.inline_banners_state.shared_session_banner_state,
                SharedSessionBanners::ActiveShare { .. }
            ));

            // We should have removed two banners and inserted one. So overall,
            // we lost one item in the blocklist since the last time.
            let block_height_items = model.block_list().block_heights().items();
            assert_eq!(block_height_items.len(), expected_block_heights_len);

            // The banner should have been inserted at the end of the blocklist, before the active block.
            let last_block_total_index = model
                .block_list()
                .last_non_hidden_block_by_index()
                .unwrap()
                .to_total_index(model.block_list());
            assert_lines_approx_eq!(
                block_height_items[last_block_total_index.0 + 1]
                    .height()
                    .into_lines(),
                INLINE_BANNER_HEIGHT
            );
        });
    })
}

#[test]
fn test_resize_shared_session_viewer_from_server() {
    let _flag = FeatureFlag::CreatingSharedSessions.override_enabled(true);

    App::test((), |mut app| async move {
        let terminal = terminal_view_for_viewer(&mut app);
        terminal.update(&mut app, |view, ctx| {
            // Refresh the size at the start of the test to make sure
            // we're using a consistent size throughout.
            view.refresh_size(ctx);
        });

        let model = terminal.read(&app, |view, _| view.model.clone());
        model
            .lock()
            .set_shared_session_status(SharedSessionStatus::ActiveViewer {
                role: Default::default(),
            });

        // The viewer's current size info.
        let original_size_info = *model.lock().block_list().size();
        let original_num_rows = original_size_info.rows();
        let original_num_cols = original_size_info.columns();

        // Case 1: suppose the sharer has a larger size.
        // The size info we expect is the old one with the greater
        // number of rows and columns (nothing else changed).
        let new_num_rows = original_num_rows + 1;
        let new_num_cols = original_num_cols + 1;
        let expected_size_info =
            original_size_info.with_rows_and_columns(new_num_rows, new_num_cols);

        terminal.update(&mut app, |view, ctx| {
            view.resize_from_sharer_update(
                WindowSize {
                    num_rows: new_num_rows,
                    num_cols: new_num_cols,
                },
                ctx,
            );
        });

        // Make sure the view and model reflect the new, expected size info.
        terminal.read(&app, |view, _ctx| {
            assert_eq!(*view.size_info(), expected_size_info);
            assert_eq!(*view.model.lock().block_list().size(), expected_size_info);
        });

        // Case 2: suppose the sharer has a smaller size.
        // The size info we expect is our old, larger one; nothing changed.
        let new_num_rows = original_num_rows - 1;
        let new_num_cols = original_num_cols - 1;
        let expected_size_info = original_size_info;

        terminal.update(&mut app, |view, ctx| {
            view.resize_from_sharer_update(
                WindowSize {
                    num_rows: new_num_rows,
                    num_cols: new_num_cols,
                },
                ctx,
            );
        });

        // Make sure the view and model reflect the old, expected size info.
        terminal.read(&app, |view, _ctx| {
            assert_eq!(*view.size_info(), expected_size_info);
            assert_eq!(*view.model.lock().block_list().size(), expected_size_info);
        });
    })
}

#[test]
fn test_resize_shared_session_viewer_independent_of_sharer() {
    let _create_flag = FeatureFlag::CreatingSharedSessions.override_enabled(true);
    let _view_flag = FeatureFlag::ViewingSharedSessions.override_enabled(true);

    App::test((), |mut app| async move {
        let terminal = terminal_view_for_viewer(&mut app);
        terminal.update(&mut app, |view, ctx| {
            // Refresh the size at the start of the test to make sure
            // we're using a consistent size throughout.
            view.after_terminal_view_layout(vec2f(100., 100.), ctx);

            // Set the sharer's size.
            let num_rows = view.size_info().rows();
            let num_cols = view.size_info().columns();
            view.resize_from_sharer_update(WindowSize { num_rows, num_cols }, ctx);
        });

        let original_size_info = terminal.read(&app, |view, _| *view.size_info());
        let original_num_rows = original_size_info.rows();
        let original_num_cols = original_size_info.columns();

        // Case 1: make the viewer winsize smaller by making the pane narrower.
        terminal.update(&mut app, |view, ctx| {
            let narrower = vec2f(
                original_size_info.pane_width_px().as_f32() - 10.,
                original_size_info.pane_height_px().as_f32(),
            );
            view.after_terminal_view_layout(narrower, ctx);
        });

        // Make sure the overall size info was changed but the rows, columns
        // were unchanged because we're respecting the sharer's larger size.
        terminal.read(&app, |view, _ctx| {
            let new_size_info = *view.size_info();
            assert_ne!(original_size_info, new_size_info);

            let expected_size_info =
                new_size_info.with_rows_and_columns(original_num_rows, original_num_cols);
            assert_eq!(*view.size_info(), expected_size_info);
            assert_eq!(*view.model.lock().block_list().size(), expected_size_info);
        });

        // Case 2: make the viewer winsize larger by making the pane wider.
        terminal.update(&mut app, |view, ctx| {
            let wider = vec2f(
                original_size_info.pane_width_px().as_f32() + 10.,
                original_size_info.pane_height_px().as_f32(),
            );
            view.after_terminal_view_layout(wider, ctx);
        });

        // Make sure the overall size info was changed, and that the rows, columns
        // were updated because we're respecting the viewer's larger size.
        terminal.read(&app, |view, _ctx| {
            let new_size_info = *view.size_info();
            assert_ne!(original_size_info, new_size_info);

            assert!(new_size_info.columns() > original_num_cols);
            assert!(view.model.lock().block_list().size().columns() > original_num_cols);
        });
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_on_session_share_ended_restores_size_after_viewer_driven_resize() {
    let _flag = FeatureFlag::CreatingSharedSessions.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |view, ctx| {
            // Refresh the size at the start of the test to make sure
            // we're using a consistent size throughout.
            view.after_terminal_view_layout(vec2f(100., 100.), ctx);
        });

        let original_size = terminal.read(&app, |view, _| *view.size_info());
        let viewer_rows = original_size.rows().saturating_sub(2).max(1);
        let viewer_cols = original_size.columns().saturating_sub(4).max(1);
        assert!(viewer_rows < original_size.rows() || viewer_cols < original_size.columns());

        // Resize the view as if a viewer with a smaller winsize has joined the session.
        terminal.update(&mut app, |view, ctx| {
            view.resize_from_viewer_report(
                WindowSize {
                    num_rows: viewer_rows,
                    num_cols: viewer_cols,
                },
                ctx,
            );
        });

        terminal.read(&app, |view, _| {
            assert_eq!(view.size_info().rows(), viewer_rows);
            assert_eq!(view.size_info().columns(), viewer_cols);
            assert_eq!(
                view.active_viewer_driven_size,
                Some((viewer_rows, viewer_cols))
            );
            assert_eq!(*view.model.lock().block_list().size(), *view.size_info());
        });

        // End the session, assert that the winsize was restored to the original.
        terminal.update(&mut app, |view, ctx| {
            view.on_session_share_ended(ctx);
        });

        terminal.read(&app, |view, _| {
            assert_eq!(view.size_info().rows(), original_size.rows());
            assert_eq!(view.size_info().columns(), original_size.columns());
            assert_eq!(view.active_viewer_driven_size, None);
            assert_eq!(*view.model.lock().block_list().size(), original_size);
        });
    })
}

#[test]
fn test_on_session_share_ended_restores_user_input() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let initial_block_height_items = terminal.read(&app, |view, _| {
            view.model.lock().block_list().block_heights().items().len()
        });

        terminal.update(&mut app, |view, ctx| {
            view.model
                .lock()
                .set_shared_session_source(SessionSourceType::User);

            view.on_session_share_ended(ctx);
        });

        terminal.read(&app, |view, ctx| {
            let model = view.model.lock();
            assert_eq!(
                model.block_list().block_heights().items().len(),
                initial_block_height_items + 1
            );
            assert!(view.is_input_box_visible(&model, ctx));
            assert_eq!(
                view.input()
                    .as_ref(ctx)
                    .editor()
                    .as_ref(ctx)
                    .interaction_state(ctx),
                InteractionState::Editable
            );
        });
    });
}

// APP-5027 regression: "Copy link" / "Copy session sharing link" must not silently do
// nothing when the Manager has no session id (e.g. during ViewPending / SharePending).

#[test]
fn test_copy_shared_session_link_does_not_write_clipboard_when_session_pending() {
    // copy_shared_session_link was a silent no-op when the Manager had no session_id
    // (e.g. ViewPending while the cloud agent environment is still setting up).
    // With the fix it shows an error toast AND does NOT write the join link to the clipboard.
    // This test asserts the new observable behavior (the toast), not just the clipboard-unchanged
    // invariant that also held on the old silent no-op path.
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(Manager::new);
        let toast_stack_handle = app.add_singleton_model(|_| crate::workspace::ToastStack);

        // Subscribe to ToastStack events so we can assert the error toast is emitted.
        let toast_text = Rc::new(RefCell::new(None::<String>));
        let toast_text_clone = toast_text.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(&toast_stack_handle, move |_, event, _| {
                if let crate::workspace::ToastStackEvent::AddEphemeralToast { toast, .. } = event {
                    *toast_text_clone.borrow_mut() = Some(toast.main_text().to_string());
                }
            });
        });

        let terminal = add_window_with_terminal(&mut app, None);
        let link_change_events = Rc::new(RefCell::new(0));
        let link_change_events_for_subscription = link_change_events.clone();
        let pane_configuration = terminal.read(&app, |view, _| view.pane_configuration().clone());
        app.update(|ctx| {
            ctx.subscribe_to_model(&pane_configuration, move |_, event, _| {
                if matches!(event, PaneConfigurationEvent::SharedSessionLinkChanged) {
                    *link_change_events_for_subscription.borrow_mut() += 1;
                }
            });
        });

        // Put the terminal in ViewPending state without registering a session_id with the Manager.
        // This simulates a cloud agent environment still setting up (no join yet).
        terminal.update(&mut app, |view, _| {
            view.model
                .lock()
                .set_shared_session_status(SharedSessionStatus::ViewPending);
        });

        // Write a sentinel to the clipboard so we can detect if it is overwritten.
        terminal.update(&mut app, |_, ctx| {
            ctx.clipboard()
                .write(warpui::clipboard::ClipboardContent::plain_text(
                    "sentinel".to_string(),
                ));
        });

        // Call copy_shared_session_link. With the fix, it shows an error toast and returns early.
        terminal.update(&mut app, |view, ctx| {
            view.copy_shared_session_link(SharedSessionActionSource::RightClickMenu, ctx);
        });

        // Assert the error toast was shown — this is the new, observable behavior that proves
        // the fix is active. Without the fix, no toast would be emitted.
        assert_eq!(
            toast_text.borrow().as_deref(),
            Some("Sharing link not yet available"),
            "copy_shared_session_link must show an error toast when no session_id is registered"
        );

        // Belt-and-suspenders: clipboard must also remain unchanged.
        let clipboard_text = terminal.update(&mut app, |_, ctx| ctx.clipboard().read().plain_text);
        assert_eq!(
            clipboard_text, "sentinel",
            "copy_shared_session_link must not write the join link when no session_id is registered"
        );

        // A previous ended session id must not become copyable again while a new share attempt is
        // pending on the same terminal.
        terminal.update(&mut app, |_, ctx| {
            let window_id = ctx.window_id();
            Manager::handle(ctx).update(ctx, |manager, ctx| {
                manager.started_share(terminal.downgrade(), SessionId::new(), window_id, ctx);
                manager.stopped_share(terminal.id(), ctx);
            });
        });
        *toast_text.borrow_mut() = None;

        terminal.update(&mut app, |view, ctx| {
            view.attempt_to_share_session(
                SharedSessionScrollbackType::None,
                None,
                SessionSourceType::User,
                ctx,
            );
        });
        assert_eq!(
            *link_change_events.borrow(),
            1,
            "starting a new share must refresh cached link and QR surfaces"
        );

        terminal.update(&mut app, |view, ctx| {
            view.copy_shared_session_link(SharedSessionActionSource::RightClickMenu, ctx);
        });

        assert_eq!(
            toast_text.borrow().as_deref(),
            Some("Sharing link not yet available"),
            "a retained ended id must not be copied while a new session is pending"
        );
        let clipboard_text = terminal.update(&mut app, |_, ctx| ctx.clipboard().read().plain_text);
        assert_eq!(
            clipboard_text, "sentinel",
            "a retained ended id must not overwrite the clipboard during a pending retry"
        );
    });
}

#[test]
fn test_pane_header_copy_link_disabled_when_view_pending_no_session_id() {
    // APP-5027 call-site regression: the pane-header "Copy link" item must be disabled
    // when the terminal is in ViewPending state and Manager has no session_id for this view.
    // This exercises the actual has_session_link call-site computation inside
    // pane_header_overflow_menu_items, not just the session_sharing_context_menu_items helper.
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(Manager::new);

        let terminal = add_window_with_terminal(&mut app, None);

        // ViewPending simulates a cloud-agent viewer mid-setup: the session exists in the model
        // but Manager has not yet received a session_id for this view.
        terminal.update(&mut app, |view, _| {
            view.model
                .lock()
                .set_shared_session_status(SharedSessionStatus::ViewPending);
        });

        terminal.read(&app, |view, ctx| {
            let items = view.pane_header_overflow_menu_items(ctx);

            let copy_link_item = items
                .iter()
                .find(|item| item.fields().is_some_and(|f| f.label() == "Copy link"));
            assert!(
                copy_link_item.is_some(),
                "Copy link item should appear when terminal is in ViewPending state"
            );
            assert!(
                copy_link_item.unwrap().fields().unwrap().is_disabled(),
                "Copy link must be disabled when Manager has no session_id (ViewPending setup)"
            );
        });
    });
}

#[test]
fn test_session_sharing_context_menu_copy_link_disabled_when_no_session_link() {
    // The "Copy session sharing link" context-menu item must be disabled (greyed out)
    // when the session link is not yet available (has_session_link=false).
    App::test((), |mut app| async move {
        let terminal = terminal_view_for_viewer(&mut app);

        terminal.read(&app, |view, _| {
            let model = view.model.lock();
            // has_session_link=false simulates ViewPending with no registered session_id.
            let items = view.session_sharing_context_menu_items(&model, false, false);

            let copy_link_item = items.iter().find(|item| {
                item.fields()
                    .is_some_and(|f| f.label() == "Copy session sharing link")
            });
            assert!(
                copy_link_item.is_some(),
                "Copy session sharing link item should be present when is_sharer_or_viewer"
            );
            assert!(
                copy_link_item.unwrap().fields().unwrap().is_disabled(),
                "Copy session sharing link must be disabled when no session link is available"
            );
        });
    });
}

#[test]
fn test_session_sharing_context_menu_copy_link_enabled_when_session_link_available() {
    // The "Copy session sharing link" item must be enabled when the session link is available.
    App::test((), |mut app| async move {
        let terminal = terminal_view_for_viewer(&mut app);

        terminal.read(&app, |view, _| {
            let model = view.model.lock();
            // has_session_link=true simulates an active or ended session with a registered id.
            let items = view.session_sharing_context_menu_items(&model, false, true);

            let copy_link_item = items.iter().find(|item| {
                item.fields()
                    .is_some_and(|f| f.label() == "Copy session sharing link")
            });
            assert!(
                copy_link_item.is_some(),
                "Copy session sharing link item should be present when is_sharer_or_viewer"
            );
            assert!(
                !copy_link_item.unwrap().fields().unwrap().is_disabled(),
                "Copy session sharing link must be enabled when session link is available"
            );
        });
    });
}
