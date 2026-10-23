use futures::FutureExt;
use warp_core::features::FeatureFlag;
use warpui::{App, SingletonEntity};

use super::{
    AuthManager, AuthManagerEvent, AuthStateProvider, CloudPreferencesSyncer, OneTimeModalModel,
    hoa_onboarding,
};
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

#[test]
fn wait_until_auto_handoff_sleep_modal_closed_tracks_modal_state() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |_, ctx| {
            OneTimeModalModel::handle(ctx).update(ctx, |model, ctx| {
                // Resolves immediately while the modal is closed.
                assert!(
                    model
                        .wait_until_auto_handoff_sleep_modal_closed()
                        .now_or_never()
                        .is_some()
                );

                // The auto-resume path creates its wait future before the
                // modal opens (e.g. while offline during sleep); it must
                // still observe the modal that opens later.
                let pending_probe = model.wait_until_auto_handoff_sleep_modal_closed();
                let resolving_waiter = model.wait_until_auto_handoff_sleep_modal_closed();

                model.set_auto_handoff_sleep_modal_open(true, ctx);

                // Pending while the modal is open, because the future reads
                // live modal state at poll time.
                assert!(pending_probe.now_or_never().is_none());

                model.mark_auto_handoff_sleep_modal_dismissed(ctx);

                // An existing waiter resolves once the modal closes.
                assert!(resolving_waiter.now_or_never().is_some());
            });
        });
    });
}

#[test]
fn hoa_onboarding_pre_dismissed_for_new_users_on_auth_complete() {
    App::test((), |mut app| async move {
        let _hoa_onboarding_flow = FeatureFlag::HOAOnboardingFlow.override_enabled(true);
        let _vertical_tabs = FeatureFlag::VerticalTabs.override_enabled(true);
        let _hoa_notifications = FeatureFlag::HOANotifications.override_enabled(true);
        let _tab_configs = FeatureFlag::TabConfigs.override_enabled(true);
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(|ctx| {
            CloudPreferencesSyncer::new(false, std::path::PathBuf::new(), ctx)
        });
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |_, ctx| {
            let _model = OneTimeModalModel::handle(ctx);

            AuthStateProvider::as_ref(ctx).get().set_is_onboarded(false);
            assert!(!hoa_onboarding::has_completed_hoa_onboarding(ctx));

            AuthManager::handle(ctx).update(ctx, |_, ctx| {
                ctx.emit(AuthManagerEvent::AuthComplete);
            });
        });
        terminal.update(&mut app, |_, ctx| {
            assert!(hoa_onboarding::has_completed_hoa_onboarding(ctx));
            OneTimeModalModel::handle(ctx).update(ctx, |model, ctx| {
                assert!(!model.check_and_trigger_hoa_onboarding(ctx));
            });
        });
    });
}

#[test]
fn hoa_onboarding_not_pre_dismissed_for_existing_users_on_auth_complete() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(|ctx| {
            CloudPreferencesSyncer::new(false, std::path::PathBuf::new(), ctx)
        });
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |_, ctx| {
            let _model = OneTimeModalModel::handle(ctx);

            AuthStateProvider::as_ref(ctx).get().set_is_onboarded(true);
            assert!(!hoa_onboarding::has_completed_hoa_onboarding(ctx));

            AuthManager::handle(ctx).update(ctx, |_, ctx| {
                ctx.emit(AuthManagerEvent::AuthComplete);
            });
        });

        app.read(|ctx| {
            assert!(!hoa_onboarding::has_completed_hoa_onboarding(ctx));
        });
    });
}
