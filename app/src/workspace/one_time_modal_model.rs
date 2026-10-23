use std::future::Future;

use settings::Setting as _;
use warp_core::features::FeatureFlag;
use warp_util::sync::Condition;
use warpui::{Entity, ModelContext, SingletonEntity, WindowId};

use super::hoa_onboarding;
use crate::ai::blocklist::agent_view::toolbar_item::AgentToolbarItemKind;
use crate::auth::auth_manager::AuthManagerEvent;
use crate::auth::{AuthManager, AuthStateProvider};
use crate::channel::{Channel, ChannelState};
use crate::settings::cloud_preferences_syncer::{
    CloudPreferencesSyncer, CloudPreferencesSyncerEvent,
};
use crate::settings::{AISettings, CodeSettings};
use crate::terminal::session_settings::{AgentToolbarChipSelection, SessionSettings};

/// Tracks the HOA onboarding flow and auto-handoff sleep notice.
pub struct OneTimeModalModel {
    is_auto_handoff_sleep_modal_open: bool,
    /// Set while the sleep notice is closed so auto-resume can wait for dismissal.
    auto_handoff_sleep_modal_closed: Condition,
    is_hoa_onboarding_open: bool,
    /// The window where the active notice is displayed.
    target_window_id: Option<WindowId>,
}

impl OneTimeModalModel {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(&AuthManager::handle(ctx), |_, _, event, ctx| {
            let AuthManagerEvent::AuthComplete = event else {
                return;
            };
            let is_existing_user = AuthStateProvider::as_ref(ctx)
                .get()
                .is_onboarded()
                .unwrap_or_default();
            if is_existing_user {
                // Wait for synced onboarding markers before checking the existing-user flow.
                ctx.subscribe_to_model(
                    &CloudPreferencesSyncer::handle(ctx),
                    |me, _, event, ctx| {
                        if let CloudPreferencesSyncerEvent::InitialLoadCompleted = event {
                            ctx.unsubscribe_from_model(&CloudPreferencesSyncer::handle(ctx));
                            me.check_and_trigger_all_modals(ctx);
                            maybe_ensure_handoff_chip_in_toolbar(ctx);
                        }
                    },
                );
            } else {
                hoa_onboarding::mark_hoa_onboarding_completed(ctx);
            }
        });

        let auto_handoff_sleep_modal_closed = Condition::new();
        auto_handoff_sleep_modal_closed.set();
        Self {
            is_auto_handoff_sleep_modal_open: false,
            auto_handoff_sleep_modal_closed,
            is_hoa_onboarding_open: false,
            target_window_id: None,
        }
    }

    /// Returns the window ID where the currently open one-time modal should be displayed.
    pub fn target_window_id(&self) -> Option<WindowId> {
        self.target_window_id
    }

    /// Returns whether the auto-handoff sleep discoverability modal is currently open.
    pub fn is_auto_handoff_sleep_modal_open(&self) -> bool {
        self.is_auto_handoff_sleep_modal_open && self.target_window_id.is_some()
    }

    pub fn mark_auto_handoff_sleep_modal_dismissed(&mut self, ctx: &mut ModelContext<Self>) {
        self.set_auto_handoff_sleep_modal_open(false, ctx);
    }

    /// Triggers the auto-handoff sleep discoverability modal. This is not
    /// called on startup: the auto-handoff controller calls
    /// it on wake when a sleep interrupted an in-progress local agent run that
    /// would have been handed off had `auto_handoff_on_sleep_enabled` been on.
    /// Shows at most once per user (tracked by a synced private setting).
    /// Returns true when the modal was opened.
    pub fn check_and_trigger_auto_handoff_sleep_modal(
        &mut self,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let ai_settings = AISettings::as_ref(ctx);
        if *ai_settings.did_show_auto_handoff_sleep_modal {
            return false;
        }

        AISettings::handle(ctx).update(ctx, |settings, ctx| {
            if let Err(e) = settings
                .did_show_auto_handoff_sleep_modal
                .set_value(true, ctx)
            {
                log::warn!("Failed to mark auto-handoff sleep modal as shown: {e}");
            }
        });

        let should_show = !matches!(ChannelState::channel(), Channel::Integration);
        self.set_auto_handoff_sleep_modal_open(should_show, ctx);
        should_show
    }

    /// Sets whether the auto-handoff sleep modal is open. `pub(crate)` so the
    /// debug palette action can force the modal open.
    pub(crate) fn set_auto_handoff_sleep_modal_open(
        &mut self,
        is_open: bool,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        if self.is_auto_handoff_sleep_modal_open != is_open {
            self.is_auto_handoff_sleep_modal_open = is_open;
            if is_open {
                self.auto_handoff_sleep_modal_closed.reset();
            } else {
                self.auto_handoff_sleep_modal_closed.set();
            }
            ctx.emit(OneTimeModalEvent::VisibilityChanged { is_open });
            return true;
        }
        false
    }

    /// Returns a future that resolves immediately if the auto-handoff sleep
    /// modal is closed, or when it next closes if currently open. The future
    /// reads live modal state at poll time, so it can be created ahead of the
    /// modal opening.
    pub fn wait_until_auto_handoff_sleep_modal_closed(&self) -> impl Future<Output = ()> + use<> {
        self.auto_handoff_sleep_modal_closed.wait()
    }

    /// Returns whether the HOA onboarding flow is currently open.
    pub fn is_hoa_onboarding_open(&self) -> bool {
        self.is_hoa_onboarding_open && self.target_window_id.is_some()
    }

    pub fn mark_hoa_onboarding_dismissed(&mut self, ctx: &mut ModelContext<Self>) {
        self.set_hoa_onboarding_open(false, ctx);
    }

    /// Returns true if a remaining one-time notice is visible.
    pub fn is_any_modal_open(&self) -> bool {
        (self.is_auto_handoff_sleep_modal_open || self.is_hoa_onboarding_open)
            && self.target_window_id.is_some()
    }

    pub fn update_target_window_id(&mut self, window_id: WindowId, ctx: &mut ModelContext<Self>) {
        let was_visible = self.is_any_modal_open();
        self.target_window_id = Some(window_id);
        let is_visible = self.is_any_modal_open();
        if was_visible != is_visible {
            ctx.emit(OneTimeModalEvent::VisibilityChanged {
                is_open: is_visible,
            });
        }
    }

    fn check_and_trigger_all_modals(&mut self, ctx: &mut ModelContext<Self>) {
        // Never show one-time modals on WASM.
        if cfg!(target_family = "wasm") {
            return;
        }

        // Existing users should never see the code toolbelt new feature popup.
        CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
            if let Err(e) = settings
                .dismissed_code_toolbelt_new_feature_popup
                .set_value(true, ctx)
            {
                log::warn!("Failed to mark code toolbelt new feature popup as dismissed: {e}");
            }
        });

        self.check_and_trigger_hoa_onboarding(ctx);
    }

    fn set_hoa_onboarding_open(&mut self, is_open: bool, ctx: &mut ModelContext<Self>) -> bool {
        if self.is_hoa_onboarding_open != is_open {
            self.is_hoa_onboarding_open = is_open;
            ctx.emit(OneTimeModalEvent::VisibilityChanged { is_open });
            return true;
        }
        false
    }

    fn check_and_trigger_hoa_onboarding(&mut self, ctx: &mut ModelContext<Self>) -> bool {
        if !FeatureFlag::HOAOnboardingFlow.is_enabled() {
            return false;
        }

        if hoa_onboarding::has_completed_hoa_onboarding(ctx) {
            return false;
        }

        // All required dependent feature flags must be enabled.
        if !FeatureFlag::VerticalTabs.is_enabled()
            || !FeatureFlag::HOANotifications.is_enabled()
            || !FeatureFlag::TabConfigs.is_enabled()
        {
            return false;
        }

        self.set_hoa_onboarding_open(true, ctx)
    }
}

/// One-time migration: if the user has a custom agent toolbar layout that
/// predates the handoff-to-cloud chip, append the chip so they get the
/// new feature without losing their customization.
///
/// Users on `Default` already see the chip via `AgentToolbarItemKind::default_right()`.
fn maybe_ensure_handoff_chip_in_toolbar(ctx: &mut ModelContext<OneTimeModalModel>) {
    if !FeatureFlag::OzHandoff.is_enabled()
        || !FeatureFlag::HandoffLocalCloud.is_enabled()
        || !cfg!(all(feature = "local_fs", not(target_family = "wasm")))
    {
        return;
    }

    let session_settings = SessionSettings::as_ref(ctx);
    if *session_settings.did_add_handoff_chip_to_toolbar {
        return;
    }

    // Mark as done so future app starts skip this path.
    SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
        if let Err(e) = settings
            .did_add_handoff_chip_to_toolbar
            .set_value(true, ctx)
        {
            log::warn!("Failed to mark handoff chip toolbar migration as done: {e}");
        }
    });

    // `Default` already includes the chip — nothing to do.
    let selection = SessionSettings::as_ref(ctx)
        .agent_footer_chip_selection
        .clone();
    let AgentToolbarChipSelection::Custom { mut left, right } = selection else {
        return;
    };

    let handoff = AgentToolbarItemKind::HandoffToCloud;
    if left.contains(&handoff) || right.contains(&handoff) {
        return;
    }

    left.push(handoff);
    SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
        if let Err(e) = settings
            .agent_footer_chip_selection
            .set_value(AgentToolbarChipSelection::Custom { left, right }, ctx)
        {
            log::warn!("Failed to add handoff chip to toolbar: {e}");
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneTimeModalEvent {
    VisibilityChanged { is_open: bool },
}

impl Entity for OneTimeModalModel {
    type Event = OneTimeModalEvent;
}

impl SingletonEntity for OneTimeModalModel {}

#[cfg(test)]
#[path = "one_time_modal_model_tests.rs"]
mod tests;
