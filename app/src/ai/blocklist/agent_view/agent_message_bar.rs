use std::sync::Arc;

use parking_lot::FairMutex;
use warp_core::features::FeatureFlag;
use warp_core::ui::appearance::Appearance;
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{Element, Empty, MouseStateHandle};
use warpui::keymap::Keystroke;
use warpui::platform::OperatingSystem;
use warpui::{
    AppContext, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
};

use super::{AgentViewState, EphemeralMessageModel, EphemeralMessageModelEvent};
use crate::BlocklistAIHistoryModel;
use crate::ai::agent::conversation::AIConversation;
use crate::ai::agent::{
    AIAgentExchangeId, AIAgentOutputStatus, FinishedAIAgentOutput, RenderableAIError,
};
#[cfg(not(target_family = "wasm"))]
use crate::ai::blocklist::agent_view::is_in_cloud_context;
use crate::ai::blocklist::agent_view::shortcuts::AgentShortcutViewModel;
use crate::ai::blocklist::agent_view::zero_state_block::{
    render_ambient_credits_banner, render_dismissible_promo_pill,
};
use crate::ai::blocklist::agent_view::{AgentViewController, AgentViewControllerEvent};
use crate::ai::blocklist::view_util::usage_display_unit;
use crate::ai::blocklist::{
    BlocklistAIContextEvent, BlocklistAIContextModel, BlocklistAIHistoryEvent,
    BlocklistAIInputEvent, BlocklistAIInputModel,
};
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::ai::mcp::templatable_manager::{FigmaMcpStatus, TemplatableMCPServerManagerEvent};
use crate::ai::pricing_promotion::{
    PricingPromotionState, PricingPromotionStateEvent, PricingPromotionSurface,
};
use crate::ai::request_usage_model::{
    AIRequestUsageModel, AIRequestUsageModelEvent, AMBIENT_AGENT_TRIAL_CREDIT_THRESHOLD,
};
use crate::auth::auth_manager::AuthManager;
use crate::settings::{AISettings, AISettingsChangedEvent, UsageDisplayUnit};
use crate::terminal::input::buffer_model::{InputBufferModel, InputBufferUpdateEvent};
use crate::terminal::input::message_bar::attached_context::{
    AttachedBlocksMessageProducer, AttachedContextArgs, AttachedTextSelectionMessageProducer,
};
use crate::terminal::input::message_bar::common::{
    disableable_message_item_color_overrides, render_standard_message_bar,
};
use crate::terminal::input::message_bar::{
    ChipHorizontalAlignment, EmptyMessageProducer, Message, MessageItem, MessageProvider,
};
use crate::terminal::input::{InputAction, SET_INPUT_MODE_AGENT_ACTION_NAME};
use crate::terminal::model::TerminalModel;
use crate::terminal::view::TerminalAction;
use crate::util::bindings::keybinding_name_to_keystroke;
#[cfg(not(target_family = "wasm"))]
use crate::workspace::WorkspaceAction;
use crate::workspace::tab_settings::{TabSettings, TabSettingsChangedEvent};

const FIGMA_ICON_SIZE: f32 = 14.;

#[derive(Clone, Default)]
pub struct AgentMessageBarMouseStates {
    pub resume_conversation: MouseStateHandle,
    pub fork_from_last_known_good_state: MouseStateHandle,
    pub toggle_shortcuts: MouseStateHandle,
    pub toggle_code_review: MouseStateHandle,
    pub clear_attached_context: MouseStateHandle,
    /// Mouse state handle for the "Get Figma MCP" contextual button.
    pub figma_install_button: MouseStateHandle,
    /// Mouse state handle for the "Enable Figma MCP" contextual button.
    pub figma_enable_button: MouseStateHandle,
    /// Mouse state handle for dismissing the ambient credits banner.
    pub ambient_credits_banner_close: MouseStateHandle,
    pub pricing_promotion: MouseStateHandle,
    pub pricing_promotion_close: MouseStateHandle,
}

/// Renders contextual hint text at the bottom of the agent view status bar.
pub struct AgentMessageBar {
    agent_view_controller: ModelHandle<AgentViewController>,
    ephemeral_message_model: ModelHandle<EphemeralMessageModel>,
    shortcut_view_model: ModelHandle<AgentShortcutViewModel>,
    input_buffer_model: ModelHandle<InputBufferModel>,
    input_model: ModelHandle<BlocklistAIInputModel>,
    context_model: ModelHandle<BlocklistAIContextModel>,
    terminal_model: Arc<FairMutex<TerminalModel>>,
    mouse_states: AgentMessageBarMouseStates,
    /// Whether the word "figma" has been detected in the current input buffer or attached images.
    /// Only meaningful when `FeatureFlag::FigmaDetection` is enabled.
    figma_detected: bool,
}

impl Entity for AgentMessageBar {
    type Event = ();
}
#[derive(Clone, Debug)]
pub enum AgentMessageBarAction {
    DismissAmbientCreditsBanner,
    UpgradePricingPromotion,
    DismissPricingPromotion,
}

impl AgentMessageBar {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent_view_controller: ModelHandle<AgentViewController>,
        ephemeral_message_model: ModelHandle<EphemeralMessageModel>,
        shortcut_view_model: ModelHandle<AgentShortcutViewModel>,
        input_buffer_model: ModelHandle<InputBufferModel>,
        input_model: ModelHandle<BlocklistAIInputModel>,
        context_model: ModelHandle<BlocklistAIContextModel>,
        terminal_model: Arc<FairMutex<TerminalModel>>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(&agent_view_controller, |me, _, event, ctx| {
            if matches!(
                event,
                AgentViewControllerEvent::EnteredAgentView { .. }
                    | AgentViewControllerEvent::ExitedAgentView { .. }
            ) {
                me.record_visible_promotion(ctx);
                ctx.notify();
            }
        });
        ctx.subscribe_to_model(&ephemeral_message_model, |_, _, event, ctx| {
            if matches!(event, EphemeralMessageModelEvent::MessageChanged) {
                ctx.notify();
            }
        });
        ctx.subscribe_to_model(&input_model, |_, _, event, ctx| {
            if matches!(
                event,
                BlocklistAIInputEvent::InputTypeChanged { .. }
                    | BlocklistAIInputEvent::LockChanged { .. }
            ) {
                ctx.notify();
            }
        });
        ctx.subscribe_to_model(&input_buffer_model, |me, _, event, ctx| {
            let InputBufferUpdateEvent {
                old_content: old,
                new_content: new,
                ..
            } = event;
            // If the user inputs into the buffer, dismiss any explicit message if we have one.
            me.ephemeral_message_model
                .update(ctx, |m, ctx| m.try_dismiss_explicit_message(ctx));
            let empty_state_changed = old.is_empty() != new.is_empty();
            let in_shell_mode = !me.input_model.as_ref(ctx).is_ai_input_enabled();
            if empty_state_changed || in_shell_mode {
                ctx.notify();
            }
            me.update_figma_detected(ctx);
        });
        ctx.subscribe_to_model(&shortcut_view_model, |_, _, _, ctx| {
            ctx.notify();
        });
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, |_, _, event, ctx| {
            if matches!(
                event,
                BlocklistAIHistoryEvent::UpdatedConversationStatus { .. }
            ) {
                ctx.notify();
            }
        });
        ctx.subscribe_to_model(&PricingPromotionState::handle(ctx), |me, _, event, ctx| {
            if matches!(event, PricingPromotionStateEvent::Updated) {
                me.record_visible_promotion(ctx);
                ctx.notify();
            }
        });
        ctx.subscribe_to_model(&context_model, |me, _, event, ctx| {
            if let BlocklistAIContextEvent::UpdatedPendingContext { .. } = event {
                me.update_figma_detected(ctx);
                ctx.notify();
            }
        });

        ctx.subscribe_to_model(&TabSettings::handle(ctx), |_, _, event, ctx| {
            if matches!(event, TabSettingsChangedEvent::ShowCodeReviewButton { .. }) {
                ctx.notify();
            }
        });

        if FeatureFlag::FigmaDetection.is_enabled() {
            // When the state of the Figma MCP changes, re-render to update the Figma CTA button.
            ctx.subscribe_to_model(
                &TemplatableMCPServerManager::handle(ctx),
                |_, model, event, ctx| {
                    if let TemplatableMCPServerManagerEvent::StateChanged { uuid, .. } = event
                        && let Some(figma_mcp_uuid) =
                            model.as_ref(ctx).get_figma_installation_uuid()
                        && uuid == &figma_mcp_uuid
                    {
                        ctx.notify();
                    }
                },
            );
        }

        ctx.subscribe_to_model(&AIRequestUsageModel::handle(ctx), |_, _, event, ctx| {
            if matches!(
                event,
                AIRequestUsageModelEvent::RequestUsageUpdated
                    | AIRequestUsageModelEvent::CreditAvailabilityUpdated
                    | AIRequestUsageModelEvent::AmbientCreditsBannerDismissed
            ) {
                ctx.notify();
            }
        });
        ctx.subscribe_to_model(&AISettings::handle(ctx), |_, _, event, ctx| {
            if matches!(event, AISettingsChangedEvent::UsageDisplayUnit { .. }) {
                ctx.notify();
            }
        });

        let message_bar = Self {
            agent_view_controller,
            ephemeral_message_model,
            shortcut_view_model,
            input_buffer_model,
            input_model,
            context_model,
            terminal_model,
            mouse_states: AgentMessageBarMouseStates::default(),
            figma_detected: false,
        };
        message_bar.record_visible_promotion(ctx);
        message_bar
    }
}

impl AgentMessageBar {
    /// Sets `figma_detected` by checking both the current input text and attached images.
    /// `figma_detected` is `true` when either the text contains "figma" (case-insensitive)
    /// or any attached image was exported from Figma.
    fn update_figma_detected(&mut self, ctx: &mut ViewContext<Self>) {
        if !FeatureFlag::FigmaDetection.is_enabled() {
            return;
        }
        let text_has_figma = self
            .input_buffer_model
            .as_ref(ctx)
            .current_value()
            .to_lowercase()
            .contains("figma");
        let image_has_figma = self
            .context_model
            .as_ref(ctx)
            .pending_images()
            .iter()
            .any(|image| image.is_figma);
        let detected = text_has_figma || image_has_figma;
        if self.figma_detected != detected {
            self.figma_detected = detected;
            ctx.notify();
        }
    }

    /// Returns the Figma MCP status if the contextual button area should be rendered
    /// (i.e. when `FeatureFlag::FigmaDetection` is enabled and "figma" is detected in the input).
    fn figma_button_status(&self, app: &AppContext) -> Option<FigmaMcpStatus> {
        if FeatureFlag::FigmaDetection.is_enabled() && self.figma_detected {
            Some(TemplatableMCPServerManager::as_ref(app).get_figma_mcp_status())
        } else {
            None
        }
    }

    fn record_visible_promotion(&self, ctx: &mut ViewContext<Self>) {
        if cfg!(target_family = "wasm") || !self.agent_view_controller.as_ref(ctx).is_active() {
            return;
        }
        if PricingPromotionState::as_ref(ctx)
            .visible_message(PricingPromotionSurface::AgentMessageBar, ctx)
            .is_none()
        {
            return;
        }
        PricingPromotionState::handle(ctx).update(ctx, |state, ctx| {
            state.record_displayed(PricingPromotionSurface::AgentMessageBar, ctx);
        });
    }
}

impl View for AgentMessageBar {
    fn ui_name() -> &'static str {
        "AgentMessageBar"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let shortcut_view_model = self.shortcut_view_model.as_ref(app);
        let input_buffer_model = self.input_buffer_model.as_ref(app);
        let input_model = self.input_model.as_ref(app);
        let agent_view_controller = self.agent_view_controller.as_ref(app);
        let context_model = self.context_model.as_ref(app);
        let terminal_model = self.terminal_model.lock();

        let appearance = Appearance::as_ref(app);

        let Some(active_conversation) = agent_view_controller
            .agent_view_state()
            .active_conversation_id()
            .and_then(|id| BlocklistAIHistoryModel::as_ref(app).conversation(&id))
        else {
            // This should never be hit, as the agent view requires there be an active conversation.
            return Empty::new().finish();
        };

        let ephemeral_message_model = self.ephemeral_message_model.as_ref(app);

        let args = AgentMessageArgs {
            active_conversation,
            agent_view_controller,
            ephemeral_message_model,
            shortcut_view_model,
            input_buffer_model,
            input_model,
            context_model,
            terminal_model: &terminal_model,
            appearance,
            app,
            mouse_states: &self.mouse_states,
        };

        // Ephemeral messages take highest priority.
        let Some(mut message) = ephemeral_message_model
            .produce_message(args)
            .or_else(|| BootstrappingMessageProducer.produce_message(args))
            .or_else(|| AttachedBlocksMessageProducer.produce_message(args))
            .or_else(|| AttachedTextSelectionMessageProducer.produce_message(args))
            .or_else(|| AutodetectedBashModeMessageProducer.produce_message(args))
            .or_else(|| ExitBashModeMessageProducer.produce_message(args))
            .or_else(|| HideShortcutsMessageProducer.produce_message(args))
            .or_else(|| ZeroStateMessageProducer.produce_message(args))
            .or_else(|| EmptyMessageProducer.produce_message(args))
        else {
            return Empty::new().finish();
        };

        let right_element = if cfg!(target_family = "wasm") {
            None
        } else if let Some(message) = PricingPromotionState::as_ref(app)
            .visible_message(PricingPromotionSurface::AgentMessageBar, app)
        {
            Some(render_dismissible_promo_pill(
                message,
                appearance.theme().ansi_fg_green(),
                Some(self.mouse_states.pricing_promotion.clone()),
                Some(AgentMessageBarAction::UpgradePricingPromotion),
                self.mouse_states.pricing_promotion_close.clone(),
                AgentMessageBarAction::DismissPricingPromotion,
                app,
            ))
        } else {
            let request_usage_model = AIRequestUsageModel::as_ref(app);
            request_usage_model
                .ambient_only_credits_remaining()
                .filter(|credits| {
                    *credits >= AMBIENT_AGENT_TRIAL_CREDIT_THRESHOLD
                        && !request_usage_model.is_ambient_credits_banner_dismissed()
                })
                .map(|credits| {
                    let usage_cents = match usage_display_unit(app) {
                        UsageDisplayUnit::Dollars => {
                            request_usage_model.ambient_only_usage_cents_remaining()
                        }
                        UsageDisplayUnit::Credits => None,
                    };
                    render_ambient_credits_banner(
                        credits,
                        usage_cents,
                        self.mouse_states.ambient_credits_banner_close.clone(),
                        AgentMessageBarAction::DismissAmbientCreditsBanner,
                        app,
                    )
                })
        };

        // Append a Figma MCP chip to the message if applicable.
        match self.figma_button_status(app) {
            Some(FigmaMcpStatus::NotInstalled) => {
                message.items.push(figma_chip(
                    self.mouse_states.figma_install_button.clone(),
                    "Get Figma MCP",
                    Some(InputAction::FigmaAddButtonClicked),
                ));
            }
            Some(FigmaMcpStatus::Installed) => {
                message.items.push(figma_chip(
                    self.mouse_states.figma_enable_button.clone(),
                    "Enable Figma MCP",
                    Some(InputAction::FigmaEnableButtonClicked),
                ));
            }
            Some(FigmaMcpStatus::Enabling) => {
                message.items.push(
                    figma_chip(
                        self.mouse_states.figma_enable_button.clone(),
                        "Enabling...",
                        None,
                    )
                    .with_is_disabled(true),
                );
            }
            Some(FigmaMcpStatus::Running) | None => {}
        }

        render_standard_message_bar(message, right_element, app)
    }
}

impl TypedActionView for AgentMessageBar {
    type Action = AgentMessageBarAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AgentMessageBarAction::DismissAmbientCreditsBanner => {
                AIRequestUsageModel::handle(ctx).update(ctx, |model, ctx| {
                    model.dismiss_ambient_credits_banner(ctx);
                });
            }
            AgentMessageBarAction::UpgradePricingPromotion => {
                PricingPromotionState::handle(ctx).update(ctx, |state, ctx| {
                    state.record_clicked(PricingPromotionSurface::AgentMessageBar, ctx);
                });
                let upgrade_url = AuthManager::handle(ctx)
                    .update(ctx, |auth_manager, _| auth_manager.upgrade_url());
                ctx.open_url(&upgrade_url);
            }
            AgentMessageBarAction::DismissPricingPromotion => {
                PricingPromotionState::handle(ctx).update(ctx, |state, ctx| {
                    state.dismiss(PricingPromotionSurface::AgentMessageBar, ctx);
                });
                ctx.notify();
            }
        }
    }
}
/// Arguments for agent message producers.
#[derive(Copy, Clone)]
pub struct AgentMessageArgs<'a> {
    pub active_conversation: &'a AIConversation,
    pub agent_view_controller: &'a AgentViewController,
    pub ephemeral_message_model: &'a EphemeralMessageModel,
    pub shortcut_view_model: &'a AgentShortcutViewModel,
    pub input_buffer_model: &'a InputBufferModel,
    pub input_model: &'a BlocklistAIInputModel,
    pub context_model: &'a BlocklistAIContextModel,
    pub terminal_model: &'a TerminalModel,
    pub appearance: &'a Appearance,
    pub app: &'a AppContext,
    pub mouse_states: &'a AgentMessageBarMouseStates,
}

impl AttachedContextArgs for AgentMessageArgs<'_> {
    fn terminal_model(&self) -> &TerminalModel {
        self.terminal_model
    }

    fn input_buffer_model(&self) -> &InputBufferModel {
        self.input_buffer_model
    }

    fn input_model(&self) -> &BlocklistAIInputModel {
        self.input_model
    }

    fn agent_view_controller(&self) -> &AgentViewController {
        self.agent_view_controller
    }

    fn context_model(&self) -> &BlocklistAIContextModel {
        self.context_model
    }

    fn mouse_states(&self) -> &AgentMessageBarMouseStates {
        self.mouse_states
    }
}

/// Produces a message while the shell is still bootstrapping.
struct BootstrappingMessageProducer;

impl MessageProvider<AgentMessageArgs<'_>> for BootstrappingMessageProducer {
    fn produce_message(&self, args: AgentMessageArgs<'_>) -> Option<Message> {
        if args.terminal_model.block_list().is_bootstrapped()
            || args.terminal_model.is_dummy_cloud_mode_session()
            || args.terminal_model.is_shared_ambient_agent_session()
        {
            None
        } else {
            Some(Message::from_text("Starting shell..."))
        }
    }
}

/// Produces the zero state message
/// When a task is stopped, we also include "Cmd+Shift+R to resume conversation".
/// When a plan exists for the active conversation, we also include "cmd-alt-p to view plan".
struct ZeroStateMessageProducer;

impl MessageProvider<AgentMessageArgs<'_>> for ZeroStateMessageProducer {
    fn produce_message(&self, args: AgentMessageArgs<'_>) -> Option<Message> {
        let AgentMessageArgs {
            active_conversation,
            agent_view_controller,
            input_model,
            input_buffer_model,
            terminal_model,
            app,
            mouse_states,
            ..
        } = args;

        let is_locked_shell_input =
            !input_model.is_ai_input_enabled() && input_model.is_input_type_locked();
        if is_locked_shell_input {
            return None;
        }

        let AgentViewState::Active { .. } = agent_view_controller.agent_view_state() else {
            return None;
        };

        let mut items = Vec::new();

        let show_resume =
            active_conversation.status().is_cancelled() || active_conversation.status().is_error();
        if show_resume {
            let resume_keystroke = if OperatingSystem::get().is_mac() {
                Keystroke::parse("cmd-shift-R").expect("keystroke should parse")
            } else {
                Keystroke::parse("ctrl-alt-r").expect("keystroke should parse")
            };
            items.push(MessageItem::clickable(
                vec![
                    MessageItem::keystroke(resume_keystroke),
                    MessageItem::text("to resume conversation"),
                ],
                |ctx| {
                    ctx.dispatch_typed_action(TerminalAction::ResumeConversation);
                },
                mouse_states.resume_conversation.clone(),
            ));
        }

        // Override to disabled text color if the buffer is not empty, because
        // these shortcuts require the buffer be empty to take effect.
        let is_buffer_empty = input_buffer_model.current_value().is_empty();
        let (
            color_override_for_shortcuts_and_commands,
            bg_color_override_for_shortcuts_and_commands,
        ) = disableable_message_item_color_overrides(!is_buffer_empty, app);

        items.push(
            MessageItem::clickable(
                vec![
                    MessageItem::Keystroke {
                        keystroke: Keystroke {
                            key: "?".to_owned(),
                            ..Default::default()
                        },
                        color: color_override_for_shortcuts_and_commands,
                        background_color: bg_color_override_for_shortcuts_and_commands,
                    },
                    MessageItem::Text {
                        content: "for help".into(),
                        color: color_override_for_shortcuts_and_commands,
                    },
                ],
                |ctx| {
                    ctx.dispatch_typed_action(InputAction::ToggleAgentViewShortcuts);
                },
                mouse_states.toggle_shortcuts.clone(),
            )
            .with_is_disabled(!is_buffer_empty),
        );

        #[cfg(not(target_family = "wasm"))]
        let is_cloud_agent = is_in_cloud_context(terminal_model);

        // Code review only works locally.
        #[cfg(not(target_family = "wasm"))]
        if !is_cloud_agent && *TabSettings::as_ref(app).show_code_review_button {
            let code_review_keystroke = if OperatingSystem::get().is_mac() {
                Keystroke::parse("cmd-shift-+").expect("keystroke should parse")
            } else {
                Keystroke::parse("ctrl-shift-+").expect("keystroke should parse")
            };
            items.push(MessageItem::clickable(
                vec![
                    MessageItem::keystroke(code_review_keystroke),
                    MessageItem::text("for code review"),
                ],
                |ctx| {
                    ctx.dispatch_typed_action(WorkspaceAction::ToggleRightPanel);
                },
                mouse_states.toggle_code_review.clone(),
            ));
        }

        if fork_from_last_known_good_state_exchange_id(active_conversation, terminal_model)
            .is_some()
        {
            let fork_keystroke = if OperatingSystem::get().is_mac() {
                Keystroke::parse("cmd-alt-y").expect("keystroke should parse")
            } else {
                Keystroke::parse("ctrl-alt-y").expect("keystroke should parse")
            };
            items.push(MessageItem::clickable(
                vec![
                    MessageItem::keystroke(fork_keystroke),
                    MessageItem::text("to fork and continue"),
                ],
                |ctx| {
                    ctx.dispatch_typed_action(
                        TerminalAction::ForkConversationFromLastKnownGoodState,
                    );
                },
                mouse_states.fork_from_last_known_good_state.clone(),
            ));
        }

        Some(Message::new(items))
    }
}

pub(crate) fn fork_from_last_known_good_state_exchange_id(
    active_conversation: &AIConversation,
    terminal_model: &TerminalModel,
) -> Option<AIAgentExchangeId> {
    if !should_fork_from_last_known_good_state(active_conversation, terminal_model) {
        return None;
    }

    active_conversation
        .exchanges_reversed()
        .filter(|exchange| exchange.has_user_query())
        // Assumes the failed latest exchange is in the root task; exchanges_reversed only
        // iterates root-task exchanges.
        // Skip the failed latest user query; fork from the nearest prior successful one.
        .skip(1)
        .find(|exchange| exchange.output_status.is_finished_and_successful())
        .map(|exchange| exchange.id)
}
fn should_fork_from_last_known_good_state(
    active_conversation: &AIConversation,
    terminal_model: &TerminalModel,
) -> bool {
    if terminal_model.is_conversation_transcript_viewer()
        || terminal_model.shared_session_status().is_viewer()
        || active_conversation.is_viewing_shared_session()
    {
        return false;
    }

    let Some(latest_exchange) = active_conversation.latest_exchange() else {
        return false;
    };

    let error = match &latest_exchange.output_status {
        AIAgentOutputStatus::Finished {
            finished_output: FinishedAIAgentOutput::Error { error, .. },
        } => error,
        _ => return false,
    };

    match error {
        RenderableAIError::QuotaLimit { .. }
        | RenderableAIError::ServerOverloaded
        | RenderableAIError::ContextWindowExceeded(_)
        | RenderableAIError::InvalidApiKey { .. }
        | RenderableAIError::AwsBedrockCredentialsExpiredOrInvalid { .. }
        | RenderableAIError::GeminiEnterpriseCredentialsExpiredOrInvalid
        | RenderableAIError::ChatGPTSubscriptionError { .. } => false,
        // A shell-exit failure can't resume in this (now-dead) pane, but the user
        // can fork from the last known good state to continue in a fresh one.
        RenderableAIError::InternalWarpError
        | RenderableAIError::AgentStreamFailure { .. }
        | RenderableAIError::AgentExitedShell { .. } => true,
        RenderableAIError::Other {
            will_attempt_resume,
            ..
        }
        | RenderableAIError::TransientNetworkError {
            will_attempt_resume,
            ..
        } => !will_attempt_resume,
        // Cloud startup failures mean the agent never started; there is no prior
        // successful state to fork from.
        RenderableAIError::CloudStartupFailed(_) => false,
    }
}

struct HideShortcutsMessageProducer;

impl MessageProvider<AgentMessageArgs<'_>> for HideShortcutsMessageProducer {
    fn produce_message(&self, args: AgentMessageArgs<'_>) -> Option<Message> {
        if !args.shortcut_view_model.is_shortcut_view_open() {
            return None;
        }

        Some(Message::new(vec![MessageItem::clickable(
            vec![
                MessageItem::keystroke(Keystroke {
                    key: "?".to_owned(),
                    ..Default::default()
                }),
                MessageItem::text("to hide help"),
            ],
            |ctx| {
                ctx.dispatch_typed_action(InputAction::ToggleAgentViewShortcuts);
            },
            args.mouse_states.toggle_shortcuts.clone(),
        )]))
    }
}

struct AutodetectedBashModeMessageProducer;

impl MessageProvider<AgentMessageArgs<'_>> for AutodetectedBashModeMessageProducer {
    fn produce_message(&self, args: AgentMessageArgs<'_>) -> Option<Message> {
        let AgentMessageArgs {
            input_buffer_model,
            input_model,
            appearance,
            app,
            ..
        } = args;
        if input_model.is_ai_input_enabled()
            || input_model.is_input_type_locked()
            || input_buffer_model.current_value().is_empty()
        {
            return None;
        }

        let message = match keybinding_name_to_keystroke(SET_INPUT_MODE_AGENT_ACTION_NAME, app) {
            Some(keystroke) => Message::new(vec![
                MessageItem::text("autodetected shell command, "),
                MessageItem::keystroke(keystroke),
                MessageItem::text(" to override"),
            ])
            .with_text_color(appearance.theme().ansi_fg_blue()),
            None => Message::from_text("autodetected shell command"),
        };

        Some(message)
    }
}

struct ExitBashModeMessageProducer;

impl MessageProvider<AgentMessageArgs<'_>> for ExitBashModeMessageProducer {
    fn produce_message(&self, args: AgentMessageArgs<'_>) -> Option<Message> {
        let AgentMessageArgs {
            input_model,
            appearance,
            app,
            ..
        } = args;
        if input_model.is_ai_input_enabled() || !input_model.is_input_type_locked() {
            return None;
        }
        let set_input_mode_agent_keystroke =
            keybinding_name_to_keystroke(SET_INPUT_MODE_AGENT_ACTION_NAME, app)?;

        let text_color = appearance.theme().ansi_fg_blue();

        Some(
            Message::new(vec![
                MessageItem::Keystroke {
                    keystroke: set_input_mode_agent_keystroke,
                    color: None,
                    background_color: None,
                },
                MessageItem::text("to exit shell mode"),
            ])
            .with_text_color(text_color),
        )
    }
}

/// Creates a `MessageItem::Chip` for a Figma MCP contextual action.
/// When `action` is `Some`, the chip is interactive and dispatches that action on click.
/// When `action` is `None`, the chip is returned without an action (caller should disable it).
fn figma_chip(
    mouse_state: MouseStateHandle,
    label: &'static str,
    action: Option<InputAction>,
) -> MessageItem {
    let items = vec![
        MessageItem::Image {
            source: AssetSource::Bundled {
                path: "bundled/svg/figma-colored.svg",
            },
            width: FIGMA_ICON_SIZE,
            height: FIGMA_ICON_SIZE,
        },
        MessageItem::text(label),
    ];
    if let Some(action) = action {
        MessageItem::chip(
            items,
            move |ctx| ctx.dispatch_typed_action(action.clone()),
            mouse_state,
        )
        .with_horizontal_alignment(ChipHorizontalAlignment::Right)
    } else {
        MessageItem::Chip {
            items,
            action: Arc::new(|_| {}),
            mouse_state,
            disabled: true,
            horizontal_alignment: ChipHorizontalAlignment::Right,
        }
    }
}
