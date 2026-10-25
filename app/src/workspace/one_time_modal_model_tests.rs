use warp_core::features::FeatureFlag;
use warpui::{App, SingletonEntity};

use super::{AuthManager, AuthManagerEvent, AuthStateProvider, OneTimeModalModel, hoa_onboarding};
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

#[test]
fn hoa_onboarding_pre_dismissed_for_new_users_on_auth_complete() {
    App::test((), |mut app| async move {
        let _hoa_onboarding_flow = FeatureFlag::HOAOnboardingFlow.override_enabled(true);
        let _vertical_tabs = FeatureFlag::VerticalTabs.override_enabled(true);
        let _hoa_notifications = FeatureFlag::HOANotifications.override_enabled(true);
        let _tab_configs = FeatureFlag::TabConfigs.override_enabled(true);
        initialize_app_for_terminal_view(&mut app);
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
