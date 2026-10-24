pub(super) mod chips;
pub mod editor;
pub mod toolbar_item;

use std::sync::Arc;

use chrono::{DateTime, Local};
use parking_lot::FairMutex;
use pathfinder_color::ColorU;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use settings::ToggleableSetting;
use toolbar_item::AgentToolbarItemKind;
use warp_core::context_flag::ContextFlag;
use warp_core::ui::color::blend::Blend;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::{AnsiColorIdentifier, Fill};
use warp_errors::report_if_error;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::elements::{
    ChildAnchor, ChildView, Clipped, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    DispatchEventResult, Element, Empty, EventHandler, Flex, MainAxisAlignment, MainAxisSize,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Radius, SavePosition,
    Shrinkable, Stack, Wrap, WrapFill,
};
use warpui::{
    AppContext, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::ai::AIRequestUsageModel;
use crate::ai::blocklist::history_model::{BlocklistAIHistoryEvent, BlocklistAIHistoryModel};
use crate::ai::blocklist::prompt::prompt_alert::{PromptAlertEvent, PromptAlertView};
use crate::ai::blocklist::usage::icon_for_context_window_usage;
use crate::ai::blocklist::usage::usage_popover_view::{
    UsagePopoverEvent, UsagePopoverView, conversation_total_text, conversation_usage_display_unit,
};
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::appearance::Appearance;
use crate::auth::{AuthManager, AuthStateProvider};
use crate::completer::SessionContext;
use crate::context_chips;
use crate::context_chips::display_chip::{DisplayChip, DisplayChipConfig, PromptChipShellCommand};
use crate::context_chips::prompt_type::PromptType;
use crate::features::FeatureFlag;
use crate::network::NetworkStatus;
use crate::settings::{
    AISettings, AISettingsChangedEvent, CodeSettings, CodeSettingsChangedEvent, PrivacySettings,
    PrivacySettingsChangedEvent,
};
use crate::settings_view::SettingsSection;
use crate::terminal::TerminalModel;
use crate::terminal::input::{MenuPositioning, MenuPositioningProvider};
use crate::terminal::session_settings::{
    SessionSettings, SessionSettingsChangedEvent, ToolbarChipSelection,
};
use crate::terminal::shared_session::SharedSessionStatus;
use crate::terminal::view::TerminalAction;
use crate::terminal::view::init::ATTACH_FILE_KEYBINDING;
use crate::ui_components::icons::Icon;
use crate::view_components::action_button::{
    ActionButton, ActionButtonTheme, ButtonSize, KeystrokeSource, TooltipAlignment,
};
use crate::workspace::view::TOGGLE_PROJECT_EXPLORER_BINDING_NAME;
use crate::workspaces::user_workspaces::{UserWorkspaces, UserWorkspacesEvent};
use crate::workspaces::workspace::ChargeUnit;

const ENABLE_NLD_TOOLTIP: &str = "Enable terminal command autodetection";
const DISABLE_NLD_TOOLTIP: &str = "Disable terminal command autodetection";

const FAST_FORWARD_ON_TOOLTIP: &str = "Turn off auto-approve all agent actions";
const FAST_FORWARD_OFF_TOOLTIP: &str = "Auto-approve all agent actions for this task";

const START_REMOTE_CONTROL_TOOLTIP: &str = "Start remote control";
const START_REMOTE_CONTROL_LOGIN_REQUIRED_TOOLTIP: &str = "Log in to use /remote-control";

/// id for the conversation usage popover's trigger button to anchor the popover overlay
const USAGE_BUTTON_SAVE_POSITION_ID: &str = "agent_input_footer::usage_button";

fn is_conversation_transcript_context(
    terminal_view_id: EntityId,
    terminal_model: &TerminalModel,
    app: &AppContext,
) -> bool {
    terminal_model.is_conversation_transcript_viewer()
        || BlocklistAIHistoryModel::as_ref(app)
            .active_conversation(terminal_view_id)
            .is_some_and(|conversation| conversation.is_viewing_shared_session())
}

/// Footer control bar at the bottom of the agent input.
pub struct AgentInputFooter {
    terminal_view_id: EntityId,
    nld_button: ViewHandle<ActionButton>,
    file_button: ViewHandle<ActionButton>,
    start_remote_control_button: ViewHandle<ActionButton>,
    stop_remote_control_button: ViewHandle<ActionButton>,
    context_window_button: ViewHandle<ActionButton>,
    usage_button: ViewHandle<ActionButton>,
    prompt_alert: ViewHandle<PromptAlertView>,
    left_display_chips: Vec<ViewHandle<DisplayChip>>,
    right_display_chips: Vec<ViewHandle<DisplayChip>>,
    display_chip_config: DisplayChipConfig,

    terminal_model: Arc<FairMutex<TerminalModel>>,

    /// Opens the file explorer side panel.
    file_explorer_button: ViewHandle<ActionButton>,

    // Fast-forward (auto-approve) toggle button shown in the agent view footer.
    fast_forward_button: ViewHandle<ActionButton>,

    /// Pending one-shot timer that refreshes the context-window button at the
    /// prompt-cache expiry instant so the notification dot appears while idle.
    prompt_cache_expiry_timer_handle: Option<SpawnedFutureHandle>,

    /// Whether the active conversation's prompt cache has expired. Drives the
    /// yellow notification dot on the context-window chip when the
    /// `PromptCacheExpiryWarning` flag is enabled.
    prompt_cache_expired: bool,

    /// Used to anchor the usage popover above or below the input box, matching the
    /// surrounding menu positioning.
    menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
    usage_popover: ViewHandle<UsagePopoverView>,
    usage_popover_open: bool,
}

impl AgentInputFooter {
    pub fn new(
        menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
        terminal_view_id: EntityId,
        terminal_model: Arc<FairMutex<TerminalModel>>,
        prompt: ModelHandle<PromptType>,
        display_chip_config: DisplayChipConfig,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let button_size = ButtonSize::AgentInputButton;

        let nld_button = ctx.add_typed_action_view(|ctx| {
            let is_nld_enabled = AISettings::as_ref(ctx).is_ai_autodetection_enabled(ctx);
            let mut button = ActionButton::new("", NLDButtonTheme)
                .with_icon(Icon::NLD)
                .with_size(button_size)
                .with_tooltip_alignment(TooltipAlignment::Left)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AgentInputFooterAction::ToggleAutodetectionSetting);
                });
            button.set_active(is_nld_enabled, ctx);
            button.set_tooltip(
                Some(if is_nld_enabled {
                    DISABLE_NLD_TOOLTIP
                } else {
                    ENABLE_NLD_TOOLTIP
                }),
                ctx,
            );
            button
        });
        ctx.subscribe_to_model(&AISettings::handle(ctx), |me, settings, event, ctx| {
            let AISettingsChangedEvent::AIAutoDetectionEnabled { .. } = event else {
                return;
            };
            let is_nld_enabled = settings.as_ref(ctx).is_ai_autodetection_enabled(ctx);
            me.nld_button.update(ctx, |button, ctx| {
                button.set_active(is_nld_enabled, ctx);
                button.set_tooltip(
                    Some(if is_nld_enabled {
                        DISABLE_NLD_TOOLTIP
                    } else {
                        ENABLE_NLD_TOOLTIP
                    }),
                    ctx,
                );
            });
        });

        let file_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("", AgentInputButtonTheme)
                .with_icon(Icon::Plus)
                .with_tooltip("Attach file")
                .with_tooltip_keybinding(ATTACH_FILE_KEYBINDING)
                .with_size(button_size)
                .with_tooltip_alignment(TooltipAlignment::Left)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AgentInputFooterAction::SelectFile);
                })
        });

        // Fast-forward (auto-approve) toggle button.
        // Uses FastForwardButtonTheme so the button keeps its one-off semantics.
        // The theme still delegates its fill to the shared chip background.
        let fast_forward_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("", FastForwardButtonTheme)
                .with_icon(Icon::FastForward)
                .with_tooltip(FAST_FORWARD_OFF_TOOLTIP)
                .with_size(button_size)
                .with_tooltip_alignment(TooltipAlignment::Left)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(TerminalAction::ToggleAutoexecuteMode);
                })
        });

        let file_explorer_button = ctx.add_typed_action_view(|ctx| {
            ActionButton::new("File explorer", AgentInputButtonTheme)
                .with_icon(Icon::FileCopy)
                .with_tooltip("Open file explorer")
                .with_size(button_size)
                .with_tooltip_alignment(TooltipAlignment::Left)
                .with_keybinding(
                    KeystrokeSource::Binding(TOGGLE_PROJECT_EXPLORER_BINDING_NAME),
                    ctx,
                )
                .with_compact_keybinding(true)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AgentInputFooterAction::ToggleFileExplorer);
                })
        });
        let start_remote_control_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("/remote-control", RemoteControlButtonTheme)
                .with_icon(Icon::Phone01)
                .with_tooltip(START_REMOTE_CONTROL_TOOLTIP)
                .with_size(button_size)
                .with_tooltip_alignment(TooltipAlignment::Left)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AgentInputFooterAction::StartRemoteControl);
                })
        });

        let stop_remote_control_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("Stop sharing", RemoteControlButtonTheme)
                .with_icon(Icon::StopFilled)
                .with_icon_ansi_color(AnsiColorIdentifier::Red)
                .with_tooltip("Stop sharing")
                .with_size(button_size)
                .with_tooltip_alignment(TooltipAlignment::Left)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AgentInputFooterAction::StopRemoteControl);
                })
        });

        let context_window_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("", AgentInputButtonTheme)
                .with_icon(Icon::ContextRemaining100)
                .with_tooltip("Context window usage")
                .with_size(button_size)
                .with_tooltip_alignment(TooltipAlignment::Left)
        });

        let usage_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("", AgentInputButtonTheme)
                .with_icon(Icon::PieChart)
                .with_size(button_size)
                .with_tooltip_alignment(TooltipAlignment::Left)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AgentInputFooterAction::ToggleUsagePopover);
                })
        });

        let usage_popover = ctx.add_typed_action_view(|ctx| UsagePopoverView::new(None, ctx));
        ctx.subscribe_to_view(&usage_popover, |me, _, event, ctx| match event {
            UsagePopoverEvent::Close => {
                me.usage_popover_open = false;
                ctx.notify();
            }
        });

        let prompt_alert = ctx.add_typed_action_view(PromptAlertView::new);
        ctx.subscribe_to_view(&prompt_alert, |_, _, event, ctx| {
            ctx.emit(AgentInputFooterEvent::PromptAlert(event.clone()));
        });

        ctx.subscribe_to_model(&NetworkStatus::handle(ctx), |_, _, _, ctx| {
            ctx.notify();
        });
        ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), |me, _, event, ctx| {
            // The tier carries the unit the usage tooltip renders in.
            if matches!(
                event,
                UserWorkspacesEvent::TeamsChanged | UserWorkspacesEvent::CurrentWorkspaceChanged
            ) {
                me.update_usage_button(ctx);
            }
            ctx.notify();
        });
        ctx.subscribe_to_model(&AIRequestUsageModel::handle(ctx), |_, _, _, ctx| {
            ctx.notify()
        });
        ctx.subscribe_to_model(&AISettings::handle(ctx), |me, _, event, ctx| {
            if matches!(event, AISettingsChangedEvent::UsageDisplayUnit { .. }) {
                me.update_usage_button(ctx);
                ctx.notify()
            } else if matches!(event, AISettingsChangedEvent::AIAutoDetectionEnabled { .. }) {
                ctx.notify()
            }
        });
        ctx.subscribe_to_model(&PrivacySettings::handle(ctx), |_, _, event, ctx| {
            if matches!(
                event,
                PrivacySettingsChangedEvent::UpdateIsCloudConversationStorageEnabled { .. }
            ) {
                ctx.notify()
            }
        });
        // The File explorer item's availability follows this setting, so the footer has to
        // repaint when it is toggled rather than waiting for an unrelated re-render.
        ctx.subscribe_to_model(&CodeSettings::handle(ctx), |_, _, event, ctx| {
            if matches!(event, CodeSettingsChangedEvent::ShowProjectExplorer { .. }) {
                ctx.notify()
            }
        });
        ctx.subscribe_to_model(
            &display_chip_config.agent_view_controller,
            |me, _, _, ctx| {
                me.sync_fast_forward_button(ctx);
            },
        );

        // Keep the remote-control chip in sync with login state so we can
        // disable it and swap the tooltip when the user is anonymous or
        // logged out.
        ctx.subscribe_to_model(&AuthManager::handle(ctx), |me, _, _, ctx| {
            me.sync_remote_control_button(ctx);
        });

        let prompt_for_session_settings = prompt.clone();
        ctx.subscribe_to_model(
            &SessionSettings::handle(ctx),
            move |me, _, event, ctx| match event {
                SessionSettingsChangedEvent::AgentToolbarChipSelectionSetting { .. }
                | SessionSettingsChangedEvent::GithubPrChipDefaultValidation { .. } => {
                    me.update_display_chips(&prompt_for_session_settings, ctx);
                    ctx.notify();
                }
                _ => {}
            },
        );
        // Subscribe to AIExecutionProfilesModel to potentially show/hide the profile selector button when profiles are added/removed
        ctx.subscribe_to_model(&AIExecutionProfilesModel::handle(ctx), |_, _, _, ctx| {
            ctx.notify();
        });

        ctx.subscribe_to_model(
            &BlocklistAIHistoryModel::handle(ctx),
            |me, _, event, ctx| {
                if event
                    .terminal_surface_id()
                    .is_some_and(|id| id != me.terminal_view_id)
                {
                    return;
                }

                match event {
                    BlocklistAIHistoryEvent::StartedNewConversation { .. }
                    | BlocklistAIHistoryEvent::SetActiveConversation { .. }
                    | BlocklistAIHistoryEvent::ClearedActiveConversation { .. }
                    | BlocklistAIHistoryEvent::ClearedConversationsForTerminalSurface { .. }
                    | BlocklistAIHistoryEvent::RemoveConversation { .. }
                    | BlocklistAIHistoryEvent::UpdatedAutoexecuteOverride { .. } => {
                        me.sync_fast_forward_button(ctx);
                        me.update_context_window_button(ctx);
                        me.update_usage_button(ctx);
                        me.retarget_usage_popover_if_open(ctx);
                        ctx.notify();
                    }
                    BlocklistAIHistoryEvent::ConversationUsageMetadataUpdated {
                        conversation_id,
                    }
                    | BlocklistAIHistoryEvent::UpdatedConversationMetadata {
                        conversation_id,
                        ..
                    } => {
                        // Only the active conversation's usage affects this
                        // footer's figures, and a metadata-only event can flip
                        // the usage item's visibility, so the footer must
                        // repaint too.
                        let is_active_conversation = BlocklistAIHistoryModel::as_ref(ctx)
                            .active_conversation(me.terminal_view_id)
                            .is_some_and(|conversation| conversation.id() == *conversation_id);
                        if is_active_conversation {
                            me.update_usage_button(ctx);
                            ctx.notify();
                        }
                    }
                    BlocklistAIHistoryEvent::UpdatedTodoList { .. }
                    | BlocklistAIHistoryEvent::UpdatedConversationStatus { .. }
                    | BlocklistAIHistoryEvent::AppendedExchange { .. }
                    | BlocklistAIHistoryEvent::UpdatedStreamingExchange { .. } => {
                        me.update_context_window_button(ctx);
                        me.update_usage_button(ctx);
                        ctx.notify();
                    }
                    _ => (),
                }
            },
        );

        ctx.observe(&prompt, |me, model, ctx| {
            me.update_display_chips(&model, ctx);
        });

        let mut me = Self {
            terminal_view_id,
            nld_button,
            file_button,
            file_explorer_button,
            start_remote_control_button,
            stop_remote_control_button,
            context_window_button,
            usage_button,
            prompt_alert,
            terminal_model,
            left_display_chips: vec![],
            right_display_chips: vec![],
            display_chip_config,
            fast_forward_button,
            prompt_cache_expiry_timer_handle: None,
            prompt_cache_expired: false,
            menu_positioning_provider: menu_positioning_provider.clone(),
            usage_popover,
            usage_popover_open: false,
        };
        me.sync_fast_forward_button(ctx);
        me.sync_remote_control_button(ctx);
        me.update_context_window_button(ctx);
        me.update_usage_button(ctx);
        me.update_display_chips(&prompt, ctx);
        me
    }

    pub fn set_current_repo_path(
        &mut self,
        repo_path: Option<std::path::PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.display_chip_config.current_repo_path = repo_path;
        // Chips will be rebuilt on the next GitRepoStatusEvent::MetadataChanged.
        // Notify to ensure any existing chips reflect the change.
        ctx.notify();
    }

    fn all_display_chips(&self) -> impl Iterator<Item = &ViewHandle<DisplayChip>> {
        self.left_display_chips
            .iter()
            .chain(self.right_display_chips.iter())
    }

    pub fn update_session_context(
        &mut self,
        session_context: Option<SessionContext>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.display_chip_config.session_context = session_context.clone();
        for chip_view in self.all_display_chips() {
            chip_view.update(ctx, |chip, chip_ctx| {
                chip.update_session_context(session_context.clone(), chip_ctx);
            });
        }
    }

    pub(crate) fn select_file(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(AgentInputFooterEvent::SelectFile);
    }

    pub fn has_open_chip_menu(&self, app: &AppContext) -> bool {
        self.all_display_chips()
            .any(|chip| chip.as_ref(app).display_chip_kind().has_open_menu())
    }

    fn sync_fast_forward_button(&self, ctx: &mut ViewContext<Self>) {
        // Read directly from the conversation, same data source as the warping
        // indicator footer's auto-approve chip.
        let is_active = BlocklistAIHistoryModel::as_ref(ctx)
            .active_conversation(self.terminal_view_id)
            .map(|c| c.autoexecute_any_action())
            .unwrap_or(false);

        let icon = if is_active {
            Icon::FastForwardFilled
        } else {
            Icon::FastForward
        };
        let tooltip = if is_active {
            FAST_FORWARD_ON_TOOLTIP
        } else {
            FAST_FORWARD_OFF_TOOLTIP
        };

        self.fast_forward_button.update(ctx, |button, ctx| {
            button.set_icon(Some(icon), ctx);
            button.set_tooltip(Some(tooltip), ctx);
            button.set_active(is_active, ctx);
        });
    }

    /// Disable the start-remote-control chip and swap its tooltip when the
    /// user is anonymous or logged out, since session sharing requires a
    /// real account.
    fn sync_remote_control_button(&self, ctx: &mut ViewContext<Self>) {
        let login_required = AuthStateProvider::as_ref(ctx)
            .get()
            .is_anonymous_or_logged_out();
        let tooltip = if login_required {
            START_REMOTE_CONTROL_LOGIN_REQUIRED_TOOLTIP
        } else {
            START_REMOTE_CONTROL_TOOLTIP
        };
        self.start_remote_control_button.update(ctx, |button, ctx| {
            button.set_disabled(login_required, ctx);
            button.set_tooltip(Some(tooltip), ctx);
        });
    }

    fn update_context_window_button(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(conversation) =
            BlocklistAIHistoryModel::as_ref(ctx).active_conversation(self.terminal_view_id)
        {
            let usage = conversation.context_window_usage();
            let icon = icon_for_context_window_usage(usage);
            let remaining_pct = ((1.0 - usage) * 100.0).round() as i32;

            let expiry = conversation.latest_exchange().and_then(|exchange| {
                let output = exchange.output_status.output()?;
                output.get().model_info.as_ref()?.prompt_cache_expires_at
            });
            let is_cache_expired = FeatureFlag::PromptCacheExpiryWarning.is_enabled()
                && expiry.is_some_and(|expiry| expiry <= Local::now());
            let context_remaining_tooltip = format!("{remaining_pct}% context remaining");
            let tooltip = if is_cache_expired {
                format!("{context_remaining_tooltip} · prompt cache expired")
            } else {
                context_remaining_tooltip
            };

            self.prompt_cache_expired = is_cache_expired;
            self.context_window_button.update(ctx, |button, ctx| {
                button.set_icon(Some(icon), ctx);
                button.set_tooltip(Some(tooltip), ctx);
            });

            self.reschedule_prompt_cache_expiry_timer(expiry, ctx);
        }
    }

    /// Retargets (or closes) an open usage popover when the active
    /// conversation changes, so it never shows the previous conversation's
    /// figures.
    fn retarget_usage_popover_if_open(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.usage_popover_open {
            return;
        }
        let active_conversation_id = BlocklistAIHistoryModel::as_ref(ctx)
            .active_conversation(self.terminal_view_id)
            .map(|conversation| conversation.id());
        if self.usage_popover.as_ref(ctx).conversation_id() == active_conversation_id {
            return;
        }
        match active_conversation_id {
            Some(conversation_id) => self.usage_popover.update(ctx, |popover, ctx| {
                popover.reset_for_conversation(conversation_id, ctx);
            }),
            None => self.usage_popover_open = false,
        }
    }

    /// Refreshes the usage button's tooltip with the active conversation's
    /// total cost — the same figure the popover's header shows.
    fn update_usage_button(&mut self, ctx: &mut ViewContext<Self>) {
        // Falls back to the bare label rather than keeping the previous
        // conversation's figure, which would attribute another conversation's
        // spend to this one.
        let tooltip = BlocklistAIHistoryModel::as_ref(ctx)
            .active_conversation(self.terminal_view_id)
            .map(|conversation| {
                format!(
                    "Conversation usage: {}",
                    conversation_total_text(
                        conversation,
                        conversation_usage_display_unit(conversation, ctx),
                    )
                )
            })
            .unwrap_or_else(|| "Conversation usage".to_string());
        self.usage_button.update(ctx, |button, ctx| {
            button.set_tooltip(Some(tooltip), ctx);
        });
    }

    /// Schedules a refresh of the context-window button at the prompt-cache
    /// expiry instant so the notification dot appears while the conversation is idle.
    fn reschedule_prompt_cache_expiry_timer(
        &mut self,
        expiry: Option<DateTime<Local>>,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(handle) = self.prompt_cache_expiry_timer_handle.take() {
            handle.abort();
        }
        if !FeatureFlag::PromptCacheExpiryWarning.is_enabled() {
            return;
        }
        // Only future expiries need a timer; past ones already render as expired.
        let Some(delay) = expiry.and_then(|expiry| (expiry - Local::now()).to_std().ok()) else {
            return;
        };
        let handle = ctx.spawn(
            async move {
                Timer::after(delay).await;
            },
            |me, _, ctx| {
                me.prompt_cache_expiry_timer_handle = None;
                me.update_context_window_button(ctx);
                ctx.notify();
            },
        );
        self.prompt_cache_expiry_timer_handle = Some(handle);
    }

    fn render_toolbar_item(
        &self,
        item: &AgentToolbarItemKind,
        shared_status: &SharedSessionStatus,
        is_conversation_transcript_context: bool,
        app: &AppContext,
    ) -> Option<Box<dyn Element>> {
        if !item.available_to_session_viewer(shared_status) {
            return None;
        }

        match item {
            AgentToolbarItemKind::ContextChip(chip_kind) => {
                let chips = match SessionSettings::as_ref(app)
                    .agent_footer_chip_selection
                    .left_chips()
                    .contains(chip_kind)
                {
                    true => &self.left_display_chips,
                    false => &self.right_display_chips,
                };
                chips
                    .iter()
                    .find(|chip| chip.as_ref(app).chip_kind() == chip_kind)
                    .filter(|chip| chip.as_ref(app).should_render(app))
                    .map(|chip| ChildView::new(chip).finish())
            }
            AgentToolbarItemKind::NLDToggle => Some(ChildView::new(&self.nld_button).finish()),
            AgentToolbarItemKind::FileAttach => Some(ChildView::new(&self.file_button).finish()),
            AgentToolbarItemKind::ContextWindowUsage => {
                let has_conversation = FeatureFlag::ContextWindowUsageV2.is_enabled()
                    && BlocklistAIHistoryModel::as_ref(app)
                        .active_conversation(self.terminal_view_id)
                        .is_some();
                has_conversation.then(|| {
                    let chip = ChildView::new(&self.context_window_button).finish();
                    if !self.prompt_cache_expired {
                        return chip;
                    }

                    let appearance = Appearance::as_ref(app);
                    let dot = Container::new(
                        ConstrainedBox::new(Empty::new().finish())
                            .with_width(6.)
                            .with_height(6.)
                            .finish(),
                    )
                    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                    .with_background(Fill::Solid(
                        AnsiColorIdentifier::Yellow
                            .to_ansi_color(&appearance.theme().terminal_colors().normal)
                            .into(),
                    ))
                    .finish();

                    let mut stack = Stack::new();
                    stack.add_child(chip);
                    stack.add_positioned_overlay_child(
                        dot,
                        OffsetPositioning::offset_from_parent(
                            vec2f(3., -3.),
                            ParentOffsetBounds::WindowByPosition,
                            ParentAnchor::TopRight,
                            ChildAnchor::TopRight,
                        ),
                    );
                    stack.finish()
                })
            }
            AgentToolbarItemKind::UsageSummary => {
                // A persisted custom toolbar layout is replayed verbatim at render time, so
                // the tier has to be checked here rather than only in `is_available`, which
                // the render path does not consult.
                match UserWorkspaces::as_ref(app).charge_unit() {
                    ChargeUnit::Cents => {}
                    ChargeUnit::Credits => return None,
                }
                let conversation = BlocklistAIHistoryModel::as_ref(app)
                    .active_conversation(self.terminal_view_id)?;
                if !conversation.usage_totals().has_usage {
                    return None;
                }

                let button = SavePosition::new(
                    ChildView::new(&self.usage_button).finish(),
                    USAGE_BUTTON_SAVE_POSITION_ID,
                )
                .finish();
                let mut stack = Stack::new().with_child(button);
                if self.usage_popover_open {
                    let positioning = match self.menu_positioning_provider.menu_position(app) {
                        MenuPositioning::BelowInputBox => {
                            OffsetPositioning::offset_from_save_position_element(
                                USAGE_BUTTON_SAVE_POSITION_ID,
                                vec2f(0., 4.),
                                warpui::elements::PositionedElementOffsetBounds::WindowByPosition,
                                warpui::elements::PositionedElementAnchor::BottomRight,
                                ChildAnchor::TopRight,
                            )
                        }
                        MenuPositioning::AboveInputBox => {
                            OffsetPositioning::offset_from_save_position_element(
                                USAGE_BUTTON_SAVE_POSITION_ID,
                                vec2f(0., -4.),
                                warpui::elements::PositionedElementOffsetBounds::WindowByPosition,
                                warpui::elements::PositionedElementAnchor::TopRight,
                                ChildAnchor::BottomRight,
                            )
                        }
                    };
                    stack.add_positioned_overlay_child(
                        ChildView::new(&self.usage_popover).finish(),
                        positioning,
                    );
                }
                Some(stack.finish())
            }
            AgentToolbarItemKind::ShareSession => {
                if is_conversation_transcript_context {
                    return None;
                }
                let enabled = FeatureFlag::CreatingSharedSessions.is_enabled()
                    && FeatureFlag::HOARemoteControl.is_enabled()
                    && ContextFlag::CreateSharedSession.is_enabled();
                if !enabled {
                    return None;
                }
                let button = if shared_status.is_sharer() {
                    &self.stop_remote_control_button
                } else {
                    &self.start_remote_control_button
                };
                Some(ChildView::new(button).finish())
            }
            AgentToolbarItemKind::FastForwardToggle => FeatureFlag::FastForwardAutoexecuteButton
                .is_enabled()
                .then(|| ChildView::new(&self.fast_forward_button).finish()),

            AgentToolbarItemKind::FileExplorer => item
                .is_available(app)
                .then(|| ChildView::new(&self.file_explorer_button).finish()),
        }
    }

    #[cfg(test)]
    pub fn displayed_chip_kinds(
        &self,
        app: &AppContext,
    ) -> (
        Vec<crate::context_chips::ContextChipKind>,
        Vec<crate::context_chips::ContextChipKind>,
    ) {
        let collect_chip_kinds = |chips: &[ViewHandle<DisplayChip>]| {
            chips
                .iter()
                .map(|chip| chip.as_ref(app).chip_kind().clone())
                .collect()
        };

        (
            collect_chip_kinds(&self.left_display_chips),
            collect_chip_kinds(&self.right_display_chips),
        )
    }

    #[cfg(test)]
    pub fn usage_tooltip_for_test(&self, app: &AppContext) -> Option<String> {
        self.usage_button
            .as_ref(app)
            .tooltip_for_test()
            .map(str::to_string)
    }
}

impl View for AgentInputFooter {
    fn ui_name() -> &'static str {
        "AgentViewFooter"
    }

    fn render(&self, app: &warpui::AppContext) -> Box<dyn warpui::Element> {
        let session_settings = SessionSettings::as_ref(app);
        let left_items = session_settings.agent_footer_chip_selection.left_items();
        let right_items = session_settings.agent_footer_chip_selection.right_items();

        let mut left_buttons = Wrap::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_run_spacing(4.)
            .with_spacing(4.);

        // The lock is released before rendering toolbar items: the usage popover's menu
        // positioning provider re-locks the same non-reentrant model.
        let (shared_status, is_conversation_transcript_context) = {
            let terminal_model = self.terminal_model.lock();
            (
                terminal_model.shared_session_status().clone(),
                is_conversation_transcript_context(self.terminal_view_id, &terminal_model, app),
            )
        };

        for item in &left_items {
            if let Some(element) = self.render_toolbar_item(
                item,
                &shared_status,
                is_conversation_transcript_context,
                app,
            ) {
                left_buttons.add_child(element);
            }
        }

        let mut right_buttons = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(4.);

        let has_prompt_alert = !self.prompt_alert.as_ref(app).is_no_alert();
        if has_prompt_alert {
            right_buttons.add_child(
                Shrinkable::new(
                    1.,
                    Clipped::new(ChildView::new(&self.prompt_alert).finish()).finish(),
                )
                .finish(),
            );
        } else {
            for item in &right_items {
                if let Some(element) = self.render_toolbar_item(
                    item,
                    &shared_status,
                    is_conversation_transcript_context,
                    app,
                ) {
                    right_buttons.add_child(element);
                }
            }
        }

        let content = Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(WrapFill::new(0., left_buttons.finish()).finish())
            .with_child(WrapFill::new(0., right_buttons.finish()).finish())
            .with_run_spacing(context_chips::spacing::UDI_ROW_RUN_SPACING)
            .finish();
        let content = EventHandler::new(content)
            .on_right_mouse_down(|ctx, _, position, _| {
                ctx.dispatch_typed_action(AgentInputFooterAction::ShowContextMenu { position });
                DispatchEventResult::StopPropagation
            })
            .finish();

        let mut container = Container::new(content).with_padding_bottom(8.0);
        if !has_prompt_alert {
            container = container.with_padding_right(16.);
        }

        container.finish()
    }
}

#[derive(Debug, Clone)]
pub enum AgentInputFooterAction {
    SelectFile,
    ToggleFileExplorer,
    ToggleAutodetectionSetting,
    StartRemoteControl,
    StopRemoteControl,
    ShowContextMenu { position: Vector2F },
    ToggleUsagePopover,
}

impl TypedActionView for AgentInputFooter {
    type Action = AgentInputFooterAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut warpui::ViewContext<Self>) {
        match action {
            AgentInputFooterAction::SelectFile => {
                self.select_file(ctx);
            }
            AgentInputFooterAction::ToggleFileExplorer => {
                ctx.emit(AgentInputFooterEvent::ToggleFileExplorer);
            }
            AgentInputFooterAction::ToggleAutodetectionSetting => {
                let ai_settings = AISettings::handle(ctx);
                ai_settings.update(ctx, |settings, ctx| {
                    report_if_error!(
                        settings
                            .ai_autodetection_enabled_internal
                            .toggle_and_save_value(ctx)
                    );
                });
            }
            AgentInputFooterAction::StartRemoteControl => {
                ctx.emit(AgentInputFooterEvent::StartRemoteControl);
            }
            AgentInputFooterAction::StopRemoteControl => {
                ctx.emit(AgentInputFooterEvent::StopRemoteControl);
            }
            AgentInputFooterAction::ShowContextMenu { position } => {
                ctx.emit(AgentInputFooterEvent::ShowContextMenu {
                    position: *position,
                });
            }
            AgentInputFooterAction::ToggleUsagePopover => {
                self.usage_popover_open = !self.usage_popover_open;
                if self.usage_popover_open {
                    let conversation_id = BlocklistAIHistoryModel::as_ref(ctx)
                        .active_conversation(self.terminal_view_id)
                        .map(|conversation| conversation.id());
                    match conversation_id {
                        Some(conversation_id) => {
                            self.usage_popover.update(ctx, |popover, ctx| {
                                popover.reset_for_conversation(conversation_id, ctx);
                            });
                        }
                        None => self.usage_popover_open = false,
                    }
                }
                ctx.notify();
            }
        }
    }
}

pub enum AgentInputFooterEvent {
    SelectFile,
    ToggleFileExplorer,
    StartRemoteControl,
    StopRemoteControl,
    ToggledChipMenu { open: bool },
    TryExecuteChipCommand(PromptChipShellCommand),
    PromptAlert(PromptAlertEvent),
    OpenSettings(SettingsSection),
    OpenCodeReview,
    ShowContextMenu { position: Vector2F },
}

impl Entity for AgentInputFooter {
    type Event = AgentInputFooterEvent;
}

pub(crate) struct AgentInputButtonTheme;

impl ActionButtonTheme for AgentInputButtonTheme {
    fn background(&self, hovered: bool, appearance: &Appearance) -> Option<Fill> {
        // Solid surface fills keep the button readable even when its parent
        // isn't `theme.background()`.
        let theme = appearance.theme();
        Some(if hovered {
            theme.surface_2()
        } else {
            theme.surface_1()
        })
    }

    fn text_color(
        &self,
        _hovered: bool,
        background: Option<Fill>,
        appearance: &Appearance,
    ) -> ColorU {
        // If a caller overrides `background()` with a translucent fill, blend
        // it over `surface_1` so text contrast is computed against the actual
        // rendered color rather than the raw overlay.
        let base_bg = appearance.theme().surface_1();
        let effective_bg = background
            .map(|overlay| base_bg.blend(&overlay))
            .unwrap_or(base_bg);

        appearance.theme().sub_text_color(effective_bg).into_solid()
    }

    fn border(&self, appearance: &Appearance) -> Option<ColorU> {
        Some(internal_colors::neutral_3(appearance.theme()))
    }

    fn should_opt_out_of_contrast_adjustment(&self) -> bool {
        true
    }
}

struct RemoteControlButtonTheme;

impl ActionButtonTheme for RemoteControlButtonTheme {
    fn background(&self, hovered: bool, appearance: &Appearance) -> Option<Fill> {
        AgentInputButtonTheme.background(hovered, appearance)
    }

    fn text_color(
        &self,
        hovered: bool,
        background: Option<Fill>,
        appearance: &Appearance,
    ) -> ColorU {
        AgentInputButtonTheme.text_color(hovered, background, appearance)
    }

    fn border(&self, appearance: &Appearance) -> Option<ColorU> {
        AgentInputButtonTheme.border(appearance)
    }

    fn should_opt_out_of_contrast_adjustment(&self) -> bool {
        AgentInputButtonTheme.should_opt_out_of_contrast_adjustment()
    }
}

/// Keeps the auto-approve chip's muted text semantics while using the shared opaque chip fill.
struct FastForwardButtonTheme;

impl ActionButtonTheme for FastForwardButtonTheme {
    fn background(&self, hovered: bool, appearance: &Appearance) -> Option<Fill> {
        AgentInputButtonTheme.background(hovered, appearance)
    }

    fn text_color(
        &self,
        _hovered: bool,
        _background: Option<Fill>,
        appearance: &Appearance,
    ) -> ColorU {
        appearance
            .theme()
            .sub_text_color(appearance.theme().surface_1())
            .into_solid()
    }

    fn border(&self, appearance: &Appearance) -> Option<ColorU> {
        AgentInputButtonTheme.border(appearance)
    }

    fn should_opt_out_of_contrast_adjustment(&self) -> bool {
        true
    }
}

/// Same as `AgentInputButtonTheme`, except with one-off special active styling for the NLD button.
struct NLDButtonTheme;

impl ActionButtonTheme for NLDButtonTheme {
    fn background(&self, hovered: bool, appearance: &Appearance) -> Option<Fill> {
        AgentInputButtonTheme.background(hovered, appearance)
    }

    fn text_color(
        &self,
        hovered: bool,
        _background: Option<Fill>,
        appearance: &Appearance,
    ) -> ColorU {
        if hovered {
            appearance.theme().ansi_fg_blue()
        } else {
            appearance
                .theme()
                .disabled_text_color(appearance.theme().surface_1())
                .into_solid()
        }
    }

    fn border(&self, appearance: &Appearance) -> Option<ColorU> {
        AgentInputButtonTheme.border(appearance)
    }

    fn should_opt_out_of_contrast_adjustment(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
